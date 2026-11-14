//! Polecam AutoPod Plus: elevation over the leg box's Panasonic-style HTTP
//! commands. Elevation is the tilt axis of those commands; pan is unused.
//!
//! Source: the open-source Bitfocus Companion module for the AutoPod Plus. It
//! is a community module, not Polecam documentation, so the bring-up tool
//! confirms each command on the unit.

use super::{with_port, Loop, PERIOD};
use crate::rig::Drive;
use ptz_core::api::AdapterStatus;
use ptz_core::pose::{AxisLimits, CalPoint, RigConfig};
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
            Calibration::Uncalibrated => Some("The AutoPod is not calibrated. Run its calibration at the leg box"),
            Calibration::Calibrating => Some("The AutoPod is still calibrating"),
            Calibration::MotorError => Some("The AutoPod reports a motor error"),
            Calibration::Unknown => Some("The AutoPod's calibration state was not understood"),
        }
    }
}

/// The path and query that carries `cmd`, for example `#PTS5050`.
pub fn request_path(cmd: &str) -> String {
    format!("/cgi-bin/aw_ptz?cmd={}&res=1", cmd.replace('#', "%23"))
}

/// Velocity: `#PTS50tt`, where `tt` is 50 for stop, above 50 up and below 50 down.
pub fn velocity_command(tt: u8) -> String {
    format!("#PTS50{:02}", tt.clamp(1, 99))
}

pub const STOP: &str = "#PTS5050";
pub const POSITION_QUERY: &str = "#APC";
pub const CALIBRATION_QUERY: &str = "#RER";

/// The speed byte for a wanted speed as a share of top speed, from -1 (down) to 1 (up).
pub fn speed_byte(share: f64) -> u8 {
    (50.0 + share.clamp(-1.0, 1.0) * 49.0).round() as u8
}

/// The raw tilt value from `aPC` plus eight hex digits (pan, then tilt).
pub fn parse_position(answer: &str) -> Option<u16> {
    let digits = answer.trim().strip_prefix("aPC")?;
    (digits.len() == 8).then(|| u16::from_str_radix(&digits[4..], 16).ok()).flatten()
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

/// One conversation with the leg box, used directly by the bring-up tool.
pub struct Link {
    agent: ureq::Agent,
    pub host: String,
    pub last_round_trip: Duration,
}

impl Link {
    pub fn new(host: &str) -> Link {
        let timeout = Duration::from_millis(400);
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(timeout))
            .timeout_global(Some(timeout))
            .build();
        Link { agent: ureq::Agent::new_with_config(config), host: host.to_string(), last_round_trip: Duration::ZERO }
    }

    pub fn send(&mut self, cmd: &str) -> Result<String, String> {
        let began = Instant::now();
        let url = format!("http://{}{}", self.host, request_path(cmd));
        let mut resp = self.agent.get(url).call().map_err(|e| format!("the AutoPod is not answering at {}: {e}", self.host))?;
        let code = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
        self.last_round_trip = began.elapsed();
        if !(200..300).contains(&code) {
            return Err(format!("the AutoPod answered {code} to {cmd}"));
        }
        Ok(text.trim().to_string())
    }

    pub fn position(&mut self) -> Result<u16, String> {
        let answer = self.send(POSITION_QUERY)?;
        parse_position(&answer).ok_or_else(|| format!("the AutoPod's position answer was not understood: {answer}"))
    }

    pub fn calibration(&mut self) -> Result<Calibration, String> {
        self.send(CALIBRATION_QUERY).map(|a| Calibration::parse(&a))
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
    round_trip_ms: f64,
    moving: bool,
    mode: Mode,
    told_at: Instant,
    starved: bool,
    limits: AxisLimits,
    quit: bool,
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
            round_trip_ms: 0.0,
            moving: false,
            mode: Mode::Idle,
            told_at: now,
            starved: false,
            limits,
            quit: false,
        }));
        let (shared, worker_cfg) = (state.clone(), cfg.clone());
        std::thread::Builder::new().name("autopod".into()).spawn(move || run(worker_cfg, shared)).expect("autopod thread");
        AutoPod { cfg, state }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> AdapterStatus {
        let s = self.lock();
        AdapterStatus {
            name: "AutoPod".into(),
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
            return Some(format!("The AutoPod is not answering at {}", self.cfg.host));
        }
        if let Some(why) = s.calibration.problem() {
            return Some(why.to_string());
        }
        if self.cfg.map.len() < CAL_POINTS_NEEDED {
            return Some(format!(
                "The AutoPod's height map has {} of the {CAL_POINTS_NEEDED} measured heights it needs. Add them in the rig settings",
                self.cfg.map.len()
            ));
        }
        if s.starved {
            return Some("The AutoPod was stopped because the show stopped sending it targets".into());
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

    pub fn drive(&self, cmd: &Drive) {
        let mut s = self.lock();
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
        let step = (|| -> Result<(), String> {
            let raw = link.position()? as f64;
            let read_at = Instant::now() - link.last_round_trip / 2;
            let round_trip_ms = link.last_round_trip.as_secs_f64() * 1000.0;
            // Calibration is asked once a second, and at once after connecting.
            let calibration = if cycle % 20 == 1 || !lock().connected { Some(link.calibration()?) } else { None };
            let mm = height_mm(&cfg.map, raw);
            let (mode, limits, moved) = {
                let mut s = lock();
                let moved = s.connected && (raw - s.raw).abs() > 1.0;
                s.raw = raw;
                s.mm = mm;
                s.read_at = read_at;
                s.connected = true;
                s.round_trip_ms = round_trip_ms;
                s.moving = moved;
                if let Some(c) = calibration {
                    s.calibration = c;
                }
                s.detail = format!("{} answers in {:.1} ms", cfg.host, round_trip_ms);
                if matches!(s.mode, Mode::Drive(_)) && s.told_at.elapsed() > STARVED_AFTER {
                    s.mode = Mode::Stop;
                    s.starved = true;
                }
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
                    }
                }
                Mode::Stop => {
                    link.stop()?;
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
        })();
        if let Err(why) = step {
            let mut s = lock();
            s.connected = false;
            s.moving = false;
            s.detail = why;
            drop(s);
            std::thread::sleep(Duration::from_millis(250));
        }
        pace.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_match_the_companion_module() {
        assert_eq!(request_path("#PTS5075"), "/cgi-bin/aw_ptz?cmd=%23PTS5075&res=1");
        assert_eq!(velocity_command(50), "#PTS5050");
        assert_eq!(velocity_command(7), "#PTS5007");
        assert_eq!(velocity_command(120), "#PTS5099", "the speed byte stays in two digits");
        assert_eq!(STOP, "#PTS5050");
        assert_eq!(speed_byte(0.0), 50);
        assert_eq!(speed_byte(1.0), 99);
        assert_eq!(speed_byte(-1.0), 1);
        assert_eq!(speed_byte(-7.0), 1);
        assert_eq!(speed_byte(0.5), 75);
    }

    #[test]
    fn answers_are_read() {
        assert_eq!(parse_position("aPC7FFF8A3C"), Some(0x8A3C));
        assert_eq!(parse_position("aPC7FFF"), None);
        assert_eq!(parse_position("rER00"), None);
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
