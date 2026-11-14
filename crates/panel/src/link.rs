//! The threads around the controller: the connection to the panel, the status
//! stream from the show server, the hand-control sender and the E-Stop sender.
//!
//! Two things never wait in the controller's queue. An E-Stop press goes from
//! the panel's socket straight to a thread that does nothing else, and the
//! joystick goes straight to the hand-control sender, which stops the moment
//! the panel is lost.

use crate::codec::{self, Color, FromPanel, Lit, Text, Trigger};
use crate::map::{PanelMap, Role};
use crate::topology::Topology;
use ptz_core::api::{Jog, Status, TransportRequest, WsMsg};
use ptz_core::client::{Api, ApiErr};
use ptz_core::transport::Action;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How often hand control is sent while a control is deflected. The server stops
/// the rig a quarter of a second after the last one.
pub const JOG_EVERY: Duration = Duration::from_millis(50);
/// How often the panel is pinged, and how long it may stay silent once it has
/// shown that it answers pings.
pub const PING_EVERY: Duration = Duration::from_millis(400);
pub const QUIET_LIMIT: Duration = Duration::from_millis(1200);
/// While a control is deflected or a hold is being counted the panel is pinged
/// this often, and the rig is let go of when nothing has come back for
/// `QUIET_CLOSE`.
pub const PING_CLOSE: Duration = Duration::from_millis(100);
pub const QUIET_CLOSE: Duration = Duration::from_millis(350);
/// A panel that has never answered a ping is given this long: a firmware that
/// does not answer still sends its own pings and events.
const QUIET_LIMIT_UNPROVEN: Duration = Duration::from_secs(7);
/// How often a panel that has not yet described itself is asked again.
const ASK_AGAIN: Duration = Duration::from_secs(2);
/// Said when the joystick is over at a moment the rig may not simply start moving.
pub const REST_FIRST: &str = "Let the joystick come to rest, then drive again";
const UNPROVEN: &str = "This panel has not answered a ping, so its link cannot be watched while driving. Hand control stays off";
const HEARTBEAT_MS: u32 = 1000;

/// What one component is asked to show.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Cell {
    pub lit: Lit,
    pub color: Color,
    pub text: Text,
}

/// The operator's hand on the rig, as the sender reads it every 50 ms.
#[derive(Clone, Debug)]
pub struct Hand {
    pub pan: f64,
    pub tilt: f64,
    pub twist: f64,
    pub rocker: f64,
    /// Encoder nudges: a speed held until the instant.
    pub focus: Option<(f64, Instant)>,
    pub elevation: Option<(f64, Instant)>,
    /// The mode gives the joystick to the rig.
    pub enabled: bool,
    pub camera: String,
    /// Fraction of each axis's top speed at full deflection.
    pub speed: f64,
    /// The panel is connected and has answered a ping.
    pub panel_up: bool,
    pub server_up: bool,
    /// The rig was let go of, or handed over, with the joystick's place unknown or
    /// away from rest. The joystick drives nothing until it has reported rest.
    pub rest_first: bool,
}

impl Hand {
    fn new() -> Hand {
        Hand {
            pan: 0.0,
            tilt: 0.0,
            twist: 0.0,
            rocker: 0.0,
            focus: None,
            elevation: None,
            enabled: false,
            camera: String::new(),
            speed: 0.25,
            panel_up: false,
            server_up: false,
            rest_first: false,
        }
    }

    /// Let go of everything. Called when the panel is lost or stops answering:
    /// where the joystick is by the time it is heard from again is not known.
    pub fn release(&mut self) {
        (self.pan, self.tilt, self.twist, self.rocker) = (0.0, 0.0, 0.0, 0.0);
        self.focus = None;
        self.elevation = None;
        self.rest_first = true;
    }

    /// The panel has reported one joystick axis.
    pub fn stick(&mut self, role: Role, v: f64) {
        match role {
            Role::JoyPan => self.pan = v,
            Role::JoyTilt => self.tilt = v,
            Role::JoyZoom => self.twist = v,
            _ => self.rocker = v,
        }
        if !self.stick_deflected() {
            self.rest_first = false;
        }
    }

    /// Any hand control is away from rest, whether or not it is driving anything.
    pub fn deflected(&self, now: Instant) -> bool {
        let held = |v: Option<(f64, Instant)>| v.is_some_and(|(v, until)| v != 0.0 && until > now);
        self.stick_deflected() || held(self.focus) || held(self.elevation)
    }

    /// The joystick is away from rest.
    pub fn stick_deflected(&self) -> bool {
        self.pan != 0.0 || self.tilt != 0.0 || self.twist != 0.0 || self.rocker != 0.0
    }

    /// What to send now; zero whenever anything says the rig must not be driven.
    pub fn jog(&self, now: Instant) -> Jog {
        if !(self.enabled && self.panel_up && self.server_up) {
            return Jog::default();
        }
        let held = |v: Option<(f64, Instant)>| v.filter(|(_, until)| *until > now).map(|(v, _)| v).unwrap_or(0.0);
        let zoom = if self.rocker.abs() > self.twist.abs() { self.rocker } else { self.twist };
        let s = self.speed.clamp(0.0, 1.0);
        // An encoder nudge is new input each time; a joystick found already over is not.
        let stick = if self.rest_first { 0.0 } else { s };
        Jog {
            pan: self.pan * stick,
            tilt: self.tilt * stick,
            zoom: zoom * stick,
            focus: held(self.focus) * s,
            elevation: held(self.elevation) * s,
        }
    }
}

pub struct Shared {
    pub stop: AtomicBool,
    /// A hold is being counted: the panel is checked as closely as while driving.
    pub watch: AtomicBool,
    /// What the panel should show, and a counter that rises when it changes.
    pub surface: Mutex<(u64, BTreeMap<u32, Cell>)>,
    pub hand: Mutex<Hand>,
}

impl Shared {
    pub fn new() -> Arc<Shared> {
        Arc::new(Shared {
            stop: AtomicBool::new(false),
            watch: AtomicBool::new(false),
            surface: Mutex::new((0, BTreeMap::new())),
            hand: Mutex::new(Hand::new()),
        })
    }

    pub fn hand(&self) -> MutexGuard<'_, Hand> {
        self.hand.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn surface(&self) -> MutexGuard<'_, (u64, BTreeMap<u32, Cell>)> {
        self.surface.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Change {
    Songs,
    Presets,
    Media,
    Project,
    Song { id: String, edit_counter: u64, reason: String },
}

pub enum Event {
    /// Something the operator did.
    Panel {
        hwc: u32,
        trigger: Trigger,
    },
    PanelUp {
        model: String,
        name: String,
        topology: Option<Topology>,
    },
    PanelDown(String),
    /// The panel said something, at this instant. Sent after everything it said.
    Heard(Instant),
    /// The panel stopped answering while it was being watched closely.
    PanelQuiet,
    /// The joystick is over but must come to rest first.
    RestFirst,
    Status(Box<Status>),
    Changed(Change),
    ServerUp,
    ServerDown(String),
    /// The E-Stop sender has an answer, or says why it has none yet.
    Estop(Result<Box<Status>, String>),
    EstopWaiting(String),
    HandRefused(String),
}

// ---- the panel ----

pub struct PanelLink {
    pub dial: Option<String>,
    pub listener: Option<TcpListener>,
    pub map: PanelMap,
}

pub(crate) struct Conn {
    stream: TcpStream,
    pending: Vec<u8>,
}

impl Conn {
    pub(crate) fn new(stream: TcpStream) -> std::io::Result<Conn> {
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_millis(15)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        Ok(Conn { stream, pending: Vec::new() })
    }

    pub(crate) fn send(&mut self, lines: &[String]) -> std::io::Result<()> {
        if lines.is_empty() {
            return Ok(());
        }
        let mut out = String::new();
        for line in lines {
            out.push_str(line);
            out.push('\n');
        }
        self.stream.write_all(out.as_bytes())
    }

    /// Whole lines that have arrived; waits at most the read timeout.
    pub(crate) fn lines(&mut self) -> std::io::Result<Vec<String>> {
        let mut chunk = [0u8; 4096];
        match self.stream.read(&mut chunk) {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => self.pending.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(e),
        }
        let mut out = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            let text = String::from_utf8_lossy(&line).trim_matches(['\r', '\n', ' ']).to_string();
            if !text.is_empty() {
                out.push(text);
            }
        }
        // The topology line is long; anything far longer than that is not Raw Panel.
        if self.pending.len() > 4 * 1024 * 1024 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "a line without an end"));
        }
        Ok(out)
    }
}

pub(crate) fn resolve(addr: &str) -> Option<SocketAddr> {
    addr.to_socket_addrs().ok()?.next()
}

/// Run the panel connection until told to stop: connect or accept, serve, and on
/// any loss let go of the rig, say so, and start again.
pub fn run_panel(link: PanelLink, shared: Arc<Shared>, events: Sender<Event>, estop: Sender<()>) {
    if let Some(l) = &link.listener {
        let _ = l.set_nonblocking(true);
    }
    let mut backoff = Duration::from_millis(500);
    let mut next_dial = Instant::now();
    while !shared.stopped() {
        let mut found: Option<(TcpStream, bool)> = None;
        if let Some(l) = &link.listener {
            if let Ok((stream, _)) = l.accept() {
                let _ = stream.set_nonblocking(false);
                found = Some((stream, true));
            }
        }
        if found.is_none() && Instant::now() >= next_dial {
            if let Some(addr) = link.dial.as_deref().and_then(resolve) {
                if let Ok(stream) = TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
                    found = Some((stream, false));
                }
            }
            next_dial = Instant::now() + backoff;
            backoff = (backoff * 2).min(Duration::from_secs(5));
        }
        let Some((stream, dialed_in)) = found else {
            std::thread::sleep(Duration::from_millis(20));
            continue;
        };
        let started = Instant::now();
        let why = serve(stream, dialed_in, &link.map, &shared, &events, &estop);
        {
            let mut hand = shared.hand();
            hand.release();
            hand.panel_up = false;
        }
        if events.send(Event::PanelDown(why)).is_err() {
            return;
        }
        // A connection that held resets the wait; one that fails at once does not.
        if started.elapsed() > Duration::from_secs(5) {
            backoff = Duration::from_millis(500);
        }
        next_dial = Instant::now() + backoff;
    }
}

fn axis(value: i32, inverted: bool) -> f64 {
    let v = (value as f64 / 500.0).clamp(-1.0, 1.0);
    // Sprung controls do not always come back to exactly 0.
    let v = if v.abs() < 0.02 { 0.0 } else { v };
    if inverted {
        -v
    } else {
        v
    }
}

fn serve(
    stream: TcpStream,
    dialed_in: bool,
    map: &PanelMap,
    shared: &Shared,
    events: &Sender<Event>,
    estop: &Sender<()>,
) -> String {
    let mut conn = match Conn::new(stream) {
        Ok(c) => c,
        Err(e) => return format!("The panel connection could not be set up: {e}"),
    };
    let estop_hwc = map.hwc(Role::Estop);
    let axes: Vec<(Role, u32)> = [Role::JoyPan, Role::JoyTilt, Role::JoyZoom, Role::ZoomRocker]
        .into_iter()
        .filter_map(|r| Some((r, map.hwc(r)?)))
        .collect();

    // A panel that dialled in waits to be told it is the active one (reference A.3).
    // `list` goes alone and first: a panel that expects the binary form reads it as
    // an impossible length and switches to ASCII (reference 2.2).
    let mut first = Vec::new();
    if dialed_in {
        first.push("\nActivePanel=1".to_string());
    }
    first.push("list".to_string());
    if let Err(e) = conn.send(&first) {
        return format!("The panel connection closed: {e}");
    }

    let opened = Instant::now();
    let mut asked = false;
    let mut up = false;
    let (mut model, mut name) = (String::new(), String::new());
    let mut topology: Option<Topology> = None;
    let mut heard = Instant::now();
    let mut heard_any = false;
    let mut proven = false;
    let mut last_ping = Instant::now();
    let mut last_ask = Instant::now();
    let mut close_since: Option<Instant> = None;
    let mut let_go = false;
    let mut rest_told: Option<Instant> = None;
    let mut unproven_told = false;
    let mut busy: Option<Instant> = None;
    let mut sent: BTreeMap<u32, Cell> = BTreeMap::new();
    let mut seen = u64::MAX;
    let mut asleep: BTreeSet<u32> = BTreeSet::new();
    let mut resend = false;

    loop {
        if shared.stopped() {
            // The last picture must not stay up as though someone were still behind it.
            if up {
                let mut bye = vec!["Clear".to_string()];
                if let Some(hwc) = map.hwc(Role::Status) {
                    let text =
                        Text { title: "Panel".into(), line1: "Controller".into(), line2: "stopped".into(), ..Text::default() };
                    bye.push(codec::hwc_text(hwc, &text));
                }
                let _ = conn.send(&bye);
            }
            return "The controller is stopping".into();
        }
        let lines = match conn.lines() {
            Ok(l) => l,
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return "The panel closed the connection".into(),
            Err(e) => return format!("The panel connection failed: {e}"),
        };
        let mut out: Vec<String> = Vec::new();
        let spoke = !lines.is_empty();
        for line in lines {
            heard = Instant::now();
            heard_any = true;
            let_go = false;
            match codec::parse(&line) {
                FromPanel::Event { hwc, trigger, .. } => {
                    if Some(hwc) == estop_hwc && matches!(trigger, Trigger::Down | Trigger::Press) {
                        let _ = estop.send(());
                    }
                    if let (Some((role, _)), Trigger::Speed(v)) = (axes.iter().find(|(_, h)| *h == hwc), &trigger) {
                        let v = axis(*v, map.inverted(*role));
                        let waiting = {
                            let mut hand = shared.hand();
                            hand.stick(*role, v);
                            hand.enabled && hand.rest_first && hand.stick_deflected()
                        };
                        let now = Instant::now();
                        if v != 0.0 && up && !proven {
                            if !unproven_told {
                                unproven_told = true;
                                let _ = events.send(Event::HandRefused(UNPROVEN.into()));
                            }
                        } else if waiting && rest_told.is_none_or(|t| now.duration_since(t) > Duration::from_secs(3)) {
                            rest_told = Some(now);
                            let _ = events.send(Event::RestFirst);
                        }
                    }
                    if events.send(Event::Panel { hwc, trigger }).is_err() {
                        return "The controller is stopping".into();
                    }
                }
                FromPanel::Ping => out.push("ack".into()),
                FromPanel::Ack => {
                    if up && !proven {
                        shared.hand().panel_up = true;
                    }
                    proven = true;
                }
                FromPanel::List => out.push("\nActivePanel=1".into()),
                FromPanel::Busy => busy = Some(Instant::now()),
                FromPanel::Ready => busy = None,
                FromPanel::Model(m) => model = m,
                FromPanel::Name(n) => name = n,
                FromPanel::Topology(json) => match Topology::parse(&json) {
                    Ok(t) => {
                        topology = Some(t);
                        if up {
                            // What was written before the panel described itself may not have landed.
                            out.push("Clear".into());
                            sent.clear();
                        }
                        if up
                            && events
                                .send(Event::PanelUp { model: model.clone(), name: name.clone(), topology: topology.clone() })
                                .is_err()
                        {
                            return "The controller is stopping".into();
                        }
                        resend = true;
                    }
                    Err(e) => eprintln!("panel: the panel's topology was not understood: {e}"),
                },
                // A component that comes back from being unavailable has forgotten what it showed (reference 5.2).
                FromPanel::Map { hwc, to, .. } => {
                    if to == 0 {
                        asleep.insert(hwc);
                    } else if asleep.remove(&hwc) {
                        sent.remove(&hwc);
                        resend = true;
                    }
                }
                FromPanel::Sleeping(false) => {
                    sent.clear();
                    resend = true;
                }
                _ => {}
            }
        }
        let now = Instant::now();
        if spoke && events.send(Event::Heard(now)).is_err() {
            return "The controller is stopping".into();
        }

        if !asked && (heard_any || now.duration_since(opened) > Duration::from_millis(500)) {
            asked = true;
            last_ask = now;
            last_ping = now;
            out.extend([
                "PanelTopology?".to_string(),
                "map".to_string(),
                format!("HeartBeatTimer={HEARTBEAT_MS}"),
                "ping".to_string(),
            ]);
        }
        if asked && topology.is_none() && now.duration_since(last_ask) >= ASK_AGAIN {
            last_ask = now;
            out.extend([
                "list".to_string(),
                "PanelTopology?".to_string(),
                "map".to_string(),
                format!("HeartBeatTimer={HEARTBEAT_MS}"),
            ]);
        }
        if !up && heard_any && (topology.is_some() || now.duration_since(opened) > Duration::from_millis(1500)) {
            up = true;
            out.push("Clear".into());
            sent.clear();
            resend = true;
            shared.hand().panel_up = proven;
            if events.send(Event::PanelUp { model: model.clone(), name: name.clone(), topology: topology.clone() }).is_err() {
                return "The controller is stopping".into();
            }
        }
        // Watched closely while anything is deflected or a hold is being counted.
        let close = shared.watch.load(Ordering::SeqCst) || shared.hand().deflected(now);
        if close && close_since.is_none() {
            close_since = Some(now);
            last_ping = now;
            out.push("ping".into());
        } else if !close {
            close_since = None;
        }
        if now.duration_since(last_ping) >= if close { PING_CLOSE } else { PING_EVERY } {
            last_ping = now;
            out.push("ping".into());
        }
        if let Some(since) = close_since.filter(|_| proven && !let_go) {
            if now.duration_since(heard.max(since)) > QUIET_CLOSE {
                let_go = true;
                shared.hand().release();
                if events.send(Event::PanelQuiet).is_err() {
                    return "The controller is stopping".into();
                }
            }
        }
        let limit = if proven { QUIET_LIMIT } else { QUIET_LIMIT_UNPROVEN };
        if now.duration_since(heard) > limit {
            return format!("The panel has not answered for {:.1} s", now.duration_since(heard).as_secs_f64());
        }
        // BSY asks for a pause; a panel that never says RDY is not obeyed for ever.
        let paused = busy.is_some_and(|since| now.duration_since(since) < Duration::from_secs(1));
        if up && !paused {
            let surface = shared.surface();
            if resend || surface.0 != seen {
                seen = surface.0;
                resend = false;
                let ids: BTreeSet<u32> = surface.1.keys().chain(sent.keys()).copied().collect();
                for hwc in ids {
                    if asleep.contains(&hwc) {
                        continue;
                    }
                    let want = surface.1.get(&hwc).cloned().unwrap_or_default();
                    let was = sent.get(&hwc).cloned().unwrap_or_default();
                    if want == was {
                        continue;
                    }
                    let part = topology.as_ref().map(|t| t.get(hwc));
                    let (light, display) = match part {
                        Some(Some(c)) => (c.light, c.display.is_some()),
                        // A component the panel does not have is not written to.
                        Some(None) => (false, false),
                        None => (true, true),
                    };
                    if light && want.color != was.color {
                        out.push(codec::hwc_color(hwc, want.color));
                    }
                    if light && want.lit != was.lit {
                        out.push(codec::hwc_mode(hwc, want.lit));
                    }
                    if display && want.text != was.text {
                        out.push(codec::hwc_text(hwc, &want.text));
                    }
                    sent.insert(hwc, want);
                }
            }
        }
        if let Err(e) = conn.send(&out) {
            return format!("The panel connection closed: {e}");
        }
    }
}

// ---- E-Stop ----

/// Wait for presses and post E-Stop for each. Nothing else runs on this thread,
/// so nothing can be ahead of an E-Stop in a queue.
pub fn run_estop(api: Api, presses: Receiver<()>, shared: Arc<Shared>, events: Sender<Event>) {
    while !shared.stopped() {
        if presses.recv_timeout(Duration::from_millis(200)).is_err() {
            continue;
        }
        while presses.try_recv().is_ok() {}
        let started = Instant::now();
        let mut told = false;
        loop {
            match api.act(Action::Estop, &TransportRequest::default()) {
                Ok(status) => {
                    let _ = events.send(Event::Estop(Ok(Box::new(status))));
                    break;
                }
                Err(ApiErr::Refused { msg, .. }) => {
                    let _ = events.send(Event::Estop(Err(msg)));
                    break;
                }
                Err(ApiErr::Network(msg)) => {
                    if started.elapsed() > Duration::from_secs(10) || shared.stopped() {
                        let _ = events.send(Event::Estop(Err(format!("E-Stop did not reach the server. {msg}"))));
                        break;
                    }
                    if !told {
                        told = true;
                        let _ = events.send(Event::EstopWaiting(msg));
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        }
    }
}

// ---- hand control ----

/// Send hand control while a control is deflected, and exactly one stop when it
/// comes to rest, the mode changes, or the panel or the server is lost.
pub fn run_hand(api: Api, shared: Arc<Shared>, events: Sender<Event>) {
    let mut moving: Option<String> = None;
    let mut refused = false;
    while !shared.stopped() {
        let tick = Instant::now();
        let (jog, camera) = {
            let hand = shared.hand();
            (hand.jog(tick), hand.camera.clone())
        };
        if moving.as_ref().is_some_and(|m| *m != camera || jog.is_zero()) {
            if let Some(was) = moving.take() {
                let _ = api.jog(&was, &Jog::default());
            }
        }
        if jog.is_zero() {
            refused = false;
        } else if !refused && !camera.is_empty() {
            match api.jog(&camera, &jog) {
                Ok(_) => moving = Some(camera),
                Err(ApiErr::Refused { msg, .. }) => {
                    // Not asked again until the control has been let go: the reason stays on the display.
                    refused = true;
                    moving = None;
                    let _ = events.send(Event::HandRefused(msg));
                }
                Err(ApiErr::Network(_)) => moving = None,
            }
        }
        std::thread::sleep(JOG_EVERY.saturating_sub(tick.elapsed()));
    }
    if let Some(was) = moving {
        let _ = api.jog(&was, &Jog::default());
    }
}

// ---- the status stream ----

fn ws_connect(api: &Api) -> Result<tungstenite::WebSocket<TcpStream>, String> {
    let url = api.ws_url();
    let host = url.trim_start_matches("ws://").split('/').next().unwrap_or("").to_string();
    let addr = resolve(&host).ok_or_else(|| format!("{host} does not resolve"))?;
    let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let (socket, _) = tungstenite::client::client(url.as_str(), stream).map_err(|e| e.to_string())?;
    socket.get_ref().set_read_timeout(Some(Duration::from_millis(100))).ok();
    Ok(socket)
}

/// Follow the server's status stream as the app does, and keep the edit lease
/// alive with its heartbeat. Says when the server is lost and when it is back.
pub fn run_status(api: Api, shared: Arc<Shared>, events: Sender<Event>) {
    let mut down_told = false;
    while !shared.stopped() {
        let mut socket = match ws_connect(&api) {
            Ok(s) => s,
            Err(e) => {
                if !down_told {
                    down_told = true;
                    if events.send(Event::ServerDown(format!("Cannot reach the server: {e}"))).is_err() {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
        };
        if events.send(Event::ServerUp).is_err() {
            return;
        }
        let mut last_beat = Instant::now();
        let mut last_heard = Instant::now();
        loop {
            if shared.stopped() {
                let _ = socket.close(None);
                return;
            }
            if last_beat.elapsed() >= Duration::from_secs(2) {
                last_beat = Instant::now();
                if socket.send(tungstenite::Message::text("hb")).is_err() {
                    break;
                }
            }
            match socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    last_heard = Instant::now();
                    let Ok(msg) = serde_json::from_str::<WsMsg>(text.as_str()) else { continue };
                    let event = match msg {
                        WsMsg::Status(s) => Event::Status(s),
                        WsMsg::SongListChanged => Event::Changed(Change::Songs),
                        WsMsg::PresetsChanged => Event::Changed(Change::Presets),
                        WsMsg::MediaChanged => Event::Changed(Change::Media),
                        WsMsg::ProjectChanged => Event::Changed(Change::Project),
                        WsMsg::SongChanged { song_id, edit_counter, reason } => {
                            Event::Changed(Change::Song { id: song_id, edit_counter, reason })
                        }
                        WsMsg::Log { .. } => continue,
                    };
                    if events.send(event).is_err() {
                        return;
                    }
                }
                Ok(tungstenite::Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    // Status arrives at least five times a second; a silent second and a half means the link is gone.
                    if last_heard.elapsed() > Duration::from_millis(1500) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        down_told = true;
        if events.send(Event::ServerDown("Lost the connection to the server. Reconnecting".into())).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hand() -> Hand {
        Hand { enabled: true, panel_up: true, server_up: true, camera: "cam1".into(), speed: 0.5, ..Hand::new() }
    }

    #[test]
    fn the_stick_is_scaled_by_top_speed() {
        let mut h = hand();
        h.pan = 1.0;
        h.tilt = -0.5;
        h.twist = 0.2;
        h.rocker = -0.8;
        let jog = h.jog(Instant::now());
        assert_eq!((jog.pan, jog.tilt, jog.zoom), (0.5, -0.25, -0.4));
    }

    #[test]
    fn nothing_is_sent_unless_mode_panel_and_server_all_allow_it() {
        let now = Instant::now();
        let mut h = hand();
        h.pan = 1.0;
        h.focus = Some((1.0, now + Duration::from_secs(1)));
        assert!(!h.jog(now).is_zero());
        for off in 0..3 {
            let mut h = h.clone();
            match off {
                0 => h.enabled = false,
                1 => h.panel_up = false,
                _ => h.server_up = false,
            }
            assert!(h.jog(now).is_zero(), "case {off}");
        }
    }

    #[test]
    fn an_encoder_nudge_ends_by_itself_and_losing_the_panel_lets_go() {
        let now = Instant::now();
        let mut h = hand();
        h.elevation = Some((1.0, now + Duration::from_millis(200)));
        assert_eq!(h.jog(now).elevation, 0.5);
        assert!(h.jog(now + Duration::from_millis(201)).is_zero());
        h.pan = 0.7;
        h.focus = Some((1.0, now + Duration::from_secs(5)));
        h.release();
        assert!(h.jog(now).is_zero() && !h.stick_deflected());
    }

    #[test]
    fn after_the_rig_is_let_go_of_the_stick_drives_only_once_it_has_been_at_rest() {
        let now = Instant::now();
        let mut h = hand();
        h.stick(Role::JoyPan, 0.6);
        assert_eq!(h.jog(now).pan, 0.3);
        h.release();
        assert!(h.rest_first);

        // Still over when the panel is heard from again: nothing moves, on any axis.
        h.stick(Role::JoyPan, 0.6);
        h.stick(Role::JoyTilt, 0.2);
        assert!(h.jog(now).is_zero() && h.deflected(now));
        h.stick(Role::JoyPan, 0.0);
        assert!(h.jog(now).is_zero(), "one axis at rest is not the joystick at rest");
        h.stick(Role::JoyTilt, 0.0);
        assert!(!h.rest_first);
        h.stick(Role::JoyPan, -0.4);
        assert_eq!(h.jog(now).pan, -0.2);

        // An encoder nudge is new input, and is not held back.
        h.release();
        h.focus = Some((1.0, now + Duration::from_millis(200)));
        assert_eq!(h.jog(now), Jog { focus: 0.5, ..Jog::default() });
        assert!(h.deflected(now) && !h.deflected(now + Duration::from_millis(201)));
    }

    #[test]
    fn a_stick_that_rests_a_little_off_centre_is_at_rest() {
        assert_eq!(axis(6, false), 0.0);
        assert_eq!(axis(-500, false), -1.0);
        assert_eq!(axis(250, true), -0.5);
        assert_eq!(axis(900, false), 1.0);
    }
}
