//! Sony FR7: pan, tilt, zoom and focus over VISCA/IP, presets over CGI.
//!
//! `Link` is one conversation with the camera, used directly by the bring-up
//! tool. `Fr7` runs a `Link` on its own thread twenty times a second so the show
//! never waits on the network.

use super::cgi::{self, Cgi};
use super::visca::{self, Packet, Reply, Turn};
use super::{with_port, Loop, PERIOD};
use crate::rig::{DevicePreset, Drive, Ride};
use ptz_core::api::AdapterStatus;
use ptz_core::pose::{AxisTable, Pose, Preset, RigConfig};
use std::collections::VecDeque;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Pan, tilt, zoom and focus as the camera reports them, in pose units.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Head {
    pub pan: f64,
    pub tilt: f64,
    pub zoom: f64,
    pub focus: f64,
}

/// When each part of a `Head` was read.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ReadTimes {
    pub pan_tilt: Instant,
    pub zoom: Instant,
    pub focus: Instant,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Fr7Config {
    /// `host:port` of the VISCA service.
    pub visca: String,
    /// `host:port` of the web interface.
    pub http: String,
    pub user: String,
    pub password: String,
    pub counts_per_degree: f64,
    pub pan_sign: f64,
    pub tilt_sign: f64,
    pub absolute_moves: bool,
    /// Degrees per second of each speed step; empty uses `visca::speed_table`.
    pub speed_steps: Vec<f64>,
}

impl Fr7Config {
    pub fn of(rig: &RigConfig) -> Fr7Config {
        let host = rig.fr7_host.trim();
        let name = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
        Fr7Config {
            visca: with_port(host, visca::PORT),
            http: format!("{name}:{}", rig.fr7_http_port),
            user: rig.fr7_user.clone(),
            password: rig.fr7_password.clone(),
            counts_per_degree: rig.fr7_counts_per_degree.max(1.0),
            pan_sign: if rig.fr7_invert_pan { -1.0 } else { 1.0 },
            tilt_sign: if rig.fr7_invert_tilt { -1.0 } else { 1.0 },
            absolute_moves: rig.fr7_absolute_moves,
            speed_steps: rig.fr7_speed_steps.clone(),
        }
    }
}

/// One VISCA/IP conversation: sequence numbers, timeouts and retries.
pub struct Link {
    dest: SocketAddr,
    inbox: Inbox,
    seq: u32,
    /// Commands the camera has acknowledged and not yet finished, and when each was acknowledged.
    unfinished: Vec<(u32, Instant)>,
    pub cfg: Fr7Config,
    pub timeout: Duration,
    pub retries: u32,
    /// Round trip of the last exchange.
    pub last_round_trip: Duration,
    table: Vec<f64>,
    /// The first message after a reset carries sequence number zero, not one.
    /// Sony's list leaves this open; the link settles on whichever the camera takes.
    zero_first: bool,
}

/// An FR7 does not answer from port 52381, and it does not answer to the port a
/// message came from. It sends every reply to UDP port 52381 on the controller,
/// from whichever port it chooses. One socket bound there is shared by every camera.
struct Hub {
    socket: UdpSocket,
    waiters: Arc<Mutex<Vec<Waiter>>>,
    next_id: AtomicU64,
}

struct Waiter {
    id: u64,
    target: SocketAddr,
    tx: SyncSender<Vec<u8>>,
}

static HUB: OnceLock<Result<Hub, String>> = OnceLock::new();

fn hub() -> Result<&'static Hub, String> {
    match HUB.get_or_init(Hub::open) {
        Ok(hub) => Ok(hub),
        Err(err) => Err(err.clone()),
    }
}

/// A stand-in camera answers from the port it listens on. A real FR7 answers
/// from some other port, and only when the message was sent to 52381.
fn replies_to(target: SocketAddr, from: SocketAddr) -> bool {
    target.ip() == from.ip() && (from.port() == target.port() || target.port() == visca::PORT)
}

impl Hub {
    fn open() -> Result<Hub, String> {
        let socket = UdpSocket::bind(("0.0.0.0", visca::PORT))
            .map_err(|e| format!("could not listen on UDP port {}: {e}. An FR7 sends its answers to that port", visca::PORT))?;
        let waiters = Arc::new(Mutex::new(Vec::new()));
        let reader = socket.try_clone().map_err(|e| format!("could not listen on UDP port {}: {e}", visca::PORT))?;
        let shared = Arc::clone(&waiters);
        std::thread::Builder::new()
            .name("visca".into())
            .spawn(move || listen(reader, shared))
            .map_err(|e| format!("could not listen on UDP port {}: {e}", visca::PORT))?;
        Ok(Hub { socket, waiters, next_id: AtomicU64::new(1) })
    }

    fn listen(&self, target: SocketAddr, tx: SyncSender<Vec<u8>>) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.waiters.lock().unwrap_or_else(|e| e.into_inner()).push(Waiter { id, target, tx });
        id
    }

    /// Whether a link to this camera is still open.
    fn hears(&self, target: SocketAddr) -> bool {
        self.waiters.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|waiter| waiter.target == target)
    }

    fn silence(&self, id: u64) {
        self.waiters.lock().unwrap_or_else(|e| e.into_inner()).retain(|waiter| waiter.id != id);
    }

    fn send(&self, packet: &[u8], dest: SocketAddr) -> Result<(), String> {
        self.socket.send_to(packet, dest).map(|_| ()).map_err(|e| format!("could not send to {dest}: {e}"))
    }
}

fn listen(socket: UdpSocket, waiters: Arc<Mutex<Vec<Waiter>>>) {
    let mut buf = [0u8; 64];
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
        let bytes = buf[..n].to_vec();
        let guard = waiters.lock().unwrap_or_else(|e| e.into_inner());
        for waiter in guard.iter().filter(|waiter| replies_to(waiter.target, from)) {
            let _ = waiter.tx.try_send(bytes.clone());
        }
    }
}

/// A command whose completion never arrives is taken as finished after this long.
const FORGOTTEN_AFTER: Duration = Duration::from_secs(5);
/// A command that is done as soon as it is acknowledged says so within a millisecond.
const PROMPT: Duration = Duration::from_millis(3);

/// Replies from one camera. A command is acknowledged at once and completed later, when
/// the move ends, so the inbox has to outlive the exchange that sent it.
enum Inbox {
    /// Answers arriving on the port an FR7 sends them to.
    Shared { id: u64, rx: Receiver<Vec<u8>> },
    /// That port belongs to another program. A socket of the link's own still reaches a
    /// stand-in camera, which answers to wherever a message came from.
    Private { socket: UdpSocket, target: SocketAddr },
}

impl Inbox {
    fn open(target: SocketAddr) -> Result<Inbox, String> {
        let Ok(hub) = hub() else {
            let socket = UdpSocket::bind(("0.0.0.0", 0)).map_err(|e| format!("could not open a UDP socket: {e}"))?;
            return Ok(Inbox::Private { socket, target });
        };
        // Two links to one camera share its sequence counter and hear each other's answers.
        // A link that is closing, such as a rig being replaced, is given the time to finish.
        let patience = Instant::now() + Duration::from_secs(2);
        while hub.hears(target) && Instant::now() < patience {
            std::thread::sleep(Duration::from_millis(10));
        }
        let (tx, rx) = mpsc::sync_channel(64);
        Ok(Inbox::Shared { id: hub.listen(target, tx), rx })
    }

    fn send(&self, packet: &[u8], dest: SocketAddr) -> Result<(), String> {
        match self {
            Inbox::Shared { .. } => hub()?.send(packet, dest),
            Inbox::Private { socket, .. } => {
                socket.send_to(packet, dest).map(|_| ()).map_err(|e| format!("could not send to {dest}: {e}"))
            }
        }
    }

    fn recv_until(&self, deadline: Instant) -> Option<Packet> {
        loop {
            let left = deadline.checked_duration_since(Instant::now()).filter(|left| !left.is_zero())?;
            let bytes = match self {
                Inbox::Shared { rx, .. } => rx.recv_timeout(left).ok()?,
                Inbox::Private { socket, target } => {
                    let mut buf = [0u8; 64];
                    socket.set_read_timeout(Some(left)).ok()?;
                    match socket.recv_from(&mut buf) {
                        Ok((n, from)) if from == *target => buf[..n].to_vec(),
                        Ok(_) => continue,
                        Err(_) => return None,
                    }
                }
            };
            if let Some(packet) = Packet::parse(&bytes) {
                return Some(packet);
            }
        }
    }

    /// Forget everything that has arrived and not been read.
    fn empty(&self) {
        match self {
            Inbox::Shared { rx, .. } => while rx.try_recv().is_ok() {},
            Inbox::Private { socket, .. } => {
                let mut buf = [0u8; 64];
                if socket.set_nonblocking(true).is_ok() {
                    while socket.recv_from(&mut buf).is_ok() {}
                }
                let _ = socket.set_nonblocking(false);
            }
        }
    }

    /// Why a real FR7 cannot be heard through this inbox, if it cannot.
    fn deaf(&self) -> Option<String> {
        match self {
            Inbox::Shared { .. } => None,
            Inbox::Private { .. } => hub().err(),
        }
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        if let (Inbox::Shared { id, .. }, Some(Ok(hub))) = (&*self, HUB.get()) {
            hub.silence(*id);
        }
    }
}

impl Link {
    pub fn connect(cfg: &Fr7Config) -> Result<Link, String> {
        let dest = cfg
            .visca
            .to_socket_addrs()
            .map_err(|e| format!("{} is not an address the FR7 can be reached at: {e}", cfg.visca))?
            .next()
            .ok_or_else(|| format!("{} is not an address the FR7 can be reached at", cfg.visca))?;
        let mut link = Link {
            dest,
            inbox: Inbox::open(dest)?,
            seq: 0,
            unfinished: Vec::new(),
            cfg: cfg.clone(),
            timeout: Duration::from_millis(120),
            retries: 2,
            last_round_trip: Duration::ZERO,
            table: visca::speed_table(),
            zero_first: false,
        };
        link.set_speed_table(cfg.speed_steps.clone());
        link.reset()?;
        Ok(link)
    }

    pub fn speed_table(&self) -> &[f64] {
        &self.table
    }

    pub fn set_speed_table(&mut self, table: Vec<f64>) {
        if !table.is_empty() {
            self.table = table;
        }
    }

    fn send_packet(&self, packet: &[u8]) -> Result<(), String> {
        self.inbox.send(packet, self.dest)
    }

    /// Set the camera's sequence counter, and ours, back to zero.
    pub fn reset(&mut self) -> Result<(), String> {
        let packet = Packet::new(visca::TYPE_CONTROL, 0, &visca::RESET).to_bytes();
        for _ in 0..=self.retries {
            self.send_packet(&packet)?;
            let deadline = Instant::now() + self.timeout;
            while let Some(p) = self.inbox.recv_until(deadline) {
                if p.kind == visca::TYPE_CONTROL_REPLY && p.payload == visca::RESET {
                    self.seq = if self.zero_first { u32::MAX } else { 0 };
                    // Sequence numbers start over, so nothing from before the reset can be matched.
                    self.unfinished.clear();
                    self.inbox.empty();
                    return Ok(());
                }
            }
        }
        match self.inbox.deaf() {
            Some(why) => Err(format!("the FR7 is not answering at {}; {why}", self.cfg.visca)),
            None => Err(format!("the FR7 is not answering at {}", self.cfg.visca)),
        }
    }

    /// Send one message and wait for what answers it. A lost packet is sent again
    /// under the same sequence number, as Sony's list asks.
    fn exchange(&mut self, kind: u16, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut resyncs = 0;
        'again: loop {
            self.seq = self.seq.wrapping_add(1);
            let seq = self.seq;
            let packet = Packet::new(kind, seq, payload).to_bytes();
            let began = Instant::now();
            for _ in 0..=self.retries {
                self.send_packet(&packet)?;
                let deadline = Instant::now() + self.timeout;
                while let Some(p) = self.inbox.recv_until(deadline) {
                    // The refusal carries the number it refused. Every link to this camera hears it,
                    // and only the one that sent that number has to start over.
                    if p.kind == visca::TYPE_CONTROL_REPLY && p.payload == visca::BAD_SEQUENCE && p.seq == seq {
                        // Another controller, or a restart, moved the camera's counter.
                        resyncs += 1;
                        if resyncs > 2 {
                            return Err("the FR7 keeps refusing the sequence number; is another controller driving it?".into());
                        }
                        if resyncs == 2 {
                            self.zero_first = !self.zero_first;
                        }
                        self.reset()?;
                        continue 'again;
                    }
                    if p.kind != visca::TYPE_REPLY {
                        continue;
                    }
                    if p.seq != seq {
                        // An earlier command has run to its end, or been given up.
                        if matches!(visca::parse_reply(&p.payload), Some(Reply::Done(_) | Reply::Error(_))) {
                            self.unfinished.retain(|(earlier, _)| *earlier != p.seq);
                        }
                        continue;
                    }
                    match visca::parse_reply(&p.payload) {
                        // An inquiry is answered by its data; a command by its acknowledgement.
                        Some(Reply::Ack) if kind == visca::TYPE_COMMAND => {
                            self.last_round_trip = began.elapsed();
                            self.unfinished.retain(|(_, at)| at.elapsed() < FORGOTTEN_AFTER);
                            if !self.finishes_promptly(seq) {
                                self.unfinished.push((seq, Instant::now()));
                            }
                            return Ok(Vec::new());
                        }
                        Some(Reply::Done(data)) => {
                            self.last_round_trip = began.elapsed();
                            return Ok(data);
                        }
                        Some(Reply::Error(code)) => {
                            return Err(format!("the FR7 refused a command: {}", visca::error_text(code)))
                        }
                        _ => {}
                    }
                }
            }
            return Err(format!("the FR7 stopped answering at {}", self.cfg.visca));
        }
    }

    /// Whether the command just acknowledged is also completed straight away. A move is not:
    /// its completion comes when the head or the lens arrives.
    fn finishes_promptly(&mut self, seq: u32) -> bool {
        let deadline = Instant::now() + PROMPT;
        while let Some(p) = self.inbox.recv_until(deadline) {
            if p.kind == visca::TYPE_REPLY && matches!(visca::parse_reply(&p.payload), Some(Reply::Done(_) | Reply::Error(_))) {
                if p.seq == seq {
                    return true;
                }
                self.unfinished.retain(|(earlier, _)| *earlier != p.seq);
            }
        }
        false
    }

    pub fn command(&mut self, payload: &[u8]) -> Result<(), String> {
        self.exchange(visca::TYPE_COMMAND, payload).map(|_| ())
    }

    /// Send a command that takes time to carry out, and get the number to ask `running` about.
    pub fn begin(&mut self, payload: &[u8]) -> Result<u32, String> {
        self.command(payload).map(|()| self.seq)
    }

    /// Whether the camera is still carrying out a command it acknowledged.
    pub fn running(&self, command: u32) -> bool {
        self.unfinished.iter().any(|(seq, at)| *seq == command && at.elapsed() < FORGOTTEN_AFTER)
    }

    pub fn inquire(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.exchange(visca::TYPE_INQUIRY, payload)
    }

    fn counts(&self, degrees: f64, sign: f64) -> i32 {
        (degrees * sign * self.cfg.counts_per_degree).round() as i32
    }

    /// Raw pan and tilt counts, as the camera gives them.
    pub fn read_counts(&mut self) -> Result<(i32, i32), String> {
        let data = self.inquire(&visca::pan_tilt_inquiry())?;
        visca::parse_pan_tilt(&data).ok_or_else(|| "the FR7's position answer was not understood".to_string())
    }

    /// Pan and tilt in degrees.
    pub fn read_pan_tilt(&mut self) -> Result<(f64, f64), String> {
        let (pan, tilt) = self.read_counts()?;
        let cpd = self.cfg.counts_per_degree;
        Ok((pan as f64 / cpd * self.cfg.pan_sign, tilt as f64 / cpd * self.cfg.tilt_sign))
    }

    pub fn read_zoom(&mut self) -> Result<f64, String> {
        let data = self.inquire(&visca::zoom_inquiry())?;
        let counts = visca::parse_value(&data).ok_or_else(|| "the FR7's zoom answer was not understood".to_string())?;
        Ok(counts as f64 / visca::ZOOM_TELE as f64)
    }

    pub fn read_focus(&mut self) -> Result<f64, String> {
        let data = self.inquire(&visca::focus_inquiry())?;
        let counts = visca::parse_value(&data).ok_or_else(|| "the FR7's focus answer was not understood".to_string())?;
        Ok(counts as f64 / 65535.0)
    }

    pub fn read_head(&mut self) -> Result<Head, String> {
        let (pan, tilt) = self.read_pan_tilt()?;
        Ok(Head { pan, tilt, zoom: self.read_zoom()?, focus: self.read_focus()? })
    }

    /// Pan and tilt at these speed steps and directions.
    pub fn drive_steps(&mut self, pan: (u8, Turn), tilt: (u8, Turn)) -> Result<(), String> {
        self.command(&visca::pan_tilt_drive(pan.0, tilt.0, pan.1, tilt.1))
    }

    /// Go to a pan and tilt, each axis at the slowest step that covers its distance in `seconds`.
    pub fn go_to(&mut self, from: (f64, f64), to: (f64, f64), seconds: f64) -> Result<(), String> {
        let seconds = seconds.max(0.05);
        let pan_step = visca::step_at_least(&self.table, (to.0 - from.0).abs() / seconds);
        let tilt_step = visca::step_at_least(&self.table, (to.1 - from.1).abs() / seconds);
        let (pan, tilt) = (self.counts(to.0, self.cfg.pan_sign), self.counts(to.1, self.cfg.tilt_sign));
        self.command(&visca::pan_tilt_absolute(pan_step, tilt_step, pan, tilt))
    }

    pub fn zoom_to(&mut self, zoom: f64) -> Result<(), String> {
        self.command(&visca::zoom_direct(zoom_counts(zoom)))
    }

    pub fn focus_to(&mut self, focus: f64) -> Result<(), String> {
        self.command(&visca::focus_direct(focus_counts(focus)))
    }

    /// Stop pan, tilt, zoom and focus. Every stop is sent even if an earlier one fails.
    /// Autofocus refuses a focus stop; the focus motor is not being driven then.
    pub fn stop_all(&mut self) -> Result<(), String> {
        let pan = self.command(&visca::pan_tilt_stop());
        let zoom = self.command(&visca::zoom_stop());
        let focus = match self.command(&visca::focus_stop()) {
            Err(why) if why.contains("current mode") => Ok(()),
            other => other,
        };
        [pan, zoom, focus].into_iter().collect()
    }

    pub fn recall(&mut self, number: u32) -> Result<(), String> {
        self.command(&visca::preset_recall(number))
    }
}

pub fn zoom_counts(zoom: f64) -> u16 {
    (zoom.clamp(0.0, 1.0) * visca::ZOOM_TELE as f64).round() as u16
}

/// Positive speed has to increase the angle we report. VISCA Plus does that unless the
/// camera's direction for the axis is Opposite, or the axis sign is already reversed.
fn directed(turn: (u8, visca::Turn), sign: f64, opposite: bool) -> (u8, visca::Turn) {
    if (sign < 0.0) ^ opposite {
        let way = match turn.1 {
            visca::Turn::Plus => visca::Turn::Minus,
            visca::Turn::Minus => visca::Turn::Plus,
            visca::Turn::Hold => visca::Turn::Hold,
        };
        (turn.0, way)
    } else {
        turn
    }
}

/// The speed steps VISCA drives a power-zoom lens at, and the share of the zoom range each covers
/// in a second. Measured on an FE PZ 28-135mm F4 G OSS. Step 1 runs like step 0, so it is left out.
const ZOOM_SPEEDS: [(u8, f64); 7] = [(0, 0.03), (2, 0.065), (3, 0.11), (4, 0.16), (5, 0.21), (6, 0.34), (7, 0.49)];
/// Share of the zoom error closed per second. Low, so the timeline's own speed leads and the
/// correction only trims it.
const ZOOM_GAIN: f64 = 1.2;
/// How much of each new zoom reading is believed. The lens reports in steps, so the distance
/// from the target seems to jump as it runs; believed whole, every jump would become a change of speed.
const ZOOM_TRUST: f64 = 0.5;
/// The lens takes about this long to act on a speed command, so it is told what the timeline
/// will be doing by then.
const ZOOM_LEAD: f64 = 0.35;
/// The shortest time between two changes of zoom speed.
const ZOOM_DWELL: Duration = Duration::from_millis(150);
/// How far past the midpoint between two steps the wanted speed has to go before the step changes,
/// as a share of the gap. Without it the lens flickers between the two.
const ZOOM_HOLD: f64 = 0.2;
/// A zoom reading older than this is no longer carried forward at the speed the lens was told.
const ZOOM_READING_AGE: f64 = 1.5;
/// The lens starts, stops and changes speed this long after it is told to.
const ZOOM_LAG: f64 = 0.2;
/// A lens told to stop has come to rest this long after, at the latest. It was seen to take up to 0.28 s.
const ZOOM_SETTLE: f64 = 0.45;
/// The lens reports its zoom in steps of up to 0.022. Asked to rest closer than it can report,
/// it creeps back and forth for ever, and that shows in the picture.
const ZOOM_REST: f64 = 0.012;

/// The middle of the last three values of something that is read once a cycle: one value that
/// is far off the others is left out, and a change that stays comes through a cycle later.
///
/// The zoom speed in a command needs it. That speed is how far the target went since the command
/// before, and the show clock follows the audio device: when that takes the clock back a touch,
/// one command says the zoom runs backwards while the target carries on.
fn steady_speed(last: &mut [f64; 3], given: f64) -> f64 {
    last.rotate_left(1);
    last[2] = given;
    let mut sorted = *last;
    sorted.sort_by(f64::total_cmp);
    sorted[1]
}

/// What a lens driven by speed should be running at: the timeline's own speed a moment from now,
/// plus a share of how far the lens is from the target.
#[derive(Clone, Copy, Debug)]
struct ZoomAim {
    /// How far the lens is from the target, smoothed over the coarse steps it reports in.
    error: f64,
    seen: Option<Instant>,
    velocity: f64,
    velocity_at: Instant,
    /// How fast the timeline's speed is changing, smoothed.
    accel: f64,
    /// The last three changes of speed, each over the time it took.
    changes: [f64; 3],
}

impl ZoomAim {
    fn new(now: Instant) -> ZoomAim {
        ZoomAim { error: 0.0, seen: None, velocity: 0.0, velocity_at: now, accel: 0.0, changes: [0.0; 3] }
    }

    /// The speed wanted now, as a share of the zoom range per second, positive toward tele.
    /// `zoom` is the last reading and `read_at` when it was taken, `now_at` where the lens has got
    /// to since, `planned_top` the fastest zoom the timeline is allowed to ask for and `top` the
    /// fastest the lens may be driven.
    #[allow(clippy::too_many_arguments)]
    fn wanted(
        &mut self,
        now: Instant,
        target: f64,
        velocity: f64,
        zoom: f64,
        now_at: f64,
        read_at: Instant,
        planned_top: f64,
        top: f64,
    ) -> f64 {
        let dt = now.saturating_duration_since(self.velocity_at).as_secs_f64();
        if dt > 0.6 {
            (self.accel, self.changes) = (0.0, [0.0; 3]);
        } else if dt > 1e-3 {
            // A timeline that starts or stops all at once changes speed in one step. That is not
            // a speed-up that carries on, and taken for one it would send the lens off at several
            // times the speed asked for. The middle of three changes leaves a single step out.
            let change = steady_speed(&mut self.changes, (velocity - self.velocity) / dt);
            self.accel += 0.5 * (change - self.accel);
        }
        (self.velocity, self.velocity_at) = (velocity, now);

        let error = target - now_at;
        if self.seen != Some(read_at) {
            self.seen = Some(read_at);
            self.error += ZOOM_TRUST * (error - self.error);
        }

        if velocity.abs() < 0.005 && (target - zoom).abs() < ZOOM_REST {
            return 0.0;
        }
        // The timeline's speed a moment from now, without running past its top speed or backing up.
        let cap = planned_top.max(velocity.abs());
        let ahead = velocity + self.accel * ZOOM_LEAD;
        let lead = if velocity > 1e-9 {
            ahead.clamp(0.0, cap)
        } else if velocity < -1e-9 {
            ahead.clamp(-cap, 0.0)
        } else {
            0.0
        };
        (lead + ZOOM_GAIN * self.error).clamp(-top, top)
    }
}

/// Zoom on a lens that only takes speed commands, through VISCA. The lens has seven speeds there
/// and every change of speed shows in the picture, so the speed moves one step at a time and no
/// more often than it has to: up through the steps as a move gets going, level, and down again.
/// Used when the camera's web interface, which takes any speed, cannot be reached.
#[derive(Clone, Copy, Debug)]
struct ZoomRamp {
    /// Toward tele or not, and the index into `ZOOM_SPEEDS`. None while the lens is stopped.
    running: Option<(bool, usize)>,
    changed: Instant,
    aim: ZoomAim,
}

impl ZoomRamp {
    fn new(now: Instant) -> ZoomRamp {
        ZoomRamp { running: None, changed: now, aim: ZoomAim::new(now) }
    }

    fn speed(running: Option<(bool, usize)>) -> f64 {
        match running {
            Some((tele, i)) => ZOOM_SPEEDS[i].1 * if tele { 1.0 } else { -1.0 },
            None => 0.0,
        }
    }

    fn command(running: Option<(bool, usize)>) -> Vec<u8> {
        match running {
            Some((tele, i)) => visca::zoom_toward(tele, ZOOM_SPEEDS[i].0),
            None => visca::zoom_stop(),
        }
    }

    /// What the lens should be doing now.
    #[allow(clippy::too_many_arguments)]
    fn decide(
        &mut self,
        now: Instant,
        target: f64,
        velocity: f64,
        zoom: f64,
        read_at: Instant,
        scale: f64,
        planned_top: f64,
    ) -> Option<(bool, usize)> {
        let top = ZOOM_SPEEDS[ZOOM_SPEEDS.len() - 1].1 * scale.clamp(0.0, 1.0);
        // Where the lens has got to since it was read, at the speed it was told to run.
        let since = now.saturating_duration_since(read_at).as_secs_f64().min(ZOOM_READING_AGE);
        let now_at = zoom + Self::speed(self.running) * since;
        let wanted = self.aim.wanted(now, target, velocity, zoom, now_at, read_at, planned_top, top);
        let (size, tele) = (wanted.abs(), wanted > 0.0);
        let slowest = ZOOM_SPEEDS[0].1;
        match self.running {
            None if size >= slowest * 0.6 => {
                let at_most = ZOOM_SPEEDS.partition_point(|(_, speed)| *speed <= size);
                Some((tele, at_most.saturating_sub(1)))
            }
            None => None,
            Some((way, _)) if way != tele || size < slowest * 0.5 => None,
            Some((way, i)) if now.saturating_duration_since(self.changed) >= ZOOM_DWELL => {
                let speed = |i: usize| ZOOM_SPEEDS[i].1;
                if i + 1 < ZOOM_SPEEDS.len() && size > (speed(i) + speed(i + 1)) / 2.0 + ZOOM_HOLD * (speed(i + 1) - speed(i)) {
                    Some((way, i + 1))
                } else if i > 0 && size < (speed(i - 1) + speed(i)) / 2.0 - ZOOM_HOLD * (speed(i) - speed(i - 1)) {
                    Some((way, i - 1))
                } else {
                    Some((way, i))
                }
            }
            running => running,
        }
    }

    fn took(&mut self, running: Option<(bool, usize)>, now: Instant) {
        self.running = running;
        self.changed = now;
    }
}

/// The zoom speeds the camera's web interface takes, 0 to 32766, and the share of the zoom range
/// each covers in a second. Measured on an FE PZ 28-135mm F4 G OSS. Below the first the lens runs
/// no slower.
const WEB_ZOOM: [(u16, f64); 13] = [
    (0, 0.038),
    (2000, 0.043),
    (4000, 0.0495),
    (6000, 0.059),
    (8000, 0.068),
    (10000, 0.076),
    (12000, 0.0895),
    (14000, 0.107),
    (16000, 0.127),
    (20000, 0.1775),
    (24000, 0.25),
    (28000, 0.348),
    (32766, 0.49),
];
/// A new speed is sent when it differs from the last by this share of it.
const GLIDE_STEP: f64 = 0.03;
/// The shortest time between two changes of speed while the lens runs.
const GLIDE_EVERY: Duration = Duration::from_millis(100);
/// A lens at rest this close to its target is brought in by running it at its slowest for a measured time.
const GLIDE_NUDGE: f64 = 0.06;
/// How far ahead the moment to stop is fixed. Further off, the estimate of where the move ends is too rough.
const GLIDE_HORIZON: f64 = 1.0;

/// What the lens is to be told: a speed, positive toward tele and zero to stop, and when to stop by itself.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ZoomOrder {
    rate: f64,
    stop_at: Option<Instant>,
}

/// Zoom on a lens that only takes speed commands, through the camera's web interface. Any speed
/// can be asked for there, so the lens is started once, its speed then follows the timeline in
/// changes too small to see, and it is stopped once, early enough to coast onto its mark.
#[derive(Clone, Debug)]
struct ZoomGlide {
    /// Share of the range per second the lens was last told to run at, positive toward tele. Zero is stopped.
    running: f64,
    /// When the speed last changed.
    sent_at: Instant,
    /// When the lens stops by itself, if a stop has been fixed.
    stop_at: Option<Instant>,
    stopped_at: Option<Instant>,
    /// The lens is on a measured run at its slowest toward a target that stands still.
    nudging: bool,
    /// Every speed the lens was told over the last few seconds, oldest first. It was stopped before the first.
    told: VecDeque<(Instant, f64)>,
    aim: ZoomAim,
}

impl ZoomGlide {
    fn new(now: Instant) -> ZoomGlide {
        ZoomGlide {
            running: 0.0,
            sent_at: now,
            stop_at: None,
            stopped_at: None,
            nudging: false,
            told: VecDeque::new(),
            aim: ZoomAim::new(now),
        }
    }

    /// The web interface's number for a speed.
    fn value_for(rate: f64) -> u16 {
        let rate = rate.abs();
        for pair in WEB_ZOOM.windows(2) {
            let ((v0, r0), (v1, r1)) = (pair[0], pair[1]);
            if rate <= r1 {
                let share = ((rate - r0) / (r1 - r0)).clamp(0.0, 1.0);
                return (v0 as f64 + (v1 - v0) as f64 * share).round() as u16;
            }
        }
        WEB_ZOOM[WEB_ZOOM.len() - 1].0
    }

    /// The request that sets the lens running at `rate`, or stops it.
    fn request(rate: f64) -> String {
        if rate == 0.0 {
            "/command/ptzf.cgi?ZoomMove=stop,0".to_string()
        } else {
            format!("/command/ptzf.cgi?ZoomMove={},{}", if rate > 0.0 { "tele" } else { "wide" }, Self::value_for(rate))
        }
    }

    /// How far the lens was told to go between `from` and `to` seconds before `now`.
    fn told_between(&self, now: Instant, from: f64, to: f64) -> f64 {
        let mut total = 0.0;
        for (i, (at, rate)) in self.told.iter().enumerate() {
            // Seconds before now at which this speed began and ended.
            let began = now.saturating_duration_since(*at).as_secs_f64();
            let ended = self.told.get(i + 1).map(|(next, _)| now.saturating_duration_since(*next).as_secs_f64()).unwrap_or(0.0);
            let (first, last) = (began.min(from), ended.max(to));
            if first > last {
                total += rate * (first - last);
            }
        }
        total
    }

    /// The lens stopped by itself at the moment that was fixed for it.
    fn settle(&mut self, now: Instant) {
        if let Some(at) = self.stop_at.filter(|at| now >= *at) {
            self.told.push_back((at, 0.0));
            (self.running, self.stopped_at, self.stop_at) = (0.0, Some(at), None);
            self.nudging = false;
        }
    }

    /// What to tell the lens now, when it is not what it has been told already. `zoom` is the
    /// last reading and `read_at` when it was taken; `planned_top` is the fastest zoom the timeline
    /// is allowed to ask for.
    #[allow(clippy::too_many_arguments)]
    fn decide(
        &mut self,
        now: Instant,
        target: f64,
        velocity: f64,
        zoom: f64,
        read_at: Instant,
        scale: f64,
        planned_top: f64,
    ) -> Option<ZoomOrder> {
        self.settle(now);
        let (slowest, fastest) = (WEB_ZOOM[0].1, WEB_ZOOM[WEB_ZOOM.len() - 1].1);
        let top = (fastest * scale.clamp(0.0, 1.0)).max(slowest);
        // The lens does at any moment what it was told a little earlier, so since the reading
        // it has gone what it was told over a stretch that long ago.
        let age = now.saturating_duration_since(read_at).as_secs_f64().min(ZOOM_READING_AGE);
        let now_at = zoom + self.told_between(now, age + ZOOM_LAG, ZOOM_LAG);
        let wanted = self.aim.wanted(now, target, velocity, zoom, now_at, read_at, planned_top, top);
        let size = wanted.abs();
        let still = velocity.abs() < 0.005;

        if self.running == 0.0 {
            if still {
                // A reading taken while the lens was still coasting says nothing about where it rests.
                let coasting = self.stopped_at.is_some_and(|at| read_at < at + Duration::from_secs_f64(ZOOM_SETTLE));
                let off = target - zoom;
                if coasting || off.abs() < ZOOM_REST {
                    return None;
                }
                if off.abs() <= GLIDE_NUDGE {
                    // Close by: its slowest speed for exactly as long as the distance takes.
                    let lasts = Duration::from_secs_f64(off.abs() / slowest);
                    self.nudging = true;
                    return Some(ZoomOrder { rate: slowest.copysign(off), stop_at: Some(now + lasts) });
                }
            }
            // A lens at rest is started for a move as soon as the move asks for its slowest speed.
            let start = if still { slowest * 0.3 } else { slowest * 0.9 };
            return (size >= start).then(|| ZoomOrder { rate: size.max(slowest).copysign(wanted), stop_at: None });
        }

        if self.nudging {
            // The run was measured from a reading of the lens at rest. Readings taken on the way
            // are coarser than the distance, so they are not allowed to change it.
            if still {
                return None;
            }
            self.nudging = false;
        }
        let way = self.running.signum();
        // The lens has got ahead of the target, or the target wants it the other way. It never
        // reverses without stopping first.
        let ahead = wanted * way <= 0.0 || size < slowest * 0.15;
        let mut rate = self.running;
        if !ahead {
            let next = size.max(slowest).copysign(wanted);
            let far = (next - self.running).abs() > GLIDE_STEP * self.running.abs();
            if far && now.saturating_duration_since(self.sent_at) >= GLIDE_EVERY {
                rate = next;
            }
        }
        // Where the target comes to rest, when that can be told: it is there already, or it is
        // slowing down steadily. The lens cannot slow down below its slowest speed, so instead
        // it is stopped at the moment that lets it coast onto that mark.
        let accel = self.aim.accel;
        let rests_at = if still {
            Some(target)
        } else if velocity * way > 0.0 && velocity * accel < 0.0 && accel.abs() > 1e-4 {
            Some(target + velocity * velocity.abs() / (2.0 * accel.abs()))
        } else {
            None
        };
        match rests_at {
            Some(mark) => {
                // Stopped now, it still goes what it has been told over the last moment.
                let left = (mark - now_at - self.told_between(now, ZOOM_LAG, 0.0)) * way;
                let lasts = left / rate.abs();
                if lasts <= GLIDE_HORIZON || ahead {
                    let stop_at = Some(now + Duration::from_secs_f64(lasts.max(0.0)));
                    return Some(ZoomOrder { rate, stop_at });
                }
            }
            None if ahead => return Some(ZoomOrder { rate: 0.0, stop_at: None }),
            None => {}
        }
        // Carrying on: a new speed, or a stop that was fixed and is no longer wanted.
        (rate != self.running || self.stop_at.is_some()).then_some(ZoomOrder { rate, stop_at: None })
    }

    fn took(&mut self, order: ZoomOrder, now: Instant) {
        if order.rate != self.running {
            if order.rate == 0.0 {
                self.stopped_at = Some(now);
            }
            self.sent_at = now;
            self.told.push_back((now, order.rate));
            while self.told.len() > 2
                && self.told.get(1).is_some_and(|(at, _)| now.saturating_duration_since(*at).as_secs_f64() > 4.0)
            {
                self.told.pop_front();
            }
        }
        self.running = order.rate;
        self.stop_at = order.stop_at;
    }
}

/// The lens is stopped when the worker has said nothing about zoom for this long, so a worker
/// that is held up or has lost the camera cannot leave it running.
const ZOOM_UNATTENDED: Duration = Duration::from_millis(1500);

/// Sends zoom speeds to the camera's web interface from a thread of its own, so the request
/// never holds up pan and tilt and a stop goes out at the moment fixed for it. Only the newest
/// speed matters: one that has not gone out yet is replaced by the next.
struct ZoomWeb {
    shared: Arc<(Mutex<ZoomWebState>, Condvar)>,
}

struct ZoomWebState {
    /// A speed that has not gone out yet.
    rate: Option<f64>,
    stop_at: Option<Instant>,
    /// When the worker last said what it wants.
    heard_at: Instant,
    /// The lens was stopped because the worker went quiet.
    lapsed: bool,
    failed: bool,
    quit: bool,
}

impl ZoomWeb {
    fn start(cfg: &Fr7Config) -> ZoomWeb {
        let state =
            ZoomWebState { rate: None, stop_at: None, heard_at: Instant::now(), lapsed: false, failed: false, quit: false };
        let shared = Arc::new((Mutex::new(state), Condvar::new()));
        let (theirs, mut cgi) = (shared.clone(), Cgi::new(&cfg.http, &cfg.user, &cfg.password, ZOOM_WEB_WAIT));
        let spawned = std::thread::Builder::new().name("fr7-zoom".into()).spawn(move || {
            let (state, wake) = &*theirs;
            let mut running = false;
            loop {
                let rate = {
                    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                    loop {
                        if let Some(rate) = s.rate.take() {
                            break rate;
                        }
                        if s.quit {
                            return;
                        }
                        if !running {
                            s = wake.wait(s).unwrap_or_else(|e| e.into_inner());
                            continue;
                        }
                        let (now, unattended) = (Instant::now(), s.heard_at + ZOOM_UNATTENDED);
                        let due = s.stop_at.map_or(unattended, |at| at.min(unattended));
                        if now >= due {
                            s.lapsed = s.stop_at.is_none_or(|at| now < at);
                            s.stop_at = None;
                            break 0.0;
                        }
                        s = wake.wait_timeout(s, due - now).unwrap_or_else(|e| e.into_inner()).0;
                    }
                };
                // A connection the camera has closed fails once and works when opened again.
                let request = ZoomGlide::request(rate);
                if cgi.get(&request).is_err() && cgi.get(&request).is_err() {
                    state.lock().unwrap_or_else(|e| e.into_inner()).failed = true;
                }
                running = rate != 0.0;
            }
        });
        if spawned.is_err() {
            shared.0.lock().unwrap_or_else(|e| e.into_inner()).failed = true;
        }
        ZoomWeb { shared }
    }

    /// Tell the lens something new.
    fn order(&self, order: ZoomOrder, changed: bool) {
        let mut s = self.shared.0.lock().unwrap_or_else(|e| e.into_inner());
        if changed {
            s.rate = Some(order.rate);
        }
        s.stop_at = order.stop_at;
        s.heard_at = Instant::now();
        drop(s);
        self.shared.1.notify_one();
    }

    /// Nothing new: the lens carries on as it was told.
    fn carry_on(&self) {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).heard_at = Instant::now();
    }

    /// Whether a speed could not be delivered.
    fn failed(&self) -> bool {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).failed
    }

    /// Whether the lens was stopped because the worker went quiet, once.
    fn lapsed(&self) -> bool {
        std::mem::take(&mut self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).lapsed)
    }
}

impl Drop for ZoomWeb {
    fn drop(&mut self) {
        self.shared.0.lock().unwrap_or_else(|e| e.into_inner()).quit = true;
        self.shared.1.notify_one();
    }
}

pub fn focus_counts(focus: f64) -> u16 {
    (focus.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// The camera's own settings that hold it below its full speed, the value each is given, and its
/// name in words. Every speed this program asks for is scaled by them, so a camera left low by
/// whoever used it last runs slower than the timeline expects. The last is how many speed steps
/// the camera counts: set to 50 ("extended"), the 24 this program sends are the slow half of them
/// and the head tops out near a fifth of its speed.
const FULL_SPEED: [(&str, &str, &str); 4] = [
    ("PanTiltSpeedScale", "100", "pan and tilt speed scale"),
    ("ZoomSpeedScale", "100", "zoom speed scale"),
    ("PanTiltSpeedMode", "normal", "pan and tilt speed mode"),
    ("PanTiltSpeedStep", "normal", "pan and tilt speed steps"),
];

/// What became of the settings that held a camera below its full speed.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SpeedSetup {
    /// Settings that were raised, in words.
    pub raised: Vec<String>,
    /// Settings the camera would not take, and why.
    pub refused: Vec<String>,
}

impl SpeedSetup {
    /// What to tell the operator, if anything was low.
    pub fn note(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.raised.is_empty() {
            parts.push(format!("Raised to full speed: {}", self.raised.join(", ")));
        }
        if !self.refused.is_empty() {
            parts.push(format!("Left below full speed: {}", self.refused.join(", ")));
        }
        (!parts.is_empty()).then(|| parts.join(". "))
    }
}

/// Give the camera its full speed, going by `fields` of `command/inquiry.cgi?inq=ptzf`.
/// A setting the camera does not name is left alone.
pub fn full_speed(cgi: &mut Cgi, fields: &[(String, String)]) -> SpeedSetup {
    let mut setup = SpeedSetup::default();
    for (name, full, words) in FULL_SPEED {
        let Some(now) = cgi::field(fields, name) else { continue };
        let low = match (now.parse::<f64>(), full.parse::<f64>()) {
            (Ok(now), Ok(full)) => now < full,
            _ => !now.eq_ignore_ascii_case(full),
        };
        if !low {
            continue;
        }
        match cgi.get(&format!("/command/ptzf.cgi?{name}={full}")) {
            Ok(_) => setup.raised.push(format!("{words} from {now} to {full}")),
            Err(why) => setup.refused.push(format!("{words} stays at {now} ({why})")),
        }
    }
    setup
}

/// Preset numbers and names from `command/inquiry.cgi?inq=presetposition`.
pub fn preset_names(cgi: &mut Cgi) -> Result<Vec<(u32, String)>, String> {
    let text = cgi.get_text("/command/inquiry.cgi?inq=presetposition")?;
    let fields = cgi::fields(&text);
    let Some(list) = cgi::field(&fields, "PresetName") else {
        return Err("the FR7's preset list was not understood".into());
    };
    let parts: Vec<&str> = list.split(',').collect();
    Ok(parts
        .chunks(2)
        .filter_map(|pair| match pair {
            [number, name] => Some((number.trim().parse::<u32>().ok()?, name.trim().to_string())),
            _ => None,
        })
        .collect())
}

pub fn preset_picture(cgi: &mut Cgi, number: u32) -> Option<Vec<u8>> {
    cgi.get(&format!("/preset/presetimg{number}.jpg")).ok().filter(|b| b.starts_with(&[0xFF, 0xD8]))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Idle,
    Drive(Drive),
    Stop,
    Sync,
}

struct State {
    head: Head,
    read_at: ReadTimes,
    connected: bool,
    /// A preset move can leave the link connected while the camera refuses position inquiries.
    last_position_refusal: Option<Instant>,
    detail: String,
    round_trip_ms: f64,
    moving: bool,
    mode: Mode,
    told_at: Instant,
    /// The engine stopped sending targets while live, so the worker stopped the head.
    starved: bool,
    axes: AxisTable,
    sync_progress: Option<String>,
    sync_result: Option<Result<Vec<DevicePreset>, String>>,
    quit: bool,
}

pub struct Fr7 {
    cfg: Fr7Config,
    state: Arc<Mutex<State>>,
}

/// How long a live command stays good. The engine repeats it sixty times a second.
const STARVED_AFTER: Duration = Duration::from_millis(300);
/// Share of the position error closed per second.
const GAIN: f64 = 4.0;

/// The FR7 refuses queries and some commands with this while a preset move is under way.
fn busy(err: &str) -> bool {
    err.contains("current mode")
}

/// The FR7 runs two commands at a time. With two moves under way it refuses everything else with this.
fn crowded(err: &str) -> bool {
    err.contains("buffer is full")
}
/// Refusals for a full buffer that go on this long are no longer a move finishing.
const CROWDED_TOO_LONG: Duration = Duration::from_secs(2);
/// How long a zoom speed sent through the web interface may take to be answered. It usually takes 12 ms.
const ZOOM_WEB_WAIT: Duration = Duration::from_millis(500);
/// A focus move is over by this long after it was sent, whatever the camera says. They take about 0.2 s.
const FOCUS_TAKES: Duration = Duration::from_secs(1);
/// How long after the lens refused a focus position it is offered the next.
const FOCUS_REFUSED: Duration = Duration::from_millis(300);
/// Focus this far from where it was sent is sent there again,
const FOCUS_DRIFT: f64 = 0.01;
/// but no more often than this.
const FOCUS_AGAIN: Duration = Duration::from_millis(500);
/// A stopped head has to hold still for this many readings before it counts as stopped.
const STILL_READINGS: u32 = 2;

impl Fr7 {
    pub fn start(cfg: Fr7Config, axes: AxisTable) -> Fr7 {
        let now = Instant::now();
        let state = Arc::new(Mutex::new(State {
            head: Head::default(),
            read_at: ReadTimes { pan_tilt: now, zoom: now, focus: now },
            connected: false,
            last_position_refusal: None,
            detail: format!("Connecting to {}", cfg.visca),
            round_trip_ms: 0.0,
            moving: false,
            mode: Mode::Idle,
            told_at: now,
            starved: false,
            axes,
            sync_progress: None,
            sync_result: None,
            quit: false,
        }));
        let (shared, worker_cfg) = (state.clone(), cfg.clone());
        std::thread::Builder::new().name("fr7".into()).spawn(move || Worker::new(worker_cfg, shared).run()).expect("fr7 thread");
        Fr7 { cfg, state }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> AdapterStatus {
        let s = self.lock();
        AdapterStatus {
            name: "FR7".into(),
            connected: s.connected,
            detail: s.detail.clone(),
            calibrated: None,
            round_trip_ms: s.connected.then_some(s.round_trip_ms),
            raw_position: None,
        }
    }

    pub fn problem(&self) -> Option<String> {
        let s = self.lock();
        if !s.connected {
            Some(format!("The FR7 is not answering at {}", self.cfg.visca))
        } else if s.starved {
            Some("The FR7 was stopped because the show stopped sending it targets".into())
        } else {
            None
        }
    }

    pub fn head(&self) -> (Head, ReadTimes) {
        let s = self.lock();
        (s.head, s.read_at)
    }

    pub(super) fn capture_head(&self) -> Result<Head, String> {
        let s = self.lock();
        if !s.connected {
            return Err(format!("The FR7 is not answering at {}", self.cfg.visca));
        }
        let oldest = s.read_at.pan_tilt.min(s.read_at.zoom).min(s.read_at.focus);
        if s.last_position_refusal.is_some_and(|at| oldest <= at) || oldest.elapsed() > super::MAX_CAPTURE_AGE {
            return Err("Waiting for the FR7 to report its current position and zoom".into());
        }
        Ok(s.head)
    }

    pub fn round_trip_s(&self) -> f64 {
        self.lock().round_trip_ms / 1000.0
    }

    pub fn is_moving(&self) -> bool {
        let s = self.lock();
        s.moving || s.mode == Mode::Stop
    }

    pub fn set_axes(&self, axes: &AxisTable) {
        self.lock().axes = *axes;
    }

    pub fn drive(&self, cmd: &Drive) {
        let mut s = self.lock();
        // A preset sync owns the head until it ends or is stopped.
        if s.mode != Mode::Sync {
            s.mode = Mode::Drive(*cmd);
            s.told_at = Instant::now();
            s.starved = false;
        }
    }

    pub fn stop(&self) {
        let mut s = self.lock();
        if s.mode == Mode::Sync {
            s.sync_result = Some(Err("The preset sync was stopped".into()));
            s.sync_progress = None;
        }
        s.mode = Mode::Stop;
    }

    pub fn begin_sync(&self) -> Result<(), String> {
        let mut s = self.lock();
        if !s.connected {
            return Err(format!("The FR7 is not answering at {}", self.cfg.visca));
        }
        if s.mode == Mode::Sync {
            return Err("A preset sync is already running".into());
        }
        s.mode = Mode::Sync;
        s.sync_result = None;
        s.sync_progress = Some("Reading the preset list".into());
        Ok(())
    }

    pub fn take_sync(&self) -> Option<Result<Vec<DevicePreset>, String>> {
        let mut s = self.lock();
        let result = s.sync_result.take()?;
        s.sync_progress = None;
        Some(result)
    }

    pub fn sync_progress(&self) -> Option<String> {
        self.lock().sync_progress.clone()
    }
}

impl Drop for Fr7 {
    fn drop(&mut self) {
        self.lock().quit = true;
    }
}

/// Recalling presets one at a time to learn their poses.
struct SyncJob {
    list: Vec<(u32, String)>,
    index: usize,
    recalled_at: Option<Instant>,
    /// When a recall was first refused because a move was already running.
    recall_refused: Option<Instant>,
    still: u32,
    out: Vec<DevicePreset>,
}

struct Worker {
    cfg: Fr7Config,
    state: Arc<Mutex<State>>,
    link: Option<Link>,
    cgi: Cgi,
    carry: [f64; 2],
    cycle: u64,
    last_sent: Option<Vec<u8>>,
    sent_at_cycle: u64,
    zoom_sent: Option<u16>,
    /// This lens refused a direct zoom position, so zoom is driven by speed instead.
    zoom_by_speed: bool,
    /// Zoom speed goes through the web interface, which takes any speed. VISCA has seven.
    zoom_web: bool,
    /// Started the first time a lens turns out to need it.
    zoom_sender: Option<ZoomWeb>,
    zoom_glide: ZoomGlide,
    zoom_ramp: ZoomRamp,
    focus_sent: Option<u16>,
    focus_sent_at: Instant,
    /// When the lens last refused a focus position because it was zooming.
    focus_refused_at: Option<Instant>,
    /// The zoom speeds of the last three commands. See `steady_speed`.
    zoom_speeds: [f64; 3],
    focus_is_manual: bool,
    /// The last zoom or focus position command, which holds a place in the camera until the lens arrives.
    lens_move: Option<u32>,
    /// The last lens position sent was a zoom, so focus goes first next time.
    zoom_went_last: bool,
    crowded_since: Option<Instant>,
    /// The camera's own pan or tilt direction is Opposite, so VISCA Plus moves that axis backwards.
    pan_opposite: bool,
    tilt_opposite: bool,
    /// What was done about the camera's speed settings when it was last connected to, if anything.
    speed_note: Option<String>,
    ride: Option<Ride>,
    stops_sent: u32,
    still: u32,
    sync: Option<SyncJob>,
    was: Mode,
}

impl Worker {
    fn new(cfg: Fr7Config, state: Arc<Mutex<State>>) -> Worker {
        let cgi = Cgi::new(&cfg.http, &cfg.user, &cfg.password, Duration::from_secs(3));
        Worker {
            cfg,
            state,
            link: None,
            cgi,
            carry: [0.0; 2],
            cycle: 0,
            last_sent: None,
            sent_at_cycle: 0,
            zoom_sent: None,
            zoom_by_speed: false,
            zoom_web: true,
            zoom_sender: None,
            zoom_glide: ZoomGlide::new(Instant::now()),
            zoom_ramp: ZoomRamp::new(Instant::now()),
            focus_sent: None,
            focus_sent_at: Instant::now(),
            focus_refused_at: None,
            zoom_speeds: [0.0; 3],
            focus_is_manual: false,
            lens_move: None,
            zoom_went_last: false,
            crowded_since: None,
            pan_opposite: false,
            tilt_opposite: false,
            speed_note: None,
            ride: None,
            stops_sent: 0,
            still: 0,
            sync: None,
            was: Mode::Idle,
        }
    }

    /// Pan and tilt direction as the camera itself is set. Opposite means VISCA Plus
    /// moves that axis against the angle the position query reports. Driven the wrong way an axis
    /// runs away from its target, so a camera that does not say is not driven at all.
    /// The same answer carries the camera's speed settings, which are raised to full speed here.
    fn read_camera_setup(&mut self) -> Result<(), String> {
        let mut why = String::new();
        for _ in 0..3 {
            match self.cgi.get_text("/command/inquiry.cgi?inq=ptzf") {
                Ok(text) => {
                    let fields = cgi::fields(&text);
                    if let (Some(pan), Some(tilt)) = (cgi::field(&fields, "PanDirection"), cgi::field(&fields, "TiltDirection")) {
                        self.pan_opposite = pan.eq_ignore_ascii_case("opposite");
                        self.tilt_opposite = tilt.eq_ignore_ascii_case("opposite");
                        self.speed_note = full_speed(&mut self.cgi, &fields).note();
                        return Ok(());
                    }
                    why = "its answer names no pan or tilt direction".into();
                }
                Err(err) => why = err,
            }
        }
        Err(format!("the FR7's web interface at {} did not say which way pan and tilt run: {why}", self.cfg.http))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn run(mut self) {
        let mut pace = Loop::new(PERIOD);
        loop {
            if self.lock().quit {
                if let Some(link) = &mut self.link {
                    let _ = link.stop_all();
                }
                self.stop_web_zoom();
                return;
            }
            let outcome = self.cycle();
            // A full buffer empties when a move ends. The command it refused is sent again next cycle.
            let waiting = match &outcome {
                Err(why) if crowded(why) => self.crowded_since.get_or_insert_with(Instant::now).elapsed() < CROWDED_TOO_LONG,
                _ => false,
            };
            if !waiting {
                self.crowded_since = None;
            }
            if let Err(why) = outcome {
                // A preset move refuses questions until it arrives. That is not the camera going away,
                // and stopping here is what cuts the move off.
                if !busy(&why) && !waiting {
                    // Nothing can be steered without the link, so nothing is left running.
                    self.stop_web_zoom();
                    self.link = None;
                    let mut s = self.lock();
                    s.connected = false;
                    s.moving = false;
                    s.detail = why;
                    if s.mode == Mode::Sync {
                        s.sync_result = Some(Err("The FR7 stopped answering during the preset sync".into()));
                        s.sync_progress = None;
                        s.mode = Mode::Idle;
                    }
                    drop(s);
                    self.sync = None;
                    // Do not hammer an address nothing answers at.
                    std::thread::sleep(Duration::from_millis(250));
                }
            }
            pace.wait();
        }
    }

    fn cycle(&mut self) -> Result<(), String> {
        let fresh = self.link.is_none();
        if fresh {
            self.link = Some(Link::connect(&self.cfg)?);
            // A real FR7 often answers in 30 to 150 ms. 120 ms treats a slow answer as silence.
            if let Some(link) = &mut self.link {
                link.timeout = Duration::from_millis(400);
            }
            if let Err(why) = self.read_camera_setup() {
                self.link = None;
                return Err(why);
            }
            self.last_sent = None;
            self.zoom_sent = None;
            // A power-zoom lens can be sent to a position, but it goes there in bursts at its own
            // speed. Where the web interface takes a zoom speed, the lens is steered by speed instead.
            self.zoom_by_speed = self.cgi.get(&ZoomGlide::request(0.0)).is_ok();
            self.zoom_web = true;
            self.zoom_sender = None;
            self.zoom_glide = ZoomGlide::new(Instant::now());
            self.zoom_ramp = ZoomRamp::new(Instant::now());
            self.focus_sent = None;
            self.focus_is_manual = false;
            self.lens_move = None;
        }
        self.cycle += 1;
        let cycle = self.cycle;
        let link = self.link.as_mut().expect("connected above");
        if fresh {
            // Nothing is reported as connected until every axis has been read once.
            let head = link.read_head()?;
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.head = head;
            let now = Instant::now();
            s.read_at = ReadTimes { pan_tilt: now, zoom: now, focus: now };
            // A server that died while live leaves the head on its last speed command.
            if s.mode == Mode::Idle {
                s.mode = Mode::Stop;
            }
        }
        let before = self.state.lock().unwrap_or_else(|e| e.into_inner()).head;

        // While a preset is running the camera refuses position questions. Keep the last
        // reading and do not treat the refusal as the camera having gone away.
        let mut quiet = false;
        let (pan, tilt) = match link.read_pan_tilt() {
            Ok(pose) => pose,
            Err(why) if busy(&why) => {
                quiet = true;
                (before.pan, before.tilt)
            }
            Err(why) => return Err(why),
        };
        let mut read_at = self.state.lock().unwrap_or_else(|e| e.into_inner()).read_at;
        if !quiet {
            read_at.pan_tilt = Instant::now() - link.last_round_trip / 2;
        }
        let round_trip_ms = link.last_round_trip.as_secs_f64() * 1000.0;
        // Zoom and focus change slowly; they share the cycles between them. A lens being run by
        // speed is steered by its zoom reading, so that one is taken three times in four.
        let zooming = self.zoom_glide.running != 0.0 || self.zoom_ramp.running.is_some();
        let mut head = Head { pan, tilt, ..before };
        if if zooming { !cycle.is_multiple_of(4) } else { cycle.is_multiple_of(2) } {
            match link.read_zoom() {
                Ok(zoom) => {
                    head.zoom = zoom;
                    read_at.zoom = Instant::now() - link.last_round_trip / 2;
                }
                Err(why) if busy(&why) => quiet = true,
                Err(why) => return Err(why),
            }
        } else {
            match link.read_focus() {
                Ok(focus) => {
                    head.focus = focus;
                    read_at.focus = Instant::now() - link.last_round_trip / 2;
                }
                Err(why) if busy(&why) => quiet = true,
                Err(why) => return Err(why),
            }
        }
        let cpd = self.cfg.counts_per_degree;
        let moved = (head.pan - before.pan).abs() > 1.5 / cpd
            || (head.tilt - before.tilt).abs() > 1.5 / cpd
            || (head.zoom - before.zoom).abs() > 2.0 / visca::ZOOM_TELE as f64
            || (head.focus - before.focus).abs() > 8.0 / 65535.0;

        let (mode, axes) = {
            let mut s = self.lock();
            s.head = head;
            s.read_at = read_at;
            s.connected = true;
            if quiet {
                s.last_position_refusal = Some(Instant::now());
            }
            s.round_trip_ms = round_trip_ms;
            s.moving = moved || quiet;
            s.detail = if quiet {
                format!("{} is moving and will answer when it arrives", self.cfg.visca)
            } else {
                match &self.speed_note {
                    Some(note) => format!("{} answers in {:.1} ms. {note}", self.cfg.visca, round_trip_ms),
                    None => format!("{} answers in {:.1} ms", self.cfg.visca, round_trip_ms),
                }
            };
            if matches!(s.mode, Mode::Drive(_)) && s.told_at.elapsed() > STARVED_AFTER {
                s.mode = Mode::Stop;
                s.starved = true;
            }
            (s.mode, s.axes)
        };
        if mode != self.was {
            if mode == Mode::Stop {
                self.stops_sent = 0;
                self.still = 0;
            }
            if !matches!(mode, Mode::Drive(_)) {
                self.ride = None;
                self.last_sent = None;
                // Recalling a preset can put the camera back in autofocus, where it refuses every
                // focus position. Manual focus is asked for again whenever driving starts afresh.
                self.focus_is_manual = false;
                self.focus_sent = None;
            }
            self.was = mode;
        }

        match mode {
            Mode::Idle => {}
            Mode::Drive(cmd) => self.follow(&cmd, &head, &axes)?,
            Mode::Stop => {
                // Stop is repeated until the head reads the same twice running.
                let stopped = self.link.as_mut().expect("connected").stop_all();
                self.stop_web_zoom();
                stopped?;
                self.stops_sent += 1;
                self.still = if moved { 0 } else { self.still + 1 };
                self.zoom_sent = None;
                // Stop has stopped the lens too.
                self.zoom_ramp = ZoomRamp::new(Instant::now());
                self.focus_sent = None;
                if self.stops_sent >= 2 && self.still >= STILL_READINGS {
                    let mut s = self.lock();
                    if s.mode == Mode::Stop {
                        s.mode = Mode::Idle;
                    }
                }
                self.sync = None;
            }
            Mode::Sync => self.sync_step(&head, moved, !quiet)?,
        }
        Ok(())
    }

    /// Steer toward the commanded pose: its own speed plus a share of the error.
    fn follow(&mut self, cmd: &Drive, head: &Head, axes: &AxisTable) -> Result<(), String> {
        let (pan_sign, tilt_sign) = (self.cfg.pan_sign, self.cfg.tilt_sign);
        let (pan_opposite, tilt_opposite) = (self.pan_opposite, self.tilt_opposite);
        let link = self.link.as_mut().expect("connected");
        if !self.focus_is_manual {
            link.command(&visca::focus_manual())?;
            self.focus_is_manual = true;
        }
        let ride = cmd.ride.filter(|_| self.cfg.absolute_moves);
        if let Some(ride) = ride {
            // One absolute command per move; the head runs its own ramp to the end pose.
            if self.ride.is_none_or(|r| r.pan != ride.pan || r.tilt != ride.tilt) {
                link.go_to((head.pan, head.tilt), (ride.pan, ride.tilt), ride.seconds)?;
                self.ride = Some(ride);
                self.last_sent = None;
            }
        } else {
            self.ride = None;
            let scale = cmd.scale.clamp(0.0, 1.0);
            let table = link.speed_table().to_vec();
            let want = |target: f64, feed: f64, at: f64, top: f64, carry: &mut f64| {
                let speed = (feed + GAIN * (target - at)).clamp(-top * scale, top * scale);
                visca::step_for(&table, speed, carry)
            };
            let [pan_carry, tilt_carry] = &mut self.carry;
            let pan =
                directed(want(cmd.target.pan, cmd.velocity.pan, head.pan, axes.pan.max_speed, pan_carry), pan_sign, pan_opposite);
            let tilt = directed(
                want(cmd.target.tilt, cmd.velocity.tilt, head.tilt, axes.tilt.max_speed, tilt_carry),
                tilt_sign,
                tilt_opposite,
            );
            let bytes = visca::pan_tilt_drive(pan.0, tilt.0, pan.1, tilt.1);
            // The same command is not repeated every cycle, but it is refreshed twice a second.
            if self.last_sent.as_ref() != Some(&bytes) || self.cycle - self.sent_at_cycle >= 10 {
                link.command(&bytes)?;
                self.last_sent = Some(bytes);
                self.sent_at_cycle = self.cycle;
            }
        }
        // A zoom or focus position command holds one of the camera's two places until the lens
        // arrives. A second one behind it takes the other, and then pan, tilt and stop are refused.
        // So the next lens position waits for the last, and goes to wherever the target is by then.
        // Zoom and focus take turns, or a zoom that never rests would keep focus waiting.
        if self.zoom_went_last {
            self.steer_focus(cmd, head)?;
            self.steer_zoom(cmd, head, axes)
        } else {
            self.steer_zoom(cmd, head, axes)?;
            self.steer_focus(cmd, head)
        }
    }

    fn lens_is_free(&self) -> bool {
        match (&self.link, self.lens_move) {
            (Some(link), Some(command)) => !link.running(command),
            _ => true,
        }
    }

    fn steer_zoom(&mut self, cmd: &Drive, head: &Head, axes: &AxisTable) -> Result<(), String> {
        let free = self.lens_is_free();
        let read_at = self.lock().read_at.zoom;
        let velocity = steady_speed(&mut self.zoom_speeds, cmd.velocity.zoom);
        let link = self.link.as_mut().expect("connected");
        if self.zoom_by_speed && self.zoom_web {
            let now = Instant::now();
            let sender = self.zoom_sender.get_or_insert_with(|| ZoomWeb::start(&self.cfg));
            if sender.failed() {
                // Without the web interface the lens is still reachable through VISCA, in seven speeds.
                self.zoom_web = false;
                self.zoom_sender = None;
                self.zoom_glide = ZoomGlide::new(now);
                self.zoom_ramp = ZoomRamp::new(now);
                return link.command(&visca::zoom_stop());
            }
            if sender.lapsed() {
                self.zoom_glide = ZoomGlide::new(now);
            }
            let next = self.zoom_glide.decide(now, cmd.target.zoom, velocity, head.zoom, read_at, cmd.scale, axes.zoom.max_speed);
            // A lens still on its way to a focus position does not start to zoom: it takes the
            // speed and stays where it is. The start waits until the focus move is over.
            let held = self.zoom_glide.running == 0.0 && !free && self.focus_sent_at.elapsed() < FOCUS_TAKES;
            let next = next.filter(|order| !(held && order.rate != 0.0));
            match next {
                Some(order) => {
                    sender.order(order, order.rate != self.zoom_glide.running);
                    self.zoom_glide.took(order, now);
                }
                None => sender.carry_on(),
            }
            return Ok(());
        }
        if self.zoom_by_speed {
            let now = Instant::now();
            let next = self.zoom_ramp.decide(now, cmd.target.zoom, velocity, head.zoom, read_at, cmd.scale, axes.zoom.max_speed);
            let held = self.zoom_ramp.running.is_none() && !free && self.focus_sent_at.elapsed() < FOCUS_TAKES;
            if next != self.zoom_ramp.running && !held {
                link.command(&ZoomRamp::command(next))?;
                self.zoom_ramp.took(next, Instant::now());
            }
            return Ok(());
        }
        let zoom = zoom_counts(cmd.target.zoom);
        if self.zoom_sent != Some(zoom) && free {
            match link.begin(&visca::zoom_direct(zoom)) {
                Ok(command) => {
                    self.lens_move = Some(command);
                    self.zoom_went_last = true;
                    self.zoom_sent = Some(zoom);
                }
                // A power-zoom lens reports its position but refuses to be sent to one.
                // From the next cycle on it is driven by speed.
                Err(why) if why.contains("current mode") => {
                    self.zoom_by_speed = true;
                    self.zoom_glide = ZoomGlide::new(Instant::now());
                    self.zoom_ramp = ZoomRamp::new(Instant::now());
                }
                Err(why) => return Err(why),
            }
        }
        Ok(())
    }

    fn steer_focus(&mut self, cmd: &Drive, head: &Head) -> Result<(), String> {
        let free = self.lens_is_free();
        let link = self.link.as_mut().expect("connected");
        let focus = focus_counts(cmd.target.focus);
        let zoom_rests = self.zoom_glide.running == 0.0 && self.zoom_ramp.running.is_none() && cmd.velocity.zoom.abs() < 0.005;
        if self.zoom_by_speed {
            // A lens driven by speed does one thing at a time. While it zooms it refuses a focus
            // position, and one it is still moving to keeps it from starting to zoom. So focus
            // waits for the zoom: nothing is sent while the timeline zooms or the lens runs, and
            // once the lens has come to rest it is sent the focus the timeline has reached by then.
            let settled = self.zoom_glide.stopped_at.is_none_or(|at| at.elapsed() >= Duration::from_secs_f64(ZOOM_SETTLE));
            let refused = self.focus_refused_at.is_some_and(|at| at.elapsed() < FOCUS_REFUSED);
            if !zoom_rests || !settled || refused {
                return Ok(());
            }
        }
        // A zoom lens shifts its focus as it zooms, so the focus it was sent to does not stay put.
        // It is put back once the zoom has come to rest.
        let drifted =
            zoom_rests && (head.focus - cmd.target.focus).abs() > FOCUS_DRIFT && self.focus_sent_at.elapsed() >= FOCUS_AGAIN;
        if (self.focus_sent != Some(focus) || drifted) && free {
            match link.begin(&visca::focus_direct(focus)) {
                Ok(command) => {
                    self.lens_move = Some(command);
                    self.zoom_went_last = false;
                    self.focus_sent = Some(focus);
                    self.focus_sent_at = Instant::now();
                    self.focus_refused_at = None;
                }
                // The lens had not quite finished zooming. It is asked again in a moment.
                Err(why) if self.zoom_by_speed && busy(&why) => self.focus_refused_at = Some(Instant::now()),
                Err(why) => return Err(why),
            }
        }
        Ok(())
    }

    /// Stop a zoom the web interface started. The VISCA stop halts it as well; this makes sure.
    fn stop_web_zoom(&mut self) {
        let now = Instant::now();
        self.zoom_glide.settle(now);
        if let Some(sender) = self.zoom_sender.as_ref().filter(|_| self.zoom_glide.running != 0.0) {
            sender.order(ZoomOrder { rate: 0.0, stop_at: None }, true);
        }
        self.zoom_glide = ZoomGlide::new(now);
    }

    fn sync_step(&mut self, head: &Head, moved: bool, answering: bool) -> Result<(), String> {
        if self.sync.is_none() {
            let list = match preset_names(&mut self.cgi) {
                Ok(list) => list,
                Err(why) => {
                    self.finish_sync(Err(why));
                    return Ok(());
                }
            };
            self.sync = Some(SyncJob { list, index: 0, recalled_at: None, recall_refused: None, still: 0, out: Vec::new() });
        }
        let link = self.link.as_mut().expect("connected");
        let job = self.sync.as_mut().expect("set above");
        let Some((number, name)) = job.list.get(job.index).cloned() else {
            let out = std::mem::take(&mut job.out);
            self.finish_sync(Ok(out));
            return Ok(());
        };
        match job.recalled_at {
            None => {
                match link.recall(number) {
                    Ok(()) => {
                        job.recalled_at = Some(Instant::now());
                        job.recall_refused = None;
                        job.still = 0;
                        let progress = format!("Recalling preset {} of {}: {name}", job.index + 1, job.list.len());
                        self.lock().sync_progress = Some(progress);
                    }
                    // The previous preset is still travelling. Ask again once it will take another.
                    Err(why) if busy(&why) => {
                        let since = *job.recall_refused.get_or_insert_with(Instant::now);
                        if since.elapsed() > Duration::from_secs(8) {
                            self.finish_sync(Err(why));
                        }
                    }
                    Err(why) => self.finish_sync(Err(why)),
                }
            }
            Some(since) => {
                // A running preset will not be asked where it is. That silence is the move, not a rest.
                job.still = if !answering || moved { 0 } else { job.still + 1 };
                let waited = since.elapsed();
                if waited > Duration::from_secs(40) {
                    self.finish_sync(Err(format!("Preset {number} ({name}) never came to rest")));
                    return Ok(());
                }
                // The head takes a moment to start; then it has to stand still, and be willing to say so.
                if answering && waited > Duration::from_millis(600) && job.still >= 6 {
                    let settled = Head { zoom: link.read_zoom()?, focus: link.read_focus()?, ..*head };
                    let thumbnail = preset_picture(&mut self.cgi, number);
                    let job = self.sync.as_mut().expect("still set");
                    job.out.push(DevicePreset {
                        preset: Preset {
                            id: String::new(),
                            camera: ptz_core::camera::first_camera(),
                            name,
                            pose: Pose {
                                pan: settled.pan,
                                tilt: settled.tilt,
                                zoom: settled.zoom,
                                focus: settled.focus,
                                elevation: 0.0,
                            },
                            camera_number: Some(number),
                            has_thumbnail: thumbnail.is_some(),
                        },
                        thumbnail,
                    });
                    job.index += 1;
                    job.recalled_at = None;
                }
            }
        }
        Ok(())
    }

    fn finish_sync(&mut self, result: Result<Vec<DevicePreset>, String>) {
        self.sync = None;
        let mut s = self.lock();
        s.sync_result = Some(result);
        // The progress line stays until the show has taken the result.
        s.sync_progress = Some("Storing the presets".into());
        if s.mode == Mode::Sync {
            s.mode = Mode::Idle;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lens like the one measured: it acts on a speed command 0.3 s late and reports its
    /// position in coarse steps. Runs a zoom from `from` to `to` over `seconds` on an eased curve
    /// and returns the worst error, each change of speed, and how many came after the move had settled.
    fn run_zoom(from: f64, to: f64, seconds: f64) -> (f64, Vec<Option<(bool, usize)>>, usize) {
        let start = Instant::now();
        let at = |t: f64| start + Duration::from_secs_f64(t);
        let curve = |t: f64| {
            let s = (t / seconds).clamp(0.0, 1.0);
            from + (to - from) * s * s * (3.0 - 2.0 * s)
        };
        let (cycle, lag) = (0.05, 0.3);
        let mut ramp = ZoomRamp::new(start);
        let mut sent: Vec<(f64, f64)> = vec![(-10.0, 0.0)];
        let (mut lens, mut read, mut read_t) = (from, from, 0.0);
        let (mut worst, mut changes, mut late) = (0.0f64, Vec::new(), 0);
        for n in 0..((seconds + 2.5) / cycle) as usize {
            let t = n as f64 * cycle;
            // The lens runs at whatever it was told `lag` ago.
            let speed = sent.iter().rev().find(|(when, _)| *when <= t - lag).map(|(_, speed)| *speed).unwrap_or(0.0);
            lens = (lens + speed * cycle).clamp(0.0, 1.0);
            if n % 2 == 0 {
                (read, read_t) = ((lens * 70.0).round() / 70.0, t);
            }
            let velocity = (curve(t + 0.005) - curve(t - 0.005)) / 0.01;
            let next = ramp.decide(at(t), curve(t), velocity, read, at(read_t), 1.0, 0.25);
            if next != ramp.running {
                ramp.took(next, at(t));
                sent.push((t, ZoomRamp::speed(next)));
                changes.push(next);
                if t > seconds + 1.0 {
                    late += 1;
                }
            }
            worst = worst.max((lens - curve(t)).abs());
        }
        (worst, changes, late)
    }

    #[test]
    fn a_zoom_move_climbs_through_the_speeds_one_at_a_time_and_comes_down_again() {
        for (from, to, seconds) in [(0.0, 1.0, 9.1), (1.0, 0.0, 7.0), (0.3, 0.62, 3.0), (0.62, 0.45, 2.0)] {
            let (worst, changes, late) = run_zoom(from, to, seconds);
            assert!(worst < 0.05, "{from} to {to}: the lens strayed {worst:.3} from the curve");
            assert_eq!(changes.last(), Some(&None), "{from} to {to}: the lens is stopped at the end");
            assert_eq!(late, 0, "{from} to {to}: the lens kept hunting after it arrived");
            assert!(changes.len() <= 16, "{from} to {to}: {} changes of speed", changes.len());
            for pair in changes.windows(2) {
                if let [Some((way, a)), Some((same, b))] = pair {
                    assert_eq!(way, same, "{from} to {to}: the lens never reverses without stopping");
                    assert_eq!(a.abs_diff(*b), 1, "{from} to {to}: one step at a time");
                }
            }
        }
    }

    #[test]
    fn a_lens_at_rest_close_to_its_target_is_left_alone() {
        let start = Instant::now();
        let mut ramp = ZoomRamp::new(start);
        for n in 0..40u32 {
            let now = start + Duration::from_millis(50 * n as u64);
            assert_eq!(ramp.decide(now, 0.5, 0.0, 0.493, now, 1.0, 0.25), None);
        }
        // Further off than it can be left, it is brought in.
        let mut ramp = ZoomRamp::new(start);
        let mut moved = None;
        for n in 0..40u32 {
            let now = start + Duration::from_millis(50 * n as u64);
            moved = moved.or(ramp.decide(now, 0.5, 0.0, 0.4, now, 1.0, 0.25));
        }
        assert!(matches!(moved, Some((true, _))), "{moved:?}");
    }

    /// What came of driving the simulated lens through the web interface.
    struct Glided {
        /// The furthest the lens strayed from the curve.
        worst: f64,
        /// Every speed the lens was sent and when, a stop being zero.
        sent: Vec<(f64, f64)>,
        /// Seconds after the end of the move at which the lens read as on its mark, if it did.
        arrived: Option<f64>,
        /// How far the reading rests from the mark.
        rests: f64,
    }

    /// The same lens driven through the web interface, where any speed can be asked for and a stop
    /// goes out at the moment fixed for it. `cycle` is how often the worker gets to decide; `steady`
    /// makes the target move at that speed and stop dead, as it does while positioning.
    fn run_glide(from: f64, to: f64, seconds: f64, cycle: f64, steady: Option<f64>, lag: f64) -> Glided {
        let start = Instant::now();
        let at = |t: f64| start + Duration::from_secs_f64(t);
        let seconds = steady.map_or(seconds, |speed| (to - from).abs() / speed);
        let curve = |t: f64| {
            let s = (t / seconds).clamp(0.0, 1.0);
            from + (to - from) * if steady.is_some() { s } else { s * s * (3.0 - 2.0 * s) }
        };
        let (tick, web) = (0.005, 0.03);
        let scale = if steady.is_some() { 0.5 } else { 1.0 };
        let mut glide = ZoomGlide::new(start);
        let mut sent: Vec<(f64, f64)> = vec![(-10.0, 0.0)];
        let (mut lens, mut read, mut read_t, mut worst) = (from, from, 0.0, 0.0f64);
        let (mut stop_at, mut arrived, mut cycles, mut next_cycle) = (None::<f64>, None, 0u32, 0.0);
        for n in 0..((seconds + 4.0) / tick) as usize {
            let t = n as f64 * tick;
            // The sender stops the lens at the moment fixed for it.
            if stop_at.is_some_and(|due| t >= due) {
                sent.push((t + web, 0.0));
                stop_at = None;
            }
            let speed = sent.iter().rev().find(|(when, _)| *when <= t - lag).map(|(_, speed)| *speed).unwrap_or(0.0);
            lens = (lens + speed * tick).clamp(0.0, 1.0);
            if t >= next_cycle {
                next_cycle += cycle;
                cycles += 1;
                // Zoom is read three cycles in four while the lens runs, every other cycle at rest.
                if if glide.running != 0.0 { cycles % 4 != 0 } else { cycles % 2 == 0 } {
                    (read, read_t) = ((lens * 70.0).round() / 70.0, t - 0.03);
                }
                // The decision comes after the readings of the cycle.
                let now = t + (cycle * 0.6).min(0.1);
                let velocity = (curve(now + 0.005) - curve(now - 0.005)) / 0.01;
                if let Some(order) = glide.decide(at(now), curve(now), velocity, read, at(read_t.max(0.0)), scale, 0.25) {
                    if order.rate != glide.running {
                        sent.push((now + web, order.rate));
                    }
                    stop_at = order.stop_at.map(|due| due.duration_since(start).as_secs_f64());
                    glide.took(order, at(now));
                }
                if t > seconds && arrived.is_none() && (read - to).abs() <= 0.0125 {
                    arrived = Some(t - seconds);
                }
            }
            worst = worst.max((lens - curve(t)).abs());
        }
        sent.remove(0);
        Glided { worst, sent, arrived, rests: (lens * 70.0).round() / 70.0 - to }
    }

    #[test]
    fn a_zoom_through_the_web_interface_starts_once_and_changes_speed_a_little_at_a_time() {
        // The lens acts on a command 0.1 to 0.4 s after it gets it; the worker decides every 0.05 to 0.2 s.
        for (cycle, lag) in [(0.05, 0.3), (0.12, 0.3), (0.2, 0.3), (0.05, 0.12), (0.1, 0.12), (0.1, 0.4)] {
            for (from, to, seconds) in [(0.0, 1.0, 22.0), (1.0, 0.0, 9.1), (0.0, 1.0, 7.0), (0.3, 0.62, 6.0)] {
                let run = run_glide(from, to, seconds, cycle, None, lag);
                let what = format!("{from} to {to} in {seconds} s, deciding every {cycle} s, lens {lag} s behind");
                assert!(run.worst < 0.05, "{what}: the lens strayed {:.3} from the curve", run.worst);
                assert_eq!(run.sent.last().map(|(_, rate)| *rate), Some(0.0), "{what}: the lens is stopped at the end");
                assert!(run.arrived.is_some_and(|after| after < 1.0), "{what}: on its mark {:?} s after the move", run.arrived);
                assert!(run.rests.abs() <= 0.0125, "{what}: rests {:.4} from its mark", run.rests);
                // The lens is started once and stopped once. Where it comes to rest depends on how
                // far behind it runs, which is not known to the step it reports in: when it rests a
                // step short it is brought in by one short run at its slowest.
                let stops: Vec<usize> = (0..run.sent.len()).filter(|i| run.sent[*i].1 == 0.0).collect();
                assert!(stops.len() <= 2, "{what}: started {} times: {:?}", stops.len(), run.sent);
                if stops.len() == 2 {
                    let last = &run.sent[stops[0] + 1..];
                    let (rate, lasted) = (last[0].1.abs(), last[1].0 - last[0].0);
                    assert!(
                        last.len() == 2 && rate == 0.038 && lasted < 0.6,
                        "{what}: brought in by more than a touch: {last:?}"
                    );
                }
                for pair in run.sent.windows(2) {
                    let ((_, a), (_, b)) = (pair[0], pair[1]);
                    assert!(a * b >= 0.0, "{what}: the lens never reverses without stopping");
                    // The less often the worker gets to decide, the more each change has to cover.
                    if a != 0.0 && b != 0.0 {
                        let most = 0.012 + 0.25 * a.abs() + 0.15 * cycle;
                        assert!((b - a).abs() <= most, "{what}: the speed jumped from {a:.3} to {b:.3}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_lens_sent_to_a_pose_through_the_web_interface_comes_to_rest_on_it() {
        for (cycle, lag) in [(0.05, 0.3), (0.12, 0.3), (0.2, 0.3), (0.3, 0.3), (0.05, 0.12), (0.1, 0.12), (0.1, 0.4)] {
            for (from, to) in [(1.0, 0.043), (0.0, 1.0), (0.4, 0.47), (0.9, 0.2), (0.33, 0.31)] {
                let run = run_glide(from, to, 0.0, cycle, Some(0.125), lag);
                let what = format!("{from} to {to}, deciding every {cycle} s, lens {lag} s behind");
                assert!(run.arrived.is_some_and(|after| after < 3.5), "{what}: on its mark {:?} s after the target", run.arrived);
                assert!(run.rests.abs() <= 0.0125, "{what}: rests {:.4} from its mark", run.rests);
                assert_eq!(run.sent.last().map(|(_, rate)| *rate), Some(0.0), "{what}: the lens is stopped at the end");
                let starts = run.sent.iter().filter(|(_, rate)| *rate == 0.0).count();
                assert!(starts <= 2, "{what}: started {starts} times: {:?}", run.sent);
            }
        }
    }

    #[test]
    fn one_command_that_says_the_zoom_runs_backwards_is_left_out() {
        let mut last = [0.0; 3];
        let given = [0.061, 0.0607, 0.0602, -0.1069, 0.0588, 0.0583, 0.0576];
        let taken: Vec<f64> = given.iter().map(|speed| steady_speed(&mut last, *speed)).collect();
        assert_eq!(taken[2..], [0.0607, 0.0602, 0.0588, 0.0583, 0.0583]);
        // A speed that stays is taken up one command later.
        let mut last = [0.06; 3];
        assert_eq!([0.0, 0.0, 0.0].map(|speed| steady_speed(&mut last, speed)), [0.06, 0.0, 0.0]);
    }

    #[test]
    fn a_timeline_that_sets_off_at_full_speed_does_not_send_the_lens_off_faster() {
        let start = Instant::now();
        let at = |t: f64| start + Duration::from_secs_f64(t);
        let mut glide = ZoomGlide::new(start);
        let mut fastest = 0.0f64;
        for n in 0..40 {
            let t = n as f64 * 0.08;
            // At rest on 0.36 for a second, then off at 0.068 of the range a second.
            let (target, velocity) = if t < 1.0 { (0.36, 0.0) } else { (0.36 + 0.068 * (t - 1.0), 0.068) };
            let lens = if t < 1.3 { 0.36 } else { 0.36 + 0.068 * (t - 1.3) };
            if let Some(order) = glide.decide(at(t), target, velocity, lens, at(t), 1.0, 0.25) {
                fastest = fastest.max(order.rate.abs());
                glide.took(order, at(t));
            }
        }
        assert!(glide.running > 0.06, "the lens is running with the timeline: {}", glide.running);
        assert!(fastest < 0.068 * 1.5, "the lens was sent off at {fastest:.3}");
    }

    #[test]
    fn what_the_lens_was_told_adds_up_over_any_stretch() {
        let start = Instant::now();
        let at = |t: f64| start + Duration::from_secs_f64(t);
        let mut glide = ZoomGlide::new(start);
        glide.took(ZoomOrder { rate: 0.1, stop_at: None }, at(1.0));
        glide.took(ZoomOrder { rate: 0.2, stop_at: None }, at(2.0));
        glide.took(ZoomOrder { rate: 0.2, stop_at: Some(at(3.0)) }, at(2.5));
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(close(glide.told_between(at(2.5), 2.5, 0.0), 0.1 + 0.1));
        assert!(close(glide.told_between(at(2.5), 1.0, 0.25), 0.05 + 0.05));
        assert!(close(glide.told_between(at(2.5), 9.0, 2.0), 0.0));
        // The stop that was fixed has happened by now.
        glide.settle(at(4.0));
        assert_eq!(glide.running, 0.0);
        assert!(close(glide.told_between(at(4.0), 4.0, 0.0), 0.1 + 0.2));
    }

    #[test]
    fn web_zoom_speeds_cover_the_range_the_lens_runs_at() {
        assert_eq!(ZoomGlide::value_for(0.01), 0);
        assert_eq!(ZoomGlide::value_for(0.038), 0);
        assert_eq!(ZoomGlide::value_for(0.076), 10000);
        assert_eq!(ZoomGlide::value_for(-0.25), 24000);
        assert_eq!(ZoomGlide::value_for(2.0), 32766);
        assert!(ZoomGlide::value_for(0.1) > 12000 && ZoomGlide::value_for(0.1) < 14000);
        assert_eq!(ZoomGlide::request(0.0), "/command/ptzf.cgi?ZoomMove=stop,0");
        assert_eq!(ZoomGlide::request(0.25), "/command/ptzf.cgi?ZoomMove=tele,24000");
        assert_eq!(ZoomGlide::request(-0.076), "/command/ptzf.cgi?ZoomMove=wide,10000");
        assert!(WEB_ZOOM.windows(2).all(|pair| pair[0].0 < pair[1].0 && pair[0].1 < pair[1].1));
    }

    #[test]
    fn a_lens_at_rest_close_to_its_target_is_not_started_through_the_web_interface() {
        let start = Instant::now();
        let mut glide = ZoomGlide::new(start);
        for n in 0..40u32 {
            let now = start + Duration::from_millis(50 * n as u64);
            assert_eq!(glide.decide(now, 0.5, 0.0, 0.493, now, 1.0, 0.25), None);
        }
        // A little further off, it is run at its slowest for as long as the distance takes.
        let mut glide = ZoomGlide::new(start);
        let order = glide.decide(start, 0.5, 0.0, 0.462, start, 1.0, 0.25).expect("brought in");
        assert_eq!(order.rate, 0.038);
        let lasts = order.stop_at.expect("a stop is fixed").duration_since(start).as_secs_f64();
        assert!((lasts - 1.0).abs() < 1e-6, "{lasts}");
        // Far off, it is started and steered.
        let mut glide = ZoomGlide::new(start);
        let order = glide.decide(start, 0.5, 0.0, 0.2, start, 1.0, 0.25).expect("started");
        assert!(order.rate > 0.038 && order.stop_at.is_none(), "{order:?}");
    }

    #[test]
    fn every_zoom_speed_is_a_step_the_lens_knows() {
        assert_eq!(ZoomRamp::command(None), visca::zoom_stop());
        assert_eq!(ZoomRamp::command(Some((true, 0))), visca::zoom_toward(true, 0));
        assert_eq!(ZoomRamp::command(Some((false, 6))), visca::zoom_toward(false, 7));
        assert!(ZOOM_SPEEDS.windows(2).all(|pair| pair[0].1 < pair[1].1), "each step is faster than the last");
    }
}
