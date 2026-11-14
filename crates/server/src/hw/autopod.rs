//! Polecam AutoPod+: elevation over the leg box's Panasonic-style HTTP
//! commands. Elevation is the tilt axis of those commands; pan is unused.
//!
//! The request and reply forms are the ones the owner's autopod-osc-bridge
//! recorded between a real TX (v3.30) and RX (v3.49e). That bridge never
//! forwarded motion to a real RX, so how the column answers a speed byte is
//! still measured by the bring-up tool.

use super::{with_port, Loop, PERIOD};
use crate::rig::{Drive, EstopLine};
use ptz_core::api::AdapterStatus;
use ptz_core::pose::{AxisLimits, CalPoint, RigConfig};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Heights the operator has to measure before the raw position means millimetres.
pub const CAL_POINTS_NEEDED: usize = 3;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Calibration {
    Calibrated,
    Uncalibrated,
    Calibrating,
    MotorError,
    Unknown,
}

impl Calibration {
    /// From the answer to `#RER`.
    pub fn parse(answer: &str) -> Calibration {
        match answer.trim() {
            "rER00" => Calibration::Calibrated,
            "rER53" => Calibration::Uncalibrated,
            "rER42" => Calibration::Calibrating,
            "rER03" => Calibration::MotorError,
            _ => Calibration::Unknown,
        }
    }

    pub fn problem(self) -> Option<&'static str> {
        match self {
            Calibration::Calibrated => None,
            Calibration::Uncalibrated => Some("The AutoPod+ is not calibrated. Run its calibration at the leg box"),
            Calibration::Calibrating => Some("The AutoPod+ is still calibrating"),
            Calibration::MotorError => Some("The AutoPod+ reports a motor error"),
            Calibration::Unknown => Some("The AutoPod+'s calibration state was not understood"),
        }
    }
}

/// The path and query that carries `cmd`, for example `#PTS5050`.
pub fn request_path(cmd: &str) -> String {
    format!("/cgi-bin/aw_ptz?cmd={}&res=1", cmd.replace('#', "%23"))
}

/// Velocity: `#PTS50tt`, where `tt` is 50 for stop, up to 99 up and down to 00 down.
pub fn velocity_command(tt: u8) -> String {
    format!("#PTS50{:02}", tt.min(99))
}

pub const STOP: &str = "#PTS5050";
pub const POSITION_QUERY: &str = "#APC";
pub const CALIBRATION_QUERY: &str = "#RER";
pub const MODEL_QUERY: &str = "QID";
pub const FIRMWARE_QUERY: &str = "#V?";
pub const POWER_QUERY: &str = "#O";
pub const MODEL: &str = "AUTOPOD+";

/// The speed byte for a wanted speed as a share of top speed, from -1 (down) to 1 (up).
/// The TX sends 00 for full down and 99 for full up, so down has 50 steps and up 49.
/// A speed too small for one step still takes one, so a slow correction does not stall.
pub fn speed_byte(share: f64) -> u8 {
    let share = share.clamp(-1.0, 1.0);
    if share.abs() < 1e-4 {
        return 50;
    }
    let steps = if share < 0.0 { share * 50.0 } else { share * 49.0 };
    let steps = if steps.abs() < 1.0 { steps.signum() } else { steps.round() };
    (50.0 + steps) as u8
}

/// The code of a refusal such as `ER1:PTS5075`, which is how the leg box answers a command it does not support.
pub fn refusal(answer: &str) -> Option<&str> {
    let (code, _) = answer.trim().split_once(':')?;
    let digits = code.strip_prefix("ER").or_else(|| code.strip_prefix("eR"))?;
    (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())).then_some(code)
}

/// The model from the answer to `QID`, for example `AUTOPOD+` from `OID:AUTOPOD+`.
pub fn parse_model(answer: &str) -> Option<&str> {
    answer.trim().strip_prefix("OID:").map(str::trim).filter(|m| !m.is_empty())
}

/// From the answer to `#O`: `p1` is on, `p0` is standby.
pub fn parse_power(answer: &str) -> Option<bool> {
    match answer.trim() {
        "p1" => Some(true),
        "p0" => Some(false),
        _ => None,
    }
}

/// The bytes of one request. The real RX answers exactly this, with no headers.
pub fn request_bytes(cmd: &str) -> Vec<u8> {
    format!("GET {} HTTP/1.0\r\n\r\n", request_path(cmd)).into_bytes()
}

/// Status code and body of an HTTP/1.0 reply read to the end of the connection.
pub fn parse_reply(raw: &[u8]) -> Option<(u16, String)> {
    let text = String::from_utf8_lossy(raw);
    let code = text.lines().next()?.split_whitespace().nth(1)?.parse().ok()?;
    let body = text.split_once("\r\n\r\n").or_else(|| text.split_once("\n\n")).map(|(_, b)| b).unwrap_or("");
    Some((code, body.trim().to_string()))
}

/// The raw tilt value from `aPC` plus eight hex digits (pan, then tilt).
pub fn parse_position(answer: &str) -> Option<u16> {
    let digits = answer.trim().strip_prefix("aPC")?;
    // Anything but eight hex digits is not a position, whatever a damaged reply holds in their place.
    if digits.len() != 8 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    u16::from_str_radix(&digits[4..], 16).ok()
}

/// Millimetres for a raw position, by straight lines between the measured
/// heights, carried on past the first and last.
pub fn height_mm(map: &[CalPoint], raw: f64) -> Option<f64> {
    let mut points = map.to_vec();
    points.sort_by(|a, b| a.raw.total_cmp(&b.raw));
    points.dedup_by(|a, b| a.raw == b.raw);
    if points.len() < 2 {
        return None;
    }
    let i = points.partition_point(|p| p.raw <= raw).clamp(1, points.len() - 1);
    let (a, b) = (points[i - 1], points[i]);
    Some(a.mm + (b.mm - a.mm) * (raw - a.raw) / (b.raw - a.raw))
}

#[derive(Clone, PartialEq, Debug)]
pub struct PodConfig {
    /// `host:port` of the leg box.
    pub host: String,
    pub map: Vec<CalPoint>,
}

impl PodConfig {
    pub fn of(rig: &RigConfig) -> PodConfig {
        PodConfig { host: with_port(rig.autopod_host.trim(), 80), map: rig.autopod_calibration.clone() }
    }
}

const TIMEOUT: Duration = Duration::from_millis(400);
const LONGEST_REPLY: u64 = 16 * 1024;

/// What the leg box says about itself. Each part is absent when the unit does not answer that question.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Identity {
    pub model: Option<String>,
    pub firmware: Option<String>,
}

impl Identity {
    /// "AutoPod+ V3.49e", or what the unit called itself when it is something else.
    pub fn label(&self) -> String {
        let model = match self.model.as_deref() {
            None | Some(MODEL) => "AutoPod+".to_string(),
            Some(other) => format!("{other} (not an AutoPod+)"),
        };
        match &self.firmware {
            Some(v) => format!("{model} {v}"),
            None => model,
        }
    }
}

/// One conversation with the leg box, used directly by the bring-up tool.
pub struct Link {
    pub host: String,
    pub last_round_trip: Duration,
    /// The last command reached the leg box and was refused, as opposed to not arriving.
    pub last_refused: bool,
}

impl Link {
    pub fn new(host: &str) -> Link {
        Link { host: host.to_string(), last_round_trip: Duration::ZERO, last_refused: false }
    }

    fn fetch(&self, cmd: &str) -> std::io::Result<Vec<u8>> {
        let deadline = Instant::now() + TIMEOUT;
        let addr = self.host.to_socket_addrs()?.next().ok_or(std::io::ErrorKind::AddrNotAvailable)?;
        let mut stream = TcpStream::connect_timeout(&addr, TIMEOUT)?;
        stream.set_nodelay(true)?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        stream.write_all(&request_bytes(cmd))?;
        let mut raw = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            stream.set_read_timeout(Some(left))?;
            match stream.read(&mut chunk)? {
                0 => return Ok(raw),
                n => raw.extend_from_slice(&chunk[..n]),
            }
            if raw.len() as u64 > LONGEST_REPLY {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
        }
    }

    pub fn send(&mut self, cmd: &str) -> Result<String, String> {
        self.last_refused = false;
        let question = [POSITION_QUERY, CALIBRATION_QUERY, MODEL_QUERY, FIRMWARE_QUERY, POWER_QUERY].contains(&cmd);
        if super::only_stops_now() && cmd != STOP && !question {
            return Err("this tool is being ended, and sends the AutoPod+ nothing but stops".into());
        }
        let began = Instant::now();
        let gone = |why: String| format!("the AutoPod+ is not answering at {}: {why}", self.host);
        let raw = self.fetch(cmd).map_err(|e| gone(e.to_string()))?;
        let (code, body) = parse_reply(&raw).ok_or_else(|| gone("the reply was not HTTP".into()))?;
        self.last_round_trip = began.elapsed();
        if code != 200 {
            self.last_refused = true;
            return Err(format!("the AutoPod+ answered {code} to {cmd}"));
        }
        if let Some(code) = refusal(&body) {
            self.last_refused = true;
            return Err(format!("the AutoPod+ refused {cmd} ({code})"));
        }
        Ok(body)
    }

    /// The answer to a question the motion path does not depend on. A unit that refuses it simply does not say.
    fn ask(&mut self, cmd: &str) -> Result<Option<String>, String> {
        match self.send(cmd) {
            Ok(answer) => Ok(Some(answer)),
            Err(_) if self.last_refused => Ok(None),
            Err(why) => Err(why),
        }
    }

    pub fn position(&mut self) -> Result<u16, String> {
        let answer = self.send(POSITION_QUERY)?;
        parse_position(&answer).ok_or_else(|| format!("the AutoPod+'s position answer was not understood: {answer}"))
    }

    pub fn calibration(&mut self) -> Result<Calibration, String> {
        self.send(CALIBRATION_QUERY).map(|a| Calibration::parse(&a))
    }

    pub fn identity(&mut self) -> Result<Identity, String> {
        let model = self.ask(MODEL_QUERY)?.and_then(|a| parse_model(&a).map(str::to_string));
        let firmware = self.ask(FIRMWARE_QUERY)?.filter(|v| !v.is_empty());
        Ok(Identity { model, firmware })
    }

    /// Whether the unit is switched on, when it says.
    pub fn power(&mut self) -> Result<Option<bool>, String> {
        Ok(self.ask(POWER_QUERY)?.and_then(|a| parse_power(&a)))
    }

    pub fn velocity(&mut self, share: f64) -> Result<(), String> {
        self.send(&velocity_command(speed_byte(share))).map(|_| ())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        self.send(STOP).map(|_| ())
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Idle,
    Drive(Drive),
    Stop,
}

struct State {
    raw: f64,
    mm: Option<f64>,
    read_at: Instant,
    connected: bool,
    detail: String,
    calibration: Calibration,
    identity: Identity,
    power: Option<bool>,
    refused: Option<String>,
    round_trip_ms: f64,
    moving: bool,
    mode: Mode,
    told_at: Instant,
    starved: bool,
    limits: AxisLimits,
    line: EstopLine,
    quit: bool,
}

impl State {
    /// A pressed E-Stop line stops the column, whatever the show last asked of it.
    fn obey_line(&mut self) {
        if self.line.is_pressed() && matches!(self.mode, Mode::Drive(_)) {
            self.mode = Mode::Stop;
        }
    }
}

pub struct AutoPod {
    cfg: PodConfig,
    state: Arc<Mutex<State>>,
}

const STARVED_AFTER: Duration = Duration::from_millis(300);
const GAIN: f64 = 4.0;

impl AutoPod {
    pub fn start(cfg: PodConfig, limits: AxisLimits) -> AutoPod {
        let now = Instant::now();
        let state = Arc::new(Mutex::new(State {
            raw: 0.0,
            mm: None,
            read_at: now,
            connected: false,
            detail: format!("Connecting to {}", cfg.host),
            calibration: Calibration::Unknown,
            identity: Identity::default(),
            power: None,
            refused: None,
            round_trip_ms: 0.0,
            moving: false,
            mode: Mode::Idle,
            told_at: now,
            starved: false,
            limits,
            line: EstopLine::default(),
            quit: false,
        }));
        let (shared, worker_cfg) = (state.clone(), cfg.clone());
        std::thread::Builder::new().name("autopod".into()).spawn(move || run(worker_cfg, shared)).expect("autopod thread");
        let (shared, host) = (state.clone(), cfg.host.clone());
        std::thread::Builder::new().name("autopod-stop".into()).spawn(move || stop_at_once(host, shared)).expect("stop thread");
        AutoPod { cfg, state }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> AdapterStatus {
        let s = self.lock();
        AdapterStatus {
            name: "AutoPod+".into(),
            connected: s.connected,
            detail: s.detail.clone(),
            calibrated: s.connected.then_some(s.calibration == Calibration::Calibrated),
            round_trip_ms: s.connected.then_some(s.round_trip_ms),
            raw_position: s.connected.then_some(s.raw),
        }
    }

    pub fn problem(&self) -> Option<String> {
        let s = self.lock();
        if !s.connected {
            return Some(format!("The AutoPod+ is not answering at {}", self.cfg.host));
        }
        if s.power == Some(false) {
            return Some("The AutoPod+ is in standby. Switch it on at the leg box".into());
        }
        if let Some(why) = &s.refused {
            return Some(format!("{}{}", why[..1].to_uppercase(), &why[1..]));
        }
        if let Some(why) = s.calibration.problem() {
            return Some(why.to_string());
        }
        if self.cfg.map.len() < CAL_POINTS_NEEDED {
            return Some(format!(
                "The AutoPod+'s height map has {} of the {CAL_POINTS_NEEDED} measured heights it needs. Add them in the rig settings",
                self.cfg.map.len()
            ));
        }
        if s.starved {
            return Some("The AutoPod+ was stopped because the show stopped sending it targets".into());
        }
        None
    }

    /// Elevation in millimetres and when it was read. Zero until the height map can say.
    pub fn elevation(&self) -> (f64, Instant) {
        let s = self.lock();
        (s.mm.unwrap_or(0.0), s.read_at)
    }

    pub fn round_trip_s(&self) -> f64 {
        self.lock().round_trip_ms / 1000.0
    }

    pub fn is_moving(&self) -> bool {
        let s = self.lock();
        s.moving || s.mode == Mode::Stop
    }

    pub fn set_limits(&self, limits: &AxisLimits) {
        self.lock().limits = *limits;
    }

    /// Stop when this E-Stop line is pressed, without waiting to be told by the show.
    pub fn on_line(&self, line: &EstopLine) {
        self.lock().line = line.clone();
    }

    pub fn drive(&self, cmd: &Drive) {
        let mut s = self.lock();
        if s.line.is_pressed() {
            return;
        }
        s.mode = Mode::Drive(*cmd);
        s.told_at = Instant::now();
        s.starved = false;
    }

    pub fn stop(&self) {
        self.lock().mode = Mode::Stop;
    }
}

impl Drop for AutoPod {
    fn drop(&mut self) {
        self.lock().quit = true;
    }
}

/// The control loop asks the leg box one thing at a time and waits for each answer, up to
/// 400 ms. A stop does not wait in that queue: the moment one is asked for, it is sent here,
/// on a connection of its own. The control loop still sends its own stops until the column is still.
fn stop_at_once(host: String, state: Arc<Mutex<State>>) {
    let mut link = Link::new(&host);
    let mut was_stopping = false;
    loop {
        let stopping = {
            let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
            if s.quit {
                return;
            }
            s.obey_line();
            s.mode == Mode::Stop
        };
        if stopping && !was_stopping {
            let _ = link.stop();
        }
        was_stopping = stopping;
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn run(cfg: PodConfig, state: Arc<Mutex<State>>) {
    let lock = || state.lock().unwrap_or_else(|e| e.into_inner());
    let mut link = Link::new(&cfg.host);
    let mut pace = Loop::new(PERIOD);
    let mut cycle = 0u64;
    let mut last_byte: Option<u8> = None;
    let mut sent_at_cycle = 0u64;
    let (mut stops_sent, mut still) = (0u32, 0u32);
    let mut was = Mode::Idle;
    loop {
        if lock().quit {
            let _ = link.stop();
            return;
        }
        cycle += 1;
        // A stop goes out before anything is asked of the column, so it never waits on a reading
        // and is not lost with one. A column that has not been heard from may still be on the
        // last speed it was given, by this server or by one that died while live.
        let stop_first = {
            let mut s = lock();
            s.obey_line();
            s.mode == Mode::Stop || (!s.connected && s.mode == Mode::Idle)
        };
        let stop = if stop_first { link.stop() } else { Ok(()) };
        let stop_sent = stop_first && stop.is_ok();
        let unreachable = stop.err().filter(|_| !link.last_refused);
        let step = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<(), String> {
            if let Some(why) = unreachable {
                return Err(why);
            }
            let raw = link.position()? as f64;
            let read_at = Instant::now() - link.last_round_trip / 2;
            let round_trip_ms = link.last_round_trip.as_secs_f64() * 1000.0;
            // Calibration and power are asked once a second, and at once after connecting.
            let fresh = !lock().connected;
            let health = if cycle % 20 == 1 || fresh { Some((link.calibration()?, link.power()?)) } else { None };
            let identity = if fresh { Some(link.identity()?) } else { None };
            let mm = height_mm(&cfg.map, raw);
            let (mode, limits, moved) = {
                let mut s = lock();
                // A server that died while live leaves the column on its last speed command.
                if fresh && s.mode == Mode::Idle {
                    s.mode = Mode::Stop;
                }
                let moved = s.connected && (raw - s.raw).abs() > 1.0;
                s.raw = raw;
                s.mm = mm;
                s.read_at = read_at;
                s.connected = true;
                s.round_trip_ms = round_trip_ms;
                s.moving = moved;
                if let Some((calibration, power)) = health {
                    s.calibration = calibration;
                    s.power = power;
                }
                if let Some(identity) = identity {
                    s.identity = identity;
                }
                s.detail = match s.power {
                    Some(false) => format!("{} at {} is in standby", s.identity.label(), cfg.host),
                    _ => format!("{} at {} answers in {:.1} ms", s.identity.label(), cfg.host, round_trip_ms),
                };
                if matches!(s.mode, Mode::Drive(_)) && s.told_at.elapsed() > STARVED_AFTER {
                    s.mode = Mode::Stop;
                    s.starved = true;
                }
                s.obey_line();
                (s.mode, s.limits, moved)
            };
            if mode != was {
                if mode == Mode::Stop {
                    stops_sent = 0;
                    still = 0;
                }
                last_byte = None;
                was = mode;
            }
            match mode {
                Mode::Idle => {}
                Mode::Drive(cmd) => {
                    let Some(at) = mm else {
                        return Ok(());
                    };
                    let top = limits.max_speed.max(1e-6);
                    let scale = cmd.scale.clamp(0.0, 1.0);
                    let speed = (cmd.velocity.elevation + GAIN * (cmd.target.elevation - at)).clamp(-top * scale, top * scale);
                    let byte = speed_byte(speed / top);
                    if last_byte != Some(byte) || cycle - sent_at_cycle >= 10 {
                        link.send(&velocity_command(byte))?;
                        last_byte = Some(byte);
                        sent_at_cycle = cycle;
                        lock().refused = None;
                    }
                }
                Mode::Stop => {
                    if !stop_sent {
                        link.stop()?;
                    }
                    lock().refused = None;
                    stops_sent += 1;
                    still = if moved { 0 } else { still + 1 };
                    if stops_sent >= 2 && still >= 2 {
                        let mut s = lock();
                        if s.mode == Mode::Stop {
                            s.mode = Mode::Idle;
                        }
                    }
                }
            }
            Ok(())
        }));
        let step = step.unwrap_or_else(|_| {
            // An answer this code did not expect is a column that cannot be read, not the end of the adapter.
            link.last_refused = false;
            Err(format!("the AutoPod+ at {} gave an answer the adapter could not handle", cfg.host))
        });
        if let Err(why) = step {
            let refused = link.last_refused;
            let mut s = lock();
            // A column that cannot be read cannot be steered. It is not left on its last speed.
            let driving = matches!(s.mode, Mode::Drive(_));
            if driving {
                s.mode = Mode::Stop;
            }
            let lost = !(refused && s.connected);
            if lost {
                s.connected = false;
                s.moving = false;
                s.detail = why;
            } else {
                // The unit is there and said no: a setting or firmware matter, not a lost cable.
                s.refused = Some(why);
            }
            drop(s);
            last_byte = None;
            if driving {
                let _ = link.stop();
            }
            if lost {
                std::thread::sleep(Duration::from_millis(250));
            }
        }
        pace.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_match_what_the_real_units_exchange() {
        assert_eq!(request_path("#PTS5075"), "/cgi-bin/aw_ptz?cmd=%23PTS5075&res=1");
        assert_eq!(request_path("QID"), "/cgi-bin/aw_ptz?cmd=QID&res=1");
        assert_eq!(request_bytes("#APC"), b"GET /cgi-bin/aw_ptz?cmd=%23APC&res=1 HTTP/1.0\r\n\r\n");
        assert_eq!(velocity_command(50), "#PTS5050");
        assert_eq!(velocity_command(7), "#PTS5007");
        assert_eq!(velocity_command(0), "#PTS5000", "00 is the TX's full-down byte");
        assert_eq!(velocity_command(120), "#PTS5099", "the speed byte stays in two digits");
        assert_eq!(STOP, "#PTS5050");
    }

    #[test]
    fn the_speed_byte_spans_00_to_99_around_50() {
        assert_eq!(speed_byte(0.0), 50);
        assert_eq!(speed_byte(1.0), 99);
        assert_eq!(speed_byte(-1.0), 0);
        assert_eq!(speed_byte(-7.0), 0);
        assert_eq!(speed_byte(7.0), 99);
        assert_eq!(speed_byte(0.5), 75);
        assert_eq!(speed_byte(-0.5), 25);
        assert_eq!(speed_byte(0.004), 51, "a slow correction still moves");
        assert_eq!(speed_byte(-0.004), 49);
        assert_eq!(speed_byte(0.00001), 50, "noise does not");
        // What the TX's byte means to the RX, by the bridge's own rule, comes back as the share asked.
        let share_of = |raw: u8| (raw as f64 - 50.0) / if raw < 50 { 50.0 } else { 49.0 };
        for share in [-1.0, -0.6, -0.2, 0.2, 0.6, 1.0] {
            assert!((share_of(speed_byte(share)) - share).abs() <= 0.011, "{share}");
        }
    }

    #[test]
    fn replies_are_read_as_the_rx_frames_them() {
        let raw =
            b"HTTP/1.0 200 OK\r\nServer: AutoPod+/1.7\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\naPC80008A3C\r\n";
        assert_eq!(parse_reply(raw), Some((200, "aPC80008A3C".to_string())));
        assert_eq!(parse_reply(b"HTTP/1.0 404 Not Found\r\n\r\nNot Found\r\n"), Some((404, "Not Found".to_string())));
        assert_eq!(parse_reply(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"), Some((200, String::new())));
        assert_eq!(parse_reply(b"hello"), None);
        assert_eq!(parse_reply(b""), None);
    }

    #[test]
    fn refusals_identity_and_power_are_read() {
        assert_eq!(refusal("ER1:PTS5075"), Some("ER1"));
        assert_eq!(refusal("ER3:APC"), Some("ER3"));
        assert_eq!(refusal("eR1:XYZ"), Some("eR1"));
        assert_eq!(refusal("pTS5075"), None);
        assert_eq!(refusal("OID:AUTOPOD+"), None);
        assert_eq!(refusal("ER:"), None);
        assert_eq!(parse_model("OID:AUTOPOD+"), Some("AUTOPOD+"));
        assert_eq!(parse_model("V3.49e"), None);
        assert_eq!(parse_power("p1"), Some(true));
        assert_eq!(parse_power("p0"), Some(false));
        assert_eq!(parse_power("rER00"), None);
        let id = |model: Option<&str>, firmware: Option<&str>| Identity {
            model: model.map(str::to_string),
            firmware: firmware.map(str::to_string),
        };
        assert_eq!(id(Some("AUTOPOD+"), Some("V3.49e")).label(), "AutoPod+ V3.49e");
        assert_eq!(id(None, None).label(), "AutoPod+");
        assert_eq!(id(Some("AW-HE130"), Some("V1.0")).label(), "AW-HE130 (not an AutoPod+) V1.0");
    }

    #[test]
    fn answers_are_read() {
        assert_eq!(parse_position("aPC7FFF8A3C"), Some(0x8A3C));
        assert_eq!(parse_position("aPC80008000"), Some(0x8000));
        assert_eq!(parse_position("aPC7FFF"), None);
        assert_eq!(parse_position("rER00"), None);
        // What a reply with one damaged byte reads as: the same length, and not a position.
        assert_eq!(parse_position(&String::from_utf8_lossy(b"aPC80\xE9000")), None);
        assert_eq!(parse_position("aPC8000+A3C"), None);
        assert_eq!(parse_position("aPC????????"), None);
        assert_eq!(Calibration::parse("rER00"), Calibration::Calibrated);
        assert_eq!(Calibration::parse("rER53"), Calibration::Uncalibrated);
        assert_eq!(Calibration::parse("rER42"), Calibration::Calibrating);
        assert_eq!(Calibration::parse("rER03"), Calibration::MotorError);
        assert!(Calibration::parse("rER99").problem().is_some());
        assert!(Calibration::Calibrated.problem().is_none());
    }

    #[test]
    fn the_height_map_reads_between_and_beyond_the_measured_heights() {
        let map =
            [CalPoint { raw: 30000.0, mm: 400.0 }, CalPoint { raw: 10000.0, mm: 0.0 }, CalPoint { raw: 20000.0, mm: 250.0 }];
        assert_eq!(height_mm(&map, 10000.0), Some(0.0));
        assert_eq!(height_mm(&map, 15000.0), Some(125.0));
        assert_eq!(height_mm(&map, 25000.0), Some(325.0));
        assert_eq!(height_mm(&map, 32000.0), Some(430.0));
        assert_eq!(height_mm(&map, 8000.0), Some(-50.0));
        assert_eq!(height_mm(&map[..1], 30000.0), None, "one height is not a map");
    }
}
