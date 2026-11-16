//! A stand-in for a SKAARHOJ PTZ Pro in Raw Panel mode. It speaks the bytes the
//! protocol reference describes for a panel in server mode, with the topology
//! SKAARHOJ's own emulator reports for the PTZ Pro, so the controller can be
//! driven and read without the hardware: from tests through this API, and from
//! a terminal through `ptz-panel fake`.
//!
//! It is not SKAARHOJ's firmware. It answers what the reference says a panel
//! answers; anything the reference leaves open it does the simplest way.

use crate::codec::{self, Color, Lit, Text, ToPanel, Trigger};
use crate::topology::{Input, Topology, PTZ_PRO_TOPOLOGY};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// What one component shows.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Cell {
    pub lit: Lit,
    pub color: Option<Color>,
    pub text: Text,
}

struct Client {
    id: u64,
    stream: TcpStream,
    heartbeat: Option<Duration>,
    last_ping: Instant,
    /// A ping is waiting for any line back.
    unanswered: Option<Instant>,
    /// Client mode: the system has sent `ActivePanel=1`.
    active: bool,
}

struct State {
    cells: BTreeMap<u32, Cell>,
    history: BTreeMap<u32, Vec<Text>>,
    clients: Vec<Client>,
    next_client: u64,
    lines: Vec<String>,
    unknown: Vec<String>,
    silent: bool,
    /// Does not answer `ping`, as a firmware that left that out would not.
    no_ack: bool,
    asleep: BTreeSet<u32>,
    /// The panel-wide operator inactivity timer is independent of TCP heartbeats.
    sleep_timer_ms: u32,
    last_activity: Instant,
    sleeping: bool,
    edges: bool,
    changes: u64,
}

struct Shared {
    state: Mutex<State>,
    topology: Topology,
    quit: AtomicBool,
    /// Server mode: accepting connections. Cleared while "powered off".
    listening: AtomicBool,
}

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.state.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct FakePanel {
    pub addr: SocketAddr,
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

pub const FAKE_MODEL: &str = "SK_PTZPROV2";
pub const FAKE_NAME: &str = "PTZ Pro stand-in";

fn write_line(stream: &mut TcpStream, line: &str) -> bool {
    stream.write_all(line.as_bytes()).and_then(|_| stream.write_all(b"\n")).is_ok()
}

impl Shared {
    fn send_all(&self, line: &str) {
        let mut s = lock(self);
        if s.silent {
            return;
        }
        s.clients.retain_mut(|c| !c.active || write_line(&mut c.stream, line));
    }

    fn reply(&self, client: u64, lines: &[String]) {
        let mut s = lock(self);
        if s.silent {
            return;
        }
        if let Some(c) = s.clients.iter_mut().find(|c| c.id == client) {
            for line in lines {
                write_line(&mut c.stream, line);
            }
        }
    }

    fn handle(&self, client: u64, line: &str) {
        let mut out: Vec<String> = Vec::new();
        {
            let mut s = lock(self);
            // A line still on its way when the connection was cut, or the panel went deaf, is lost.
            if s.silent || !s.clients.iter().any(|c| c.id == client) {
                return;
            }
            s.lines.push(line.to_string());
            if let Some(c) = s.clients.iter_mut().find(|c| c.id == client) {
                c.unanswered = None;
            }
            match codec::parse_to_panel(line) {
                ToPanel::List => {
                    out.extend([
                        format!("_model={FAKE_MODEL}"),
                        "_serial=000014".to_string(),
                        format!("_version=ptz-panel fake {}", env!("CARGO_PKG_VERSION")),
                        format!("_name={FAKE_NAME}"),
                        "_platform=custom".to_string(),
                        "_panelType=Emulation".to_string(),
                        "_support=ASCII".to_string(),
                    ]);
                }
                ToPanel::Topology => {
                    out.push(
                        "_panelTopology_svgbase=<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 2600 1858\"></svg>"
                            .into(),
                    );
                    out.push(format!("_panelTopology_HWC={}", PTZ_PRO_TOPOLOGY.trim()));
                }
                ToPanel::Map => {
                    for c in &self.topology.components {
                        let to = if s.asleep.contains(&c.id) { 0 } else { c.id };
                        out.push(format!("map={}:{to}", c.id));
                    }
                }
                ToPanel::Ping => {
                    if !s.no_ack {
                        out.push("ack".into());
                    }
                }
                ToPanel::Ack | ToPanel::Nack => {}
                ToPanel::ActivePanel => {
                    if let Some(c) = s.clients.iter_mut().find(|c| c.id == client) {
                        c.active = true;
                    }
                    for c in &self.topology.components {
                        out.push(format!("map={}:{}", c.id, c.id));
                    }
                }
                ToPanel::HeartBeatTimer(ms) => {
                    // Reference 4: values of 200 or less turn the heartbeat off.
                    let every = (ms > 200).then(|| Duration::from_millis(ms as u64));
                    if let Some(c) = s.clients.iter_mut().find(|c| c.id == client) {
                        c.heartbeat = every;
                        c.last_ping = Instant::now();
                    }
                    out.push(format!("_heartBeatTimer={}", if every.is_some() { ms } else { 0 }));
                }
                ToPanel::SleepTimer(ms) => {
                    s.sleep_timer_ms = ms;
                    s.last_activity = Instant::now();
                    out.push(format!("_sleepTimer={ms}"));
                }
                ToPanel::GetSleepTimer => out.push(format!("_sleepTimer={}", s.sleep_timer_ms)),
                ToPanel::WakeUp => {
                    s.last_activity = Instant::now();
                    if s.sleeping {
                        s.sleeping = false;
                        out.push("_isSleeping=0".into());
                    }
                }
                ToPanel::Clear => {
                    s.cells.clear();
                    s.changes += 1;
                }
                ToPanel::ClearLeds => {
                    for cell in s.cells.values_mut() {
                        cell.lit = Lit::Off;
                        cell.color = None;
                    }
                    s.changes += 1;
                }
                ToPanel::ClearDisplays => {
                    for cell in s.cells.values_mut() {
                        cell.text = Text::default();
                    }
                    s.changes += 1;
                }
                ToPanel::Brightness(_) | ToPanel::Extended { .. } => {}
                ToPanel::Mode { hwcs, value } => {
                    for hwc in hwcs {
                        s.cells.entry(hwc).or_default().lit = Lit::from_value(value);
                    }
                    s.changes += 1;
                }
                ToPanel::Color { hwcs, value } => {
                    for hwc in hwcs {
                        s.cells.entry(hwc).or_default().color = Color::from_value(value);
                    }
                    s.changes += 1;
                }
                ToPanel::Text { hwcs, value } => {
                    let text = codec::parse_text(&value);
                    for hwc in hwcs {
                        s.cells.entry(hwc).or_default().text = text.clone();
                        let history = s.history.entry(hwc).or_default();
                        history.push(text.clone());
                        if history.len() > 2000 {
                            history.drain(..1000);
                        }
                    }
                    s.changes += 1;
                }
                // Reference 4.7: anything the panel does not understand is answered with nack.
                ToPanel::Unknown(line) => {
                    if !line.is_empty() {
                        s.unknown.push(line);
                        out.push("nack".into());
                    }
                }
            }
        }
        self.reply(client, &out);
    }

    fn serve(self: &Arc<Self>, stream: TcpStream, active: bool) {
        let _ = stream.set_nodelay(true);
        let Ok(reader) = stream.try_clone() else { return };
        let _ = reader.set_read_timeout(Some(Duration::from_millis(50)));
        let id = {
            let mut s = lock(self);
            s.next_client += 1;
            let id = s.next_client;
            s.clients.push(Client { id, stream, heartbeat: None, last_ping: Instant::now(), unanswered: None, active });
            id
        };
        let shared = self.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            loop {
                if shared.quit.load(Ordering::SeqCst) || !lock(&shared).clients.iter().any(|c| c.id == id) {
                    break;
                }
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) if line.ends_with('\n') => {
                        shared.handle(id, line.trim_end_matches(['\r', '\n']));
                        line.clear();
                    }
                    // A partial line: keep it and read on.
                    Ok(_) => {}
                    Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                    Err(_) => break,
                }
            }
            lock(&shared).clients.retain(|c| c.id != id);
        });
    }

    /// Heartbeats: ping each client that asked for them, and hang up on one that
    /// has sent nothing for two seconds after a ping (reference 2.5).
    fn beat(&self) {
        let mut s = lock(self);
        if s.silent {
            return;
        }
        let now = Instant::now();
        if !s.sleeping && s.sleep_timer_ms > 0 && now.duration_since(s.last_activity).as_millis() >= s.sleep_timer_ms as u128 {
            s.sleeping = true;
            // Losing the displayed content exercises the controller's wake-up repaint.
            s.cells.clear();
            s.changes += 1;
            s.clients.retain_mut(|c| !c.active || write_line(&mut c.stream, "_isSleeping=1"));
        }
        s.clients.retain_mut(|c| {
            if c.unanswered.is_some_and(|since| now.duration_since(since) > Duration::from_secs(2)) {
                let _ = c.stream.shutdown(std::net::Shutdown::Both);
                return false;
            }
            if let Some(every) = c.heartbeat {
                if now.duration_since(c.last_ping) >= every {
                    c.last_ping = now;
                    c.unanswered.get_or_insert(now);
                    return write_line(&mut c.stream, "ping");
                }
            }
            true
        });
    }
}

impl FakePanel {
    /// A panel in server mode on a free local port.
    pub fn start() -> FakePanel {
        FakePanel::start_on("127.0.0.1:0").expect("a free local port")
    }

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            state: Mutex::new(State {
                cells: BTreeMap::new(),
                history: BTreeMap::new(),
                clients: Vec::new(),
                next_client: 0,
                lines: Vec::new(),
                unknown: Vec::new(),
                silent: false,
                no_ack: false,
                asleep: BTreeSet::new(),
                sleep_timer_ms: 0,
                last_activity: Instant::now(),
                sleeping: false,
                edges: true,
                changes: 0,
            }),
            topology: Topology::parse(PTZ_PRO_TOPOLOGY).expect("the bundled topology is valid"),
            quit: AtomicBool::new(false),
            listening: AtomicBool::new(true),
        })
    }

    pub fn start_on(bind: &str) -> std::io::Result<FakePanel> {
        let listener = TcpListener::bind(bind)?;
        let addr = listener.local_addr()?;
        let shared = FakePanel::shared();
        let mut threads = Vec::new();
        {
            let shared = shared.clone();
            threads.push(std::thread::spawn(move || {
                let mut listener = Some(listener);
                while !shared.quit.load(Ordering::SeqCst) {
                    // "Powered off" closes the port; "on" opens the same one again.
                    if !shared.listening.load(Ordering::SeqCst) {
                        listener = None;
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    if listener.is_none() {
                        listener = TcpListener::bind(addr).ok();
                    }
                    let Some(l) = &listener else {
                        std::thread::sleep(Duration::from_millis(20));
                        continue;
                    };
                    let _ = l.set_nonblocking(true);
                    match l.accept() {
                        Ok((stream, _)) => {
                            let _ = stream.set_nonblocking(false);
                            shared.serve(stream, true);
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            }));
        }
        threads.push(FakePanel::beater(shared.clone()));
        Ok(FakePanel { addr, shared, threads })
    }

    /// A panel in client mode (reference A.3): it dials `system`, sends `list`,
    /// and sends nothing else until it is told `ActivePanel=1`.
    pub fn dial(system: SocketAddr) -> FakePanel {
        let shared = FakePanel::shared();
        let mut threads = Vec::new();
        {
            let shared = shared.clone();
            threads.push(std::thread::spawn(move || {
                while !shared.quit.load(Ordering::SeqCst) {
                    let connected = !lock(&shared).clients.is_empty();
                    if !connected && shared.listening.load(Ordering::SeqCst) {
                        if let Ok(mut stream) = TcpStream::connect_timeout(&system, Duration::from_millis(500)) {
                            if write_line(&mut stream, "list") {
                                shared.serve(stream, false);
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }));
        }
        threads.push(FakePanel::beater(shared.clone()));
        FakePanel { addr: system, shared, threads }
    }

    fn beater(shared: Arc<Shared>) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            while !shared.quit.load(Ordering::SeqCst) {
                shared.beat();
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    }

    pub fn topology(&self) -> &Topology {
        &self.shared.topology
    }

    // ---- the operator's hands ----

    /// Send one event as the panel would. A four-way button reports its top edge,
    /// unless edges are turned off.
    pub fn event(&self, hwc: u32, trigger: Trigger) {
        let four_way = PTZ_PRO_FOUR_WAY.contains(&hwc);
        let edge = if four_way && lock(&self.shared).edges { 1 } else { 0 };
        self.event_edge(hwc, edge, trigger);
    }

    pub fn event_edge(&self, hwc: u32, edge: u8, trigger: Trigger) {
        let was_sleeping = {
            let mut s = lock(&self.shared);
            s.last_activity = Instant::now();
            std::mem::replace(&mut s.sleeping, false)
        };
        if was_sleeping {
            self.shared.send_all("_isSleeping=0");
        }
        self.shared.send_all(&codec::format_event(hwc, edge, &trigger));
    }

    pub fn down(&self, hwc: u32) {
        self.event(hwc, Trigger::Down);
    }

    pub fn up(&self, hwc: u32) {
        self.event(hwc, Trigger::Up);
    }

    /// A short press: down, 40 ms, up.
    pub fn press(&self, hwc: u32) {
        self.hold(hwc, Duration::from_millis(40));
    }

    pub fn hold(&self, hwc: u32, time: Duration) {
        self.down(hwc);
        std::thread::sleep(time);
        self.up(hwc);
    }

    /// Turn an encoder by `steps`; negative is counter-clockwise.
    pub fn turn(&self, hwc: u32, steps: i32) {
        self.event(hwc, Trigger::Enc(steps));
    }

    /// Deflect a sprung axis: -500 to 500, 0 at rest.
    pub fn deflect(&self, hwc: u32, value: i32) {
        self.event(hwc, Trigger::Speed(value.clamp(-500, 500)));
    }

    /// Four-way buttons report an edge (`HWC#6.1=Down`) or not (`HWC#6=Down`).
    pub fn set_edges(&self, on: bool) {
        lock(&self.shared).edges = on;
    }

    // ---- what the operator sees ----

    pub fn cell(&self, hwc: u32) -> Cell {
        lock(&self.shared).cells.get(&hwc).cloned().unwrap_or_default()
    }

    pub fn text(&self, hwc: u32) -> Text {
        self.cell(hwc).text
    }

    pub fn lit(&self, hwc: u32) -> Lit {
        self.cell(hwc).lit
    }

    pub fn color(&self, hwc: u32) -> Option<Color> {
        self.cell(hwc).color
    }

    pub fn cells(&self) -> BTreeMap<u32, Cell> {
        lock(&self.shared).cells.clone()
    }

    /// Every text a display has been sent, oldest first.
    pub fn history(&self, hwc: u32) -> Vec<Text> {
        lock(&self.shared).history.get(&hwc).cloned().unwrap_or_default()
    }

    pub fn clear_history(&self) {
        lock(&self.shared).history.clear();
    }

    /// Rises whenever anything shown changes.
    pub fn changes(&self) -> u64 {
        lock(&self.shared).changes
    }

    // ---- the wire ----

    /// Every line received, oldest first.
    pub fn lines(&self) -> Vec<String> {
        lock(&self.shared).lines.clone()
    }

    pub fn clear_lines(&self) {
        lock(&self.shared).lines.clear();
    }

    /// Lines that were not understood and answered with `nack`.
    pub fn unknown(&self) -> Vec<String> {
        lock(&self.shared).unknown.clone()
    }

    pub fn connections(&self) -> usize {
        lock(&self.shared).clients.len()
    }

    /// Set the panel's operator-inactivity timeout, as in its own settings. Zero disables sleep.
    pub fn set_sleep_timer(&self, milliseconds: u32) {
        let mut s = lock(&self.shared);
        s.sleep_timer_ms = milliseconds;
        s.last_activity = Instant::now();
    }

    pub fn is_sleeping(&self) -> bool {
        lock(&self.shared).sleeping
    }

    /// Close every connection, as a pulled cable that the network notices.
    pub fn drop_connections(&self) {
        let mut s = lock(&self.shared);
        for c in s.clients.drain(..) {
            let _ = c.stream.shutdown(std::net::Shutdown::Both);
        }
    }

    /// Stop answering and sending without closing anything: a dead link.
    /// Whether the panel answers `ping` with `ack`. It does unless told not to.
    pub fn set_answers_ping(&self, answers: bool) {
        lock(&self.shared).no_ack = !answers;
    }

    pub fn set_silent(&self, silent: bool) {
        lock(&self.shared).silent = silent;
    }

    /// Lose power: connections close, the port closes, everything shown is forgotten.
    pub fn power_off(&self) {
        self.shared.listening.store(false, Ordering::SeqCst);
        self.drop_connections();
        let mut s = lock(&self.shared);
        s.cells.clear();
        s.changes += 1;
    }

    pub fn power_on(&self) {
        self.shared.listening.store(true, Ordering::SeqCst);
    }

    /// Put components to sleep or wake them: the panel announces it with `map=` lines (reference 5.2).
    pub fn set_asleep(&self, hwcs: &[u32], asleep: bool) {
        {
            let mut s = lock(&self.shared);
            for hwc in hwcs {
                if asleep {
                    s.asleep.insert(*hwc);
                    s.cells.remove(hwc);
                } else {
                    s.asleep.remove(hwc);
                }
            }
            s.changes += 1;
        }
        for hwc in hwcs {
            self.shared.send_all(&format!("map={hwc}:{}", if asleep { 0 } else { *hwc }));
        }
    }

    /// Send a raw line, for anything the helpers do not cover.
    pub fn send_line(&self, line: &str) {
        self.shared.send_all(line);
    }

    /// What a component is, for the terminal: `CAM 1 (button)`.
    pub fn label(&self, hwc: u32) -> String {
        match self.shared.topology.get(hwc) {
            Some(c) => format!("{} ({})", c.label, c.input.word()),
            None => format!("HWC {hwc}"),
        }
    }

    /// Components that can be pressed, turned or deflected.
    pub fn controls(&self) -> Vec<(u32, String, Input)> {
        self.shared
            .topology
            .components
            .iter()
            .filter(|c| c.input != Input::None)
            .map(|c| (c.id, c.label.clone(), c.input))
            .collect()
    }
}

/// CAM 1 to CAM 9 on the PTZ Pro: the four-way buttons with displays.
const PTZ_PRO_FOUR_WAY: [u32; 9] = [6, 7, 8, 9, 10, 11, 12, 13, 56];

impl Drop for FakePanel {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::SeqCst);
        self.drop_connections();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Wire {
        stream: TcpStream,
        reader: BufReader<TcpStream>,
    }

    impl Wire {
        fn open(panel: &FakePanel) -> Wire {
            let stream = TcpStream::connect(panel.addr).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            Wire { reader: BufReader::new(stream.try_clone().unwrap()), stream }
        }

        fn send(&mut self, line: &str) {
            self.stream.write_all(format!("{line}\n").as_bytes()).unwrap();
        }

        fn line(&mut self) -> String {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("a line from the panel");
            line.trim_end().to_string()
        }
    }

    // The session of reference 2.4, step by step.
    #[test]
    fn it_answers_the_documented_session() {
        let panel = FakePanel::start();
        let mut w = Wire::open(&panel);
        w.send("list");
        assert_eq!(w.line(), "_model=SK_PTZPROV2");
        assert!(w.line().starts_with("_serial="));
        assert!(w.line().starts_with("_version="));
        assert_eq!(w.line(), "_name=PTZ Pro stand-in");
        assert!(w.line().starts_with("_platform="));
        assert!(w.line().starts_with("_panelType="));
        assert_eq!(w.line(), "_support=ASCII");
        w.send("PanelTopology?");
        assert!(w.line().starts_with("_panelTopology_svgbase=<svg"));
        let topology = w.line();
        let json = topology.strip_prefix("_panelTopology_HWC=").expect("the topology line");
        assert_eq!(Topology::parse(json).unwrap().components.len(), 51);
        w.send("map");
        let maps: Vec<String> = (0..51).map(|_| w.line()).collect();
        assert!(maps.contains(&"map=56:56".to_string()) && maps.iter().all(|m| m.starts_with("map=")));
        w.send("ping");
        assert_eq!(w.line(), "ack");
        w.send("NoSuchCommand");
        assert_eq!(w.line(), "nack");
        w.send("HeartBeatTimer=300");
        assert_eq!(w.line(), "_heartBeatTimer=300");
        let asked = Instant::now();
        assert_eq!(w.line(), "ping");
        assert!(asked.elapsed() >= Duration::from_millis(250), "the first ping comes one interval later");
        w.send("ack");
        assert_eq!(w.line(), "ping");
    }

    #[test]
    fn operator_sleep_is_separate_from_heartbeats_and_can_be_disabled_and_woken() {
        let panel = FakePanel::start();
        let mut w = Wire::open(&panel);
        w.send("SleepTimer=250");
        assert_eq!(w.line(), "_sleepTimer=250");
        for _ in 0..5 {
            w.send("ping");
            assert_eq!(w.line(), "ack");
            std::thread::sleep(Duration::from_millis(30));
        }
        assert_eq!(w.line(), "_isSleeping=1", "TCP traffic is not operator activity");
        assert!(panel.is_sleeping());
        assert_eq!(panel.connections(), 1, "sleep is independent of the connection");
        w.send("SleepTimer=0");
        assert_eq!(w.line(), "_sleepTimer=0");
        assert!(panel.is_sleeping(), "disabling future sleep does not wake an already sleeping panel");
        w.send("WakeUp!");
        assert_eq!(w.line(), "_isSleeping=0");
        std::thread::sleep(Duration::from_millis(300));
        w.send("ping");
        assert_eq!(w.line(), "ack");
        assert!(!panel.is_sleeping());
        w.send("SleepTimer?");
        assert_eq!(w.line(), "_sleepTimer=0");
    }

    #[test]
    fn it_hangs_up_on_a_system_that_does_not_answer_its_ping() {
        let panel = FakePanel::start();
        let mut w = Wire::open(&panel);
        w.send("HeartBeatTimer=300");
        assert_eq!(w.line(), "_heartBeatTimer=300");
        assert_eq!(w.line(), "ping");
        let since = Instant::now();
        let mut rest = String::new();
        while w.reader.read_line(&mut rest).is_ok_and(|n| n > 0) {
            rest.clear();
        }
        let took = since.elapsed();
        assert!(took >= Duration::from_millis(1900) && took < Duration::from_millis(2900), "closed after {took:?}");
        assert_eq!(panel.connections(), 0);
    }

    #[test]
    fn it_shows_what_it_is_sent_and_sends_what_is_done_to_it() {
        let panel = FakePanel::start();
        let mut w = Wire::open(&panel);
        // Reference 1.1.
        w.send("HWC#38=36");
        w.send("HWCc#38=132");
        w.send("HWCt#38=|||Camera|1|CAM 1");
        w.send("HWC#12,13,14=5");
        w.send("ping");
        assert_eq!(w.line(), "ack");
        assert_eq!(panel.lit(38), Lit::On);
        assert_eq!(panel.color(38), Some(Color::Red));
        assert_eq!(panel.text(38), Text { title: "Camera".into(), label: true, line1: "CAM 1".into(), ..Text::default() });
        assert_eq!((panel.lit(12), panel.lit(13), panel.lit(14)), (Lit::Dim, Lit::Dim, Lit::Dim));
        assert!(panel.unknown().is_empty());

        panel.press(27);
        assert_eq!(w.line(), "HWC#27=Down");
        assert_eq!(w.line(), "HWC#27=Up");
        panel.down(6);
        assert_eq!(w.line(), "HWC#6.1=Down", "a four-way button names its edge");
        panel.set_edges(false);
        panel.up(6);
        assert_eq!(w.line(), "HWC#6=Up");
        panel.turn(35, -2);
        assert_eq!(w.line(), "HWC#35=Enc:-2");
        panel.deflect(1, 900);
        assert_eq!(w.line(), "HWC#1=Speed:500");

        w.send("Clear");
        w.send("ping");
        assert_eq!(w.line(), "ack");
        assert_eq!(panel.cell(38), Cell::default());
    }
}
