//! On-site bring-up of the real rig. Each step measures something the adapters
//! otherwise have to assume, and the report carries the values to put into the
//! project: count direction and scale, the speed of every VISCA step, the query
//! delay, tolerances, the pedestal's height map and top speed, and whether to
//! steer the head along the curve or let it run plain moves by itself.

use crate::hw::autopod;
use crate::hw::fr7::{self, Fr7, Fr7Config};
use crate::hw::visca::{self, Turn};
use crate::rig::Drive;
use ptz_core::camera::CameraConfig;
use ptz_core::pose::{AxisTable, CalPoint, Pose};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    Connect,
    Moves,
    Timing,
    Speeds,
    Tracking,
    AutoPod,
    Estop,
}

impl Step {
    pub const ALL: [Step; 7] =
        [Step::Connect, Step::Moves, Step::Timing, Step::Speeds, Step::Tracking, Step::AutoPod, Step::Estop];

    pub fn name(self) -> &'static str {
        match self {
            Step::Connect => "connect",
            Step::Moves => "moves",
            Step::Timing => "timing",
            Step::Speeds => "speeds",
            Step::Tracking => "tracking",
            Step::AutoPod => "autopod",
            Step::Estop => "estop",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Step::Connect => "Connect and read positions",
            Step::Moves => "Small moves: direction and scale of each axis",
            Step::Timing => "Query delay and fastest command rate",
            Step::Speeds => "Speed of each pan and tilt step",
            Step::Tracking => "Tracking a 10 degree eased pan",
            Step::AutoPod => "AutoPod+: calibration, height map and top speed",
            Step::Estop => "E-Stop: time to a standstill",
        }
    }

    pub fn parse(name: &str) -> Option<Step> {
        Step::ALL.into_iter().find(|s| s.name() == name.trim())
    }
}

/// Whoever stands at the rig. The tool asks before anything moves and for the
/// things only a person can see or measure.
pub trait Operator {
    fn say(&mut self, line: &str);
    /// A yes or no question. Declining a question that asks to move skips that step.
    fn confirm(&mut self, question: &str) -> bool;
    /// A number someone reads off the rig. `None` takes the default, or skips when there is none.
    fn number(&mut self, question: &str, default: Option<f64>) -> Option<f64>;
}

/// Nobody is at the keyboard: every move goes ahead and every default stands.
pub struct Unattended {
    pub echo: bool,
}

impl Operator for Unattended {
    fn say(&mut self, line: &str) {
        if self.echo {
            println!("{line}");
        }
    }

    fn confirm(&mut self, _question: &str) -> bool {
        true
    }

    fn number(&mut self, _question: &str, default: Option<f64>) -> Option<f64> {
        default
    }
}

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// `host:port` of the FR7's VISCA port and of its web interface, with the web login.
    pub fr7: Option<Fr7Config>,
    /// `host:port` of the AutoPod+ leg box.
    pub autopod: Option<String>,
    /// Steps to run; empty runs them all.
    pub steps: Vec<Step>,
    /// Fewer samples and shorter runs. Good for a first pass; the full run measures every speed step.
    pub quick: bool,
    /// Findings of an earlier run, so a single step can be repeated.
    pub start: Settings,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Section {
    pub step: String,
    pub title: String,
    /// `passed`, `failed` or `skipped`.
    pub outcome: String,
    pub lines: Vec<String>,
}

/// The values a run found, in the shape the project stores them. Anything not measured stays out.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Settings {
    pub fr7_host: Option<String>,
    pub fr7_http_port: Option<u16>,
    pub fr7_user: Option<String>,
    pub autopod_host: Option<String>,
    pub fr7_counts_per_degree: Option<f64>,
    pub fr7_invert_pan: Option<bool>,
    pub fr7_invert_tilt: Option<bool>,
    pub fr7_speed_steps: Option<Vec<f64>>,
    pub fr7_absolute_moves: Option<bool>,
    pub query_delay_ms: Option<f64>,
    pub autopod_calibration: Option<Vec<CalPoint>>,
    pub tolerance_pan: Option<f64>,
    pub tolerance_tilt: Option<f64>,
    pub max_speed_pan: Option<f64>,
    pub max_speed_tilt: Option<f64>,
    pub max_speed_zoom: Option<f64>,
    pub max_speed_focus: Option<f64>,
    pub max_speed_elevation: Option<f64>,
}

impl Settings {
    /// Write the findings into the camera they were measured on. Returns one line per value
    /// that changed. A top speed is only ever lowered: the limits someone set by hand stay the ceiling.
    pub fn apply(&self, camera: &mut CameraConfig) -> Vec<String> {
        let mut changes = Vec::new();
        fn set<T: PartialEq + std::fmt::Debug>(changes: &mut Vec<String>, what: &str, slot: &mut T, value: Option<T>) {
            if let Some(v) = value {
                if *slot != v {
                    changes.push(format!("{what}: {:?} -> {:?}", slot, v));
                    *slot = v;
                }
            }
        }
        let rig = &mut camera.rig;
        set(&mut changes, "FR7 address", &mut rig.fr7_host, self.fr7_host.clone());
        set(&mut changes, "FR7 web port", &mut rig.fr7_http_port, self.fr7_http_port);
        set(&mut changes, "FR7 user", &mut rig.fr7_user, self.fr7_user.clone());
        set(&mut changes, "AutoPod+ address", &mut rig.autopod_host, self.autopod_host.clone());
        set(&mut changes, "Counts per degree", &mut rig.fr7_counts_per_degree, self.fr7_counts_per_degree);
        set(&mut changes, "Pan counts inverted", &mut rig.fr7_invert_pan, self.fr7_invert_pan);
        set(&mut changes, "Tilt counts inverted", &mut rig.fr7_invert_tilt, self.fr7_invert_tilt);
        if let Some(steps) = &self.fr7_speed_steps {
            if rig.fr7_speed_steps != *steps {
                changes.push(format!(
                    "Speed steps: measured, {:.2} to {:.1} degrees per second",
                    steps[0],
                    steps[steps.len() - 1]
                ));
                rig.fr7_speed_steps = steps.clone();
            }
        }
        set(&mut changes, "Plain moves run by the head itself", &mut rig.fr7_absolute_moves, self.fr7_absolute_moves);
        set(&mut changes, "Query delay (ms)", &mut rig.query_delay_ms, self.query_delay_ms);
        if let Some(map) = &self.autopod_calibration {
            if rig.autopod_calibration != *map {
                changes.push(format!("AutoPod+ height map: {} measured heights", map.len()));
                rig.autopod_calibration = map.clone();
            }
        }
        set(&mut changes, "Pan tolerance", &mut rig.tolerance.pan, self.tolerance_pan);
        set(&mut changes, "Tilt tolerance", &mut rig.tolerance.tilt, self.tolerance_tilt);
        let axes = &mut camera.axes;
        let lower = |changes: &mut Vec<String>, what: &str, slot: &mut f64, value: Option<f64>| {
            if let Some(v) = value.filter(|v| *v > 0.0 && *v < *slot) {
                changes.push(format!("{what}: {} -> {}", slot, v));
                *slot = v;
            }
        };
        lower(&mut changes, "Pan top speed", &mut axes.pan.max_speed, self.max_speed_pan);
        lower(&mut changes, "Tilt top speed", &mut axes.tilt.max_speed, self.max_speed_tilt);
        lower(&mut changes, "Zoom top speed", &mut axes.zoom.max_speed, self.max_speed_zoom);
        lower(&mut changes, "Focus top speed", &mut axes.focus.max_speed, self.max_speed_focus);
        lower(&mut changes, "Elevation top speed", &mut axes.elevation.max_speed, self.max_speed_elevation);
        changes
    }
}

/// What was measured, for the record. Everything here is also in the section lines in words.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Measured {
    pub fr7_round_trip_ms: Option<f64>,
    pub fr7_round_trip_p95_ms: Option<f64>,
    pub fr7_commands_per_second: Option<f64>,
    pub autopod_round_trip_ms: Option<f64>,
    /// Direction byte 01 turns the camera to its own left, and tilts it up.
    pub plus_is_left: Option<bool>,
    pub plus_is_up: Option<bool>,
    pub pan_step_speeds: Vec<(u8, f64)>,
    pub tilt_step_speeds: Vec<(u8, f64)>,
    pub velocity_worst_error: Option<f64>,
    pub velocity_rms_error: Option<f64>,
    pub velocity_lag_ms: Option<f64>,
    pub absolute_worst_error: Option<f64>,
    pub absolute_landing_error: Option<f64>,
    pub autopod_model: Option<String>,
    pub autopod_firmware: Option<String>,
    pub autopod_calibration_state: Option<String>,
    pub autopod_up_mm_s: Option<f64>,
    pub autopod_down_mm_s: Option<f64>,
    /// Speed at half command as a share of speed at full command; 0.5 means the speed byte is linear.
    pub autopod_half_speed_share: Option<f64>,
    pub estop_fr7_ms: Option<f64>,
    pub estop_autopod_ms: Option<f64>,
    pub estop_pan_overrun: Option<f64>,
    pub estop_elevation_overrun: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Report {
    pub made_at: String,
    pub tool: String,
    pub quick: bool,
    pub sections: Vec<Section>,
    pub measured: Measured,
    pub settings: Settings,
    /// `velocity tracking` or `absolute moves`, once the tracking step has run.
    pub decision: Option<String>,
    pub problems: Vec<String>,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.sections.iter().any(|s| s.outcome == "failed")
    }

    pub fn section(&self, step: Step) -> Option<&Section> {
        self.sections.iter().find(|s| s.step == step.name())
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
    }

    pub fn load(path: &Path) -> Result<Report, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{} is not a bring-up report: {e}", path.display()))
    }

    /// The end of the run in words: what passed, what to apply, what still needs attention.
    pub fn summary(&self) -> String {
        let mut out = String::from("\nBring-up summary\n");
        for s in &self.sections {
            out.push_str(&format!("  {:<8} {}\n", s.outcome, s.title));
        }
        if let Some(d) = &self.decision {
            out.push_str(&format!("\nDecision: {d}\n"));
        }
        let changes = self.settings.apply(&mut CameraConfig::default());
        if !changes.is_empty() {
            out.push_str("\nAgainst a new camera, applying this report would set:\n");
            for c in changes {
                out.push_str(&format!("  {c}\n"));
            }
        }
        if !self.problems.is_empty() {
            out.push_str("\nNeeds attention:\n");
            for p in &self.problems {
                out.push_str(&format!("  - {p}\n"));
            }
        }
        out
    }
}

enum Halt {
    Failed(String),
    Skipped(String),
}

impl From<String> for Halt {
    fn from(why: String) -> Halt {
        Halt::Failed(sentence(&why))
    }
}

fn sentence(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn round_to(value: f64, step: f64) -> f64 {
    let per_unit = 1.0 / step;
    (value * per_unit).round() / per_unit
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied().unwrap_or(0.0)
}

fn percentile(sorted: &[f64], share: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * share).round() as usize;
    sorted.get(i).copied().unwrap_or(0.0)
}

/// Smoothstep from 0 to 1 over `seconds`, and its slope.
fn ease(t: f64, seconds: f64) -> (f64, f64) {
    let u = (t / seconds).clamp(0.0, 1.0);
    let slope = if t > 0.0 && t < seconds { 6.0 * u * (1.0 - u) / seconds } else { 0.0 };
    (u * u * (3.0 - 2.0 * u), slope)
}

/// Raw counts once the head reads the same four times running.
fn settle(link: &mut fr7::Link, timeout: Duration) -> Result<(i32, i32), String> {
    let deadline = Instant::now() + timeout;
    let mut last = link.read_counts()?;
    let mut same = 0;
    while same < 4 {
        if Instant::now() > deadline {
            return Err("the head was still moving when it should have stopped".into());
        }
        std::thread::sleep(Duration::from_millis(40));
        let now = link.read_counts()?;
        same = if now == last { same + 1 } else { 0 };
        last = now;
    }
    Ok(last)
}

/// Send the head to raw counts at a brisk step and wait for it to arrive.
fn return_to(link: &mut fr7::Link, counts: (i32, i32)) -> Result<(i32, i32), String> {
    let step = visca::step_at_least(link.speed_table(), 20.0);
    link.command(&visca::pan_tilt_absolute(step, step, counts.0, counts.1))?;
    settle(link, Duration::from_secs(20))
}

/// The pedestal's raw position once it reads the same four times running.
fn pod_settle(link: &mut autopod::Link, timeout: Duration) -> Result<u16, String> {
    let deadline = Instant::now() + timeout;
    let mut last = link.position()?;
    let mut same = 0;
    while same < 4 {
        if Instant::now() > deadline {
            return Err("the column was still moving when it should have stopped".into());
        }
        std::thread::sleep(Duration::from_millis(60));
        let now = link.position()?;
        same = if now == last { same + 1 } else { 0 };
        last = now;
    }
    Ok(last)
}

/// Run the column at `share` of full speed and give its speed in raw counts per second
/// over the steady part of the run.
fn pod_run(link: &mut autopod::Link, share: f64, seconds: f64) -> Result<f64, String> {
    link.velocity(share)?;
    std::thread::sleep(Duration::from_secs_f64(seconds * 0.3));
    let a = link.position()? as f64;
    let at_a = Instant::now() - link.last_round_trip / 2;
    std::thread::sleep(Duration::from_secs_f64(seconds * 0.7));
    let b = link.position()? as f64;
    let at_b = Instant::now() - link.last_round_trip / 2;
    link.stop()?;
    pod_settle(link, Duration::from_secs(10))?;
    Ok((b - a) / at_b.duration_since(at_a).as_secs_f64().max(1e-3))
}

/// Bring the column back to a raw position at half speed. `rises` says whether the raw
/// position grows under an upward command.
fn pod_return(link: &mut autopod::Link, home: u16, rises: bool) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(90);
    let first = home as f64 - link.position()? as f64;
    if first.abs() >= 8.0 {
        link.velocity(if (first > 0.0) == rises { 0.5 } else { -0.5 })?;
        // Stop on reaching the start or passing it, whichever the next reading shows.
        loop {
            let off = home as f64 - link.position()? as f64;
            if off.abs() < 8.0 || (off > 0.0) != (first > 0.0) || Instant::now() > deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }
    link.stop()?;
    pod_settle(link, Duration::from_secs(10)).map(|_| ())
}

struct Curve {
    worst: f64,
    rms: f64,
    lag_ms: f64,
}

/// Feed the adapter an eased pan from `from` to `to` degrees and measure how far the head strays.
fn follow_curve(head: &Fr7, base: Pose, from: f64, to: f64, seconds: f64) -> Curve {
    let began = Instant::now();
    let mut asked: Vec<(f64, f64)> = Vec::new();
    let mut read: Vec<(f64, f64)> = Vec::new();
    let mut last_read = None;
    loop {
        let t = began.elapsed().as_secs_f64();
        let (s, slope) = ease(t, seconds);
        let pan = from + (to - from) * s;
        let target = Pose { pan, ..base };
        let velocity = Pose { pan: (to - from) * slope, ..Pose::default() };
        head.drive(&Drive { target, velocity, scale: 1.0, ride: None });
        asked.push((t, pan));
        let (reading, times) = head.head();
        if last_read != Some(times.pan_tilt) {
            last_read = Some(times.pan_tilt);
            if times.pan_tilt >= began {
                read.push((times.pan_tilt.duration_since(began).as_secs_f64(), reading.pan));
            }
        }
        if t > seconds + 1.0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    let asked_at = |t: f64| {
        let i = asked.partition_point(|(at, _)| *at <= t);
        if i == 0 {
            from
        } else {
            asked[i - 1].1
        }
    };
    let errors = |lag: f64| -> (f64, f64) {
        let e: Vec<f64> = read.iter().filter(|(t, _)| *t > 0.3).map(|(t, pan)| pan - asked_at(t - lag)).collect();
        let worst = e.iter().fold(0.0f64, |m, v| m.max(v.abs()));
        let rms = (e.iter().map(|v| v * v).sum::<f64>() / e.len().max(1) as f64).sqrt();
        (worst, rms)
    };
    let (worst, rms) = errors(0.0);
    let lag_ms =
        (0..=30).map(|i| i as f64 * 10.0).min_by(|a, b| errors(a / 1000.0).1.total_cmp(&errors(b / 1000.0).1)).unwrap_or(0.0);
    Curve { worst, rms, lag_ms }
}

struct Run<'a> {
    op: &'a mut dyn Operator,
    opts: &'a Options,
    cfg: Option<Fr7Config>,
    map: Vec<CalPoint>,
    report: Report,
    lines: Vec<String>,
}

impl Run<'_> {
    fn note(&mut self, line: impl Into<String>) {
        let line = line.into();
        self.op.say(&format!("  {line}"));
        self.lines.push(line);
    }

    fn problem(&mut self, line: impl Into<String>) {
        let line = line.into();
        self.note(format!("Needs attention: {line}"));
        self.report.problems.push(line);
    }

    fn go_ahead(&mut self, question: &str) -> Result<(), Halt> {
        if self.op.confirm(question) {
            Ok(())
        } else {
            Err(Halt::Skipped("Declined at the prompt; nothing was moved".into()))
        }
    }

    fn link(&self) -> Result<Option<fr7::Link>, Halt> {
        match &self.cfg {
            Some(cfg) => Ok(Some(fr7::Link::connect(cfg)?)),
            None => Ok(None),
        }
    }

    fn pod(&self) -> Option<autopod::Link> {
        self.opts.autopod.as_deref().map(autopod::Link::new)
    }

    fn connect(&mut self) -> Result<(), Halt> {
        if self.cfg.is_none() && self.opts.autopod.is_none() {
            return Err(Halt::Failed("No device was named. Give --fr7, --autopod or both".into()));
        }
        if let Some(cfg) = self.cfg.clone() {
            let mut link = fr7::Link::connect(&cfg)?;
            let (pan, tilt) = link.read_counts()?;
            let trip = link.last_round_trip.as_secs_f64() * 1000.0;
            let (zoom, focus) = (link.read_zoom()?, link.read_focus()?);
            self.note(format!("FR7 at {} answers in {trip:.1} ms.", cfg.visca));
            self.note(format!("Position: pan {pan} counts, tilt {tilt} counts, zoom {zoom:.3}, focus {focus:.3}."));
            let mut cgi = crate::hw::cgi::Cgi::new(&cfg.http, &cfg.user, &cfg.password, Duration::from_secs(3));
            match fr7::preset_names(&mut cgi) {
                Ok(list) => {
                    let names: Vec<String> = list.iter().map(|(n, name)| format!("{n} {name}")).collect();
                    self.note(format!("Web interface at {} accepts the login. Presets on the camera: {}.", cfg.http, names.join(", ")));
                }
                Err(why) => self.problem(format!(
                    "The FR7's web interface at {} did not give its preset list ({why}). Motion is unaffected; preset sync will not work until it does.",
                    cfg.http
                )),
            }
        }
        if let Some(mut pod) = self.pod() {
            let state = pod.calibration()?;
            let raw = pod.position()?;
            let trip = pod.last_round_trip.as_secs_f64() * 1000.0;
            let identity = pod.identity()?;
            self.note(format!("{} at {} answers in {trip:.1} ms. Raw position {raw}.", identity.label(), pod.host));
            if identity.model.is_none() {
                self.note(
                    "It did not say what model it is (no answer to QID). An original AutoPod speaks the same motion commands.",
                );
            }
            self.report.measured.autopod_model = identity.model.clone();
            self.report.measured.autopod_firmware = identity.firmware.clone();
            match pod.power()? {
                Some(true) => self.note("AutoPod+ is switched on."),
                Some(false) => return Err(Halt::Failed("The AutoPod+ is in standby. Switch it on at the leg box".into())),
                None => self.note("AutoPod+ did not say whether it is on or in standby (no answer to #O)."),
            }
            self.report.measured.autopod_calibration_state = Some(format!("{state:?}"));
            match state.problem() {
                None => self.note("AutoPod+ reports that it is calibrated."),
                Some(why) => self.problem(format!("AutoPod+: {why}")),
            }
        }
        Ok(())
    }

    fn moves(&mut self) -> Result<(), Halt> {
        let Some(mut link) = self.link()? else {
            return Err(Halt::Skipped("No FR7 was named".into()));
        };
        self.go_ahead(
            "This pans the head about 10 degrees to its left, tilts it about 5 degrees up, zooms and focuses a little, and comes back. Is the head clear to move?",
        )?;
        let cfg = self.cfg.clone().expect("a link exists");
        let home = settle(&mut link, Duration::from_secs(5))?;
        let step = visca::step_at_least(link.speed_table(), 5.0);

        link.drive_steps((step, Turn::Plus), (1, Turn::Hold))?;
        std::thread::sleep(Duration::from_secs(1));
        link.stop_all()?;
        let after = settle(&mut link, Duration::from_secs(5))?;
        let moved = after.0 - home.0;
        if moved.abs() < 20 {
            return Err(Halt::Failed(format!(
                "The head did not pan: its position changed by {moved} counts in a second at speed step {step}"
            )));
        }
        let invert_pan = moved < 0;
        self.note(format!(
            "Pan: {moved:+} counts in one second at step {step}. Counts {} in the direction of byte 01.",
            if invert_pan { "fall" } else { "rise" }
        ));
        let left = self.op.confirm("Did the camera turn to its own left (the picture slid to the right)?");
        self.report.measured.plus_is_left = Some(left);
        if !left {
            self.note("Direction byte 01 turned the camera to its right on this head. Motion and presets are unaffected; positive pan in the app is to the right.");
        }
        let back = return_to(&mut link, home)?;
        if (back.0 - home.0).abs() > 3 || (back.1 - home.1).abs() > 3 {
            self.problem(format!(
                "An absolute move back to the start landed {} counts off in pan and {} in tilt.",
                back.0 - home.0,
                back.1 - home.1
            ));
        } else {
            self.note("An absolute move returned the head to its start to the count.");
        }

        link.drive_steps((1, Turn::Hold), (step, Turn::Plus))?;
        std::thread::sleep(Duration::from_secs(1));
        link.stop_all()?;
        let after = settle(&mut link, Duration::from_secs(5))?;
        let moved = after.1 - home.1;
        if moved.abs() < 20 {
            return Err(Halt::Failed(format!(
                "The head did not tilt: its position changed by {moved} counts in a second at speed step {step}"
            )));
        }
        let invert_tilt = moved < 0;
        self.note(format!(
            "Tilt: {moved:+} counts in one second at step {step}. Counts {} in the direction of byte 01.",
            if invert_tilt { "fall" } else { "rise" }
        ));
        let up = self.op.confirm("Did the camera tilt up?");
        self.report.measured.plus_is_up = Some(up);
        if !up {
            self.note("Direction byte 01 tilted the camera down on this head. Motion and presets are unaffected; positive tilt in the app is down.");
        }
        return_to(&mut link, home)?;

        // Scale: a known number of counts, and a person's reading of the angle it made.
        let sent = (10.0 * cfg.counts_per_degree).round() as i32 * if invert_pan { -1 } else { 1 };
        let there = return_to(&mut link, (home.0 + sent, home.1))?;
        let counts = (there.0 - home.0).abs() as f64;
        let seen = self
            .op
            .number(
                &format!(
                    "The head was sent {} counts, which is 10 degrees if {} counts make a degree. How many degrees did it turn?",
                    sent.abs(),
                    cfg.counts_per_degree
                ),
                Some(10.0),
            )
            .unwrap_or(10.0);
        if seen < 0.5 {
            return Err(Halt::Failed("The head has to turn by an angle that can be read; nothing was changed".into()));
        }
        let per_degree = round_to(counts / seen, 0.1);
        self.note(format!("Scale: {counts:.0} counts for {seen} degrees, {per_degree} counts per degree."));
        return_to(&mut link, home)?;

        let mut next = cfg;
        next.counts_per_degree = per_degree;
        next.pan_sign = if invert_pan { -1.0 } else { 1.0 };
        next.tilt_sign = if invert_tilt { -1.0 } else { 1.0 };
        self.report.settings.fr7_counts_per_degree = Some(per_degree);
        self.report.settings.fr7_invert_pan = Some(invert_pan);
        self.report.settings.fr7_invert_tilt = Some(invert_tilt);
        self.cfg = Some(next);

        // Zoom and focus are sent as positions; the camera runs them at its own speed.
        let zoom = link.read_zoom()?;
        let target = if zoom < 0.5 { zoom + 0.15 } else { zoom - 0.15 };
        let began = Instant::now();
        link.zoom_to(target)?;
        let mut reads = zoom;
        while (reads - target).abs() > 0.004 && began.elapsed() < Duration::from_secs(6) {
            std::thread::sleep(Duration::from_millis(30));
            reads = link.read_zoom()?;
        }
        if (reads - target).abs() > 0.01 {
            self.problem(format!("Zoom was sent to {target:.3} and reads {reads:.3}. The zoom scale on this lens is not the one assumed (0 to 0x4000)."));
        } else {
            let speed = 0.15 / began.elapsed().as_secs_f64().max(0.03);
            self.note(format!("Zoom reached the position it was sent to, at {speed:.2} of its range per second."));
            self.report.settings.max_speed_zoom = Some(round_to(speed * 0.9, 0.01).max(0.01));
        }
        link.zoom_to(zoom)?;

        link.command(&visca::focus_manual())?;
        let focus = link.read_focus()?;
        let target = if focus < 0.5 { focus + 0.1 } else { focus - 0.1 };
        let began = Instant::now();
        link.focus_to(target)?;
        let mut reads = focus;
        while (reads - target).abs() > 0.004 && began.elapsed() < Duration::from_secs(6) {
            std::thread::sleep(Duration::from_millis(30));
            reads = link.read_focus()?;
        }
        if (reads - target).abs() > 0.01 {
            self.problem(format!("Focus was sent to {target:.3} and reads {reads:.3}. The focus scale on this lens is not the one assumed (0 to 0xFFFF)."));
        } else {
            let speed = 0.1 / began.elapsed().as_secs_f64().max(0.03);
            self.note(format!("Focus reached the position it was sent to, at {speed:.2} of its range per second. The camera is now in manual focus."));
            self.report.settings.max_speed_focus = Some(round_to(speed * 0.9, 0.01).max(0.01));
        }
        link.focus_to(focus)?;
        std::thread::sleep(Duration::from_millis(600));
        Ok(())
    }

    fn timing(&mut self) -> Result<(), Halt> {
        let (queries, commands) = if self.opts.quick { (60, 30) } else { (200, 100) };
        let mut delay = None;
        if let Some(mut link) = self.link()? {
            let mut trips = Vec::with_capacity(queries);
            for _ in 0..queries {
                link.read_counts()?;
                trips.push(link.last_round_trip.as_secs_f64() * 1000.0);
            }
            let mid = median(&mut trips);
            let p95 = percentile(&trips, 0.95);
            let worst = trips[trips.len() - 1];
            self.note(format!("FR7 position query: {mid:.1} ms typical, {p95:.1} ms for 95 in 100, {worst:.1} ms at worst, over {queries} queries."));
            // A stop for an axis that is already stopped: a real command that moves nothing.
            let began = Instant::now();
            let mut refused = 0;
            for _ in 0..commands {
                match link.command(&visca::pan_tilt_stop()) {
                    Ok(()) => {}
                    Err(why) if why.contains("refused") => refused += 1,
                    Err(why) => return Err(why.into()),
                }
            }
            let each = began.elapsed().as_secs_f64() * 1000.0 / commands as f64;
            let rate = 1000.0 / each.max(1e-3);
            self.note(format!("FR7 commands back to back: {rate:.0} a second ({each:.1} ms each), {refused} refused."));
            if refused > 0 {
                self.problem(format!(
                    "The FR7 refused {refused} of {commands} commands sent back to back. Its command buffer fills at this rate."
                ));
            }
            let cycle = 2.0 * p95 + 3.0 * each;
            if cycle > 40.0 {
                self.problem(format!(
                    "One 20 Hz cycle is two position queries and up to three commands. On this network that takes {cycle:.0} ms of the 50 available."
                ));
            } else {
                self.note(format!(
                    "One 20 Hz cycle (two queries, up to three commands) takes {cycle:.0} ms of the 50 available."
                ));
            }
            self.report.measured.fr7_round_trip_ms = Some(round_to(mid, 0.1));
            self.report.measured.fr7_round_trip_p95_ms = Some(round_to(p95, 0.1));
            self.report.measured.fr7_commands_per_second = Some(rate.round());
            delay = Some(mid / 2.0);
        }
        if let Some(mut pod) = self.pod() {
            let mut trips = Vec::new();
            for _ in 0..queries.min(40) {
                pod.position()?;
                trips.push(pod.last_round_trip.as_secs_f64() * 1000.0);
            }
            let mid = median(&mut trips);
            let p95 = percentile(&trips, 0.95);
            self.note(format!("AutoPod+ position query: {mid:.1} ms typical, {p95:.1} ms for 95 in 100."));
            if 2.0 * p95 > 40.0 {
                self.problem(format!(
                    "The AutoPod+ takes {p95:.0} ms to answer. A query and a command do not fit in a 50 ms cycle."
                ));
            }
            self.report.measured.autopod_round_trip_ms = Some(round_to(mid, 0.1));
            delay = Some(delay.unwrap_or(0.0).max(mid / 2.0));
        }
        match delay {
            Some(ms) => {
                let ms = round_to(ms, 0.1).max(0.1);
                self.note(format!("Query delay: a position is {ms} ms old when it arrives."));
                self.report.settings.query_delay_ms = Some(ms);
                Ok(())
            }
            None => Err(Halt::Skipped("No device was named".into())),
        }
    }

    fn speeds(&mut self) -> Result<(), Halt> {
        let Some(mut link) = self.link()? else {
            return Err(Halt::Skipped("No FR7 was named".into()));
        };
        self.go_ahead(
            "This runs the head at each speed step in turn, up to 35 degrees to its left and up from where it is now, and brings it back each time. Is the head clear to move?",
        )?;
        let cfg = self.cfg.clone().expect("a link exists");
        let assumed = visca::speed_table();
        let steps: Vec<u8> = if self.opts.quick { vec![4, 9, 14, 19, visca::TOP_STEP] } else { (1..=visca::TOP_STEP).collect() };
        let home = settle(&mut link, Duration::from_secs(5))?;
        let mut pan_speeds = Vec::new();
        let mut tilt_speeds = Vec::new();
        for step in &steps {
            let guess = assumed[*step as usize - 1];
            let run = (20.0 / guess).clamp(0.5, 2.0);
            link.drive_steps((*step, Turn::Plus), (*step, Turn::Plus))?;
            std::thread::sleep(Duration::from_secs_f64(0.2));
            let a = link.read_counts()?;
            let at_a = Instant::now() - link.last_round_trip / 2;
            std::thread::sleep(Duration::from_secs_f64(run - 0.2));
            let b = link.read_counts()?;
            let at_b = Instant::now() - link.last_round_trip / 2;
            link.stop_all()?;
            let seconds = at_b.duration_since(at_a).as_secs_f64().max(1e-3);
            let pan = (b.0 - a.0).abs() as f64 / cfg.counts_per_degree / seconds;
            let tilt = (b.1 - a.1).abs() as f64 / cfg.counts_per_degree / seconds;
            self.note(format!("Step {step:>2}: pan {pan:.3} and tilt {tilt:.3} degrees per second (expected about {guess:.3})."));
            pan_speeds.push((*step, pan));
            tilt_speeds.push((*step, tilt));
            settle(&mut link, Duration::from_secs(5))?;
            return_to(&mut link, home)?;
        }

        // One table serves both axes. Steps not run are filled in between their neighbours by ratio.
        let measured: Vec<(u8, f64)> =
            pan_speeds.iter().zip(&tilt_speeds).map(|((s, p), (_, t))| (*s, (p + t) / 2.0)).filter(|(_, v)| *v > 0.0).collect();
        if measured.len() < 2 {
            return Err(Halt::Failed("The head did not move at enough speed steps to build a table".into()));
        }
        let mut table = Vec::with_capacity(assumed.len());
        for i in 0..assumed.len() {
            let step = i as u8 + 1;
            let above = measured.partition_point(|(s, _)| *s < step);
            let value = if above < measured.len() && measured[above].0 == step {
                measured[above].1
            } else if above == 0 {
                assumed[i] * measured[0].1 / assumed[measured[0].0 as usize - 1]
            } else if above == measured.len() {
                let last = measured[measured.len() - 1];
                assumed[i] * last.1 / assumed[last.0 as usize - 1]
            } else {
                let (lo, hi) = (measured[above - 1], measured[above]);
                let share = (step - lo.0) as f64 / (hi.0 - lo.0) as f64;
                lo.1 * (hi.1 / lo.1).powf(share)
            };
            let floor = table.last().copied().unwrap_or(0.0);
            table.push(value.max(floor));
        }
        let differ = pan_speeds
            .iter()
            .zip(&tilt_speeds)
            .filter(|((_, p), (_, t))| *p > 0.5 && (p - t).abs() / p > 0.15)
            .map(|((s, _), _)| s.to_string())
            .collect::<Vec<_>>();
        if !differ.is_empty() {
            self.problem(format!(
                "Pan and tilt run at different speeds at step {}. The adapter uses one table for both; expect a larger tilt error.",
                differ.join(", ")
            ));
        }
        let top = table[table.len() - 1];
        self.note(format!(
            "Speed table: {:.3} to {top:.1} degrees per second. The fastest steps are measured over a short run and may read low.",
            table[0]
        ));
        self.report.measured.pan_step_speeds = pan_speeds;
        self.report.measured.tilt_step_speeds = tilt_speeds;
        self.report.settings.max_speed_pan = Some(round_to(top * 0.9, 0.1));
        self.report.settings.max_speed_tilt = Some(round_to(top * 0.9, 0.1));
        self.report.settings.fr7_speed_steps = Some(table.clone());
        if let Some(cfg) = &mut self.cfg {
            cfg.speed_steps = table;
        }
        Ok(())
    }

    fn tracking(&mut self) -> Result<(), Halt> {
        let Some(cfg) = self.cfg.clone() else {
            return Err(Halt::Skipped("No FR7 was named".into()));
        };
        self.go_ahead(
            "This pans the head 10 degrees to its left and back twice: once steered along an eased curve, once as a plain move the head runs by itself. Is the head clear to move?",
        )?;
        let seconds = if self.opts.quick { 2.5 } else { 4.0 };

        let mut axes = AxisTable::default();
        if let Some(top) = cfg.speed_steps.last() {
            axes.pan.max_speed = *top;
            axes.tilt.max_speed = *top;
        }
        let head = Fr7::start(cfg.clone(), axes);
        let deadline = Instant::now() + Duration::from_secs(3);
        while !head.status().connected {
            if Instant::now() > deadline {
                return Err(Halt::Failed(head.status().detail));
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let (at, _) = head.head();
        let base = Pose { pan: at.pan, tilt: at.tilt, zoom: at.zoom, focus: at.focus, elevation: 0.0 };
        let out = follow_curve(&head, base, at.pan, at.pan + 10.0, seconds);
        let back = follow_curve(&head, base, at.pan + 10.0, at.pan, seconds);
        head.stop();
        let deadline = Instant::now() + Duration::from_secs(5);
        while head.is_moving() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(head);
        let worst = out.worst.max(back.worst);
        let rms = out.rms.max(back.rms);
        let lag = (out.lag_ms + back.lag_ms) / 2.0;
        self.note(format!(
            "Steered along the curve: at most {worst:.2} degrees from it, {rms:.2} typical, following about {lag:.0} ms behind."
        ));
        self.report.measured.velocity_worst_error = Some(round_to(worst, 0.01));
        self.report.measured.velocity_rms_error = Some(round_to(rms, 0.01));
        self.report.measured.velocity_lag_ms = Some(lag);

        let mut link = fr7::Link::connect(&cfg)?;
        let home = settle(&mut link, Duration::from_secs(5))?;
        let (pan, tilt) = link.read_pan_tilt()?;
        let began = Instant::now();
        link.go_to((pan, tilt), (pan + 10.0, tilt), seconds)?;
        let mut stray = 0.0f64;
        let mut arrived = None;
        while began.elapsed().as_secs_f64() < seconds * 3.0 + 2.0 {
            let (now, _) = link.read_pan_tilt()?;
            let t = began.elapsed().as_secs_f64();
            stray = stray.max((now - (pan + 10.0 * ease(t, seconds).0)).abs());
            if (now - (pan + 10.0)).abs() < 0.05 {
                arrived = Some(t);
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        let (landed, _) = link.read_pan_tilt()?;
        let landing = (landed - (pan + 10.0)).abs();
        match arrived {
            Some(t) => self.note(format!(
                "As a plain move: arrived in {t:.1} s of the {seconds:.1} asked for, at most {stray:.2} degrees from the curve on the way, {landing:.2} from the end pose."
            )),
            None => self.note(format!("As a plain move: {landing:.2} degrees short of the end pose when the wait ran out.")),
        }
        return_to(&mut link, home)?;
        self.report.measured.absolute_worst_error = Some(round_to(stray, 0.01));
        self.report.measured.absolute_landing_error = Some(round_to(landing, 0.01));

        let steered = worst <= 1.0;
        let plain = landing <= 0.2;
        if steered {
            self.note("Decision: velocity tracking. The head follows a curve closely enough to steer it.");
            self.report.decision = Some("velocity tracking".into());
            self.report.settings.fr7_absolute_moves = Some(false);
        } else if plain {
            self.note("Decision: absolute moves. Steering strays too far, so plain preset moves are run by the head itself. Moves with keys are still steered.");
            self.report.decision = Some("absolute moves".into());
            self.report.settings.fr7_absolute_moves = Some(true);
        } else {
            self.problem(format!(
                "Neither way of moving is good enough: steering strays {worst:.2} degrees and a plain move lands {landing:.2} degrees off."
            ));
        }
        let tolerance = ((worst * 2.5 / 0.5).ceil() * 0.5).max(1.0);
        self.note(format!("Suggested pan and tilt tolerance: {tolerance:.1} degrees."));
        self.report.settings.tolerance_pan = Some(tolerance);
        self.report.settings.tolerance_tilt = Some(tolerance);
        Ok(())
    }

    fn autopod(&mut self) -> Result<(), Halt> {
        let Some(mut pod) = self.pod() else {
            return Err(Halt::Skipped("No AutoPod+ was named".into()));
        };
        let state = pod.calibration()?;
        self.report.measured.autopod_calibration_state = Some(format!("{state:?}"));
        if let Some(why) = state.problem() {
            return Err(Halt::Failed(format!("AutoPod+: {why}")));
        }
        self.note("AutoPod+ reports that it is calibrated.");
        self.go_ahead(
            "This raises the column in three stages of a few seconds, runs it up and down at full speed for about two seconds each way, and brings it back. Is the column clear to move, with room above?",
        )?;
        let home = pod_settle(&mut pod, Duration::from_secs(5))?;

        // Three heights someone measures, each with the raw position the leg box reports there.
        let mut map: Vec<CalPoint> = Vec::new();
        let stage = if self.opts.quick { 1.5 } else { 3.0 };
        for i in 0..autopod::CAL_POINTS_NEEDED {
            if i > 0 {
                pod.velocity(0.5)?;
                std::thread::sleep(Duration::from_secs_f64(stage));
                pod.stop()?;
            }
            let raw = pod_settle(&mut pod, Duration::from_secs(10))?;
            if i == 1 && (raw as i32 - home as i32).abs() < 5 {
                return Err(Halt::Failed(format!(
                    "The column did not move: its raw position stayed at {raw} after {stage} s at half speed"
                )));
            }
            let asked =
                format!("Height {} of {}: measure the column's height now, in millimetres.", i + 1, autopod::CAL_POINTS_NEEDED);
            match self.op.number(&asked, None) {
                Some(mm) => {
                    self.note(format!("Height {}: raw {raw} is {mm} mm.", i + 1));
                    map.push(CalPoint { raw: raw as f64, mm });
                }
                None => {
                    map.clear();
                    self.note("Height map: not taken. It needs someone to measure the column at three heights; run this step at the rig, or add heights in the app's rig settings.");
                    break;
                }
            }
        }
        if map.len() == autopod::CAL_POINTS_NEEDED {
            let rising = map.windows(2).all(|w| (w[1].mm - w[0].mm) * (w[1].raw - w[0].raw) > 0.0);
            if rising {
                self.report.settings.autopod_calibration = Some(map.clone());
                self.map = map;
            } else {
                self.problem("The three heights do not rise with the raw position. Measure them again.");
            }
        }

        let run = if self.opts.quick { 1.2 } else { 2.0 };
        let up = pod_run(&mut pod, 1.0, run)?;
        let down = pod_run(&mut pod, -1.0, run)?;
        let half = pod_run(&mut pod, 0.5, run)?;
        if up.abs() < 1.0 || down.abs() < 1.0 {
            return Err(Halt::Failed("The column did not move at full speed".into()));
        }
        let share = half / up;
        self.report.measured.autopod_half_speed_share = Some(round_to(share, 0.01));
        if (share - 0.5).abs() > 0.1 {
            self.problem(format!(
                "At half command the column runs at {:.0}% of full speed, not 50%. The speed byte is not linear; expect a larger elevation error.",
                share * 100.0
            ));
        } else {
            self.note(format!(
                "At half command the column runs at {:.0}% of full speed: the speed byte is linear.",
                share * 100.0
            ));
        }
        // Raw counts per millimetre from the map, where there is one.
        let per_mm = match (self.map.first(), self.map.last()) {
            (Some(a), Some(b)) if a.raw != b.raw => Some(((b.raw - a.raw) / (b.mm - a.mm)).abs()),
            _ => None,
        };
        match per_mm {
            Some(per_mm) => {
                let (up, down) = (up.abs() / per_mm, down.abs() / per_mm);
                self.note(format!("Top speed: {up:.1} mm/s up, {down:.1} mm/s down."));
                self.report.measured.autopod_up_mm_s = Some(round_to(up, 0.1));
                self.report.measured.autopod_down_mm_s = Some(round_to(down, 0.1));
                self.report.settings.max_speed_elevation = Some(round_to(up.min(down) * 0.9, 0.1));
            }
            None => self.note(format!(
                "Top speed: {:.0} raw counts a second up, {:.0} down. Without a height map this cannot be given in mm/s.",
                up.abs(),
                down.abs()
            )),
        }

        pod_return(&mut pod, home, up > 0.0)?;
        Ok(())
    }

    fn estop(&mut self) -> Result<(), Halt> {
        let mut link = self.link()?;
        let mut pod = self.pod();
        if link.is_none() && pod.is_none() {
            return Err(Halt::Skipped("No device was named".into()));
        }
        self.go_ahead(
            "This starts the head panning at about 10 degrees per second and the column rising at half speed, stops both after a moment, and brings them back. Is the rig clear to move?",
        )?;
        let cfg = self.cfg.clone();
        let home = match &mut link {
            Some(l) => Some(settle(l, Duration::from_secs(5))?),
            None => None,
        };
        let pod_home = match &mut pod {
            Some(p) => Some(pod_settle(p, Duration::from_secs(5))?),
            None => None,
        };
        if let Some(l) = &mut link {
            let step = visca::step_at_least(l.speed_table(), 10.0);
            l.drive_steps((step, Turn::Plus), (1, Turn::Hold))?;
        }
        if let Some(p) = &mut pod {
            p.velocity(0.5)?;
        }
        std::thread::sleep(Duration::from_millis(700));
        let mut head_at = match &mut link {
            Some(l) => Some(l.read_counts()?),
            None => None,
        };
        let mut column_at = match &mut pod {
            Some(p) => Some(p.position()?),
            None => None,
        };
        let (head_began, column_began) = (head_at, column_at);

        let pressed = Instant::now();
        let (mut head_still, mut column_still) = (0, 0);
        let (mut head_moved, mut column_moved) = (Duration::ZERO, Duration::ZERO);
        // Stop is sent on every pass until both read the same three times running.
        loop {
            if let Some(l) = &mut link {
                l.stop_all()?;
                let now = l.read_counts()?;
                if Some(now) == head_at {
                    head_still += 1;
                } else {
                    head_still = 0;
                    head_moved = pressed.elapsed();
                }
                head_at = Some(now);
            }
            if let Some(p) = &mut pod {
                p.stop()?;
                let now = p.position()?;
                if Some(now) == column_at {
                    column_still += 1;
                } else {
                    column_still = 0;
                    column_moved = pressed.elapsed();
                }
                column_at = Some(now);
            }
            if (link.is_none() || head_still >= 3) && (pod.is_none() || column_still >= 3) {
                break;
            }
            if pressed.elapsed() > Duration::from_secs(5) {
                return Err(Halt::Failed("The rig was still moving five seconds after stop was sent".into()));
            }
        }
        if let (Some(cfg), Some(began), Some(ended)) = (&cfg, head_began, head_at) {
            let ms = head_moved.as_secs_f64() * 1000.0;
            let overrun = (ended.0 - began.0).abs() as f64 / cfg.counts_per_degree;
            self.note(format!("FR7: standing still {ms:.0} ms after stop was sent, {overrun:.2} degrees further on."));
            self.report.measured.estop_fr7_ms = Some(ms.round());
            self.report.measured.estop_pan_overrun = Some(round_to(overrun, 0.01));
            if ms > 1000.0 {
                self.problem(format!("The FR7 takes {ms:.0} ms to stop from 10 degrees per second."));
            }
        }
        if let (Some(began), Some(ended)) = (column_began, column_at) {
            let ms = column_moved.as_secs_f64() * 1000.0;
            let counts = (ended as f64 - began as f64).abs();
            let per_mm = match (self.map.first(), self.map.last()) {
                (Some(a), Some(b)) if a.raw != b.raw => Some(((b.raw - a.raw) / (b.mm - a.mm)).abs()),
                _ => None,
            };
            match per_mm {
                Some(per_mm) => {
                    self.note(format!(
                        "AutoPod+: standing still {ms:.0} ms after stop was sent, {:.1} mm further on.",
                        counts / per_mm
                    ));
                    self.report.measured.estop_elevation_overrun = Some(round_to(counts / per_mm, 0.1));
                }
                None => self
                    .note(format!("AutoPod+: standing still {ms:.0} ms after stop was sent, {counts:.0} raw counts further on.")),
            }
            self.report.measured.estop_autopod_ms = Some(ms.round());
            if ms > 1000.0 {
                self.problem(format!("The AutoPod+ takes {ms:.0} ms to stop from half speed."));
            }
        }

        if let (Some(l), Some(home)) = (&mut link, home) {
            return_to(l, home)?;
        }
        if let (Some(p), Some(home), Some(began)) = (&mut pod, pod_home, column_began) {
            pod_return(p, home, began > home)?;
        }
        Ok(())
    }
}

/// Run the checklist. Every step that moves the rig asks first, stops what it started
/// if it fails, and leaves the rig where it found it.
pub fn run(opts: &Options, op: &mut dyn Operator) -> Report {
    let mut cfg = opts.fr7.clone();
    if let Some(cfg) = &mut cfg {
        let s = &opts.start;
        if let Some(v) = s.fr7_counts_per_degree {
            cfg.counts_per_degree = v;
        }
        if let Some(v) = s.fr7_invert_pan {
            cfg.pan_sign = if v { -1.0 } else { 1.0 };
        }
        if let Some(v) = s.fr7_invert_tilt {
            cfg.tilt_sign = if v { -1.0 } else { 1.0 };
        }
        if let Some(v) = &s.fr7_speed_steps {
            cfg.speed_steps = v.clone();
        }
    }
    let mut settings = opts.start.clone();
    if let Some(cfg) = &opts.fr7 {
        settings.fr7_host = Some(cfg.visca.clone());
        settings.fr7_http_port = cfg.http.rsplit_once(':').and_then(|(_, p)| p.parse().ok());
        settings.fr7_user = Some(cfg.user.clone());
    }
    if let Some(host) = &opts.autopod {
        settings.autopod_host = Some(host.clone());
    }
    let report = Report {
        made_at: crate::storage::now_rfc3339(),
        tool: format!("ptz-bringup {}", env!("CARGO_PKG_VERSION")),
        quick: opts.quick,
        settings,
        ..Report::default()
    };
    let map = opts.start.autopod_calibration.clone().unwrap_or_default();
    let mut run = Run { op, opts, cfg, map, report, lines: Vec::new() };

    let steps: Vec<Step> = if opts.steps.is_empty() { Step::ALL.to_vec() } else { opts.steps.clone() };
    let mut unreachable = None;
    for step in steps {
        run.op.say(&format!("\n{}", step.title()));
        run.lines.clear();
        let result = match (&unreachable, step) {
            (Some(why), _) => Err(Halt::Skipped(format!("Skipped because connecting failed: {why}"))),
            (None, Step::Connect) => run.connect(),
            (None, Step::Moves) => run.moves(),
            (None, Step::Timing) => run.timing(),
            (None, Step::Speeds) => run.speeds(),
            (None, Step::Tracking) => run.tracking(),
            (None, Step::AutoPod) => run.autopod(),
            (None, Step::Estop) => run.estop(),
        };
        let outcome = match result {
            Ok(()) => "passed",
            Err(Halt::Skipped(why)) => {
                run.note(why);
                "skipped"
            }
            Err(Halt::Failed(why)) => {
                // Whatever the step had started is stopped before anything else is tried.
                run.stop_everything();
                run.note(format!("Failed: {why}"));
                run.report.problems.push(format!("{}: {why}", step.title()));
                if step == Step::Connect {
                    unreachable = Some(why);
                }
                "failed"
            }
        };
        let lines = std::mem::take(&mut run.lines);
        run.report.sections.push(Section {
            step: step.name().into(),
            title: step.title().into(),
            outcome: outcome.into(),
            lines,
        });
    }
    run.report
}

/// Stop every device a run was given, for a run that is being ended where it stands: Ctrl-C,
/// a closed terminal, `kill`. Nothing but stops goes to the devices from this process afterwards,
/// and the stops are repeated in case a command that was already on its way lands after the first.
pub fn halt(opts: &Options) {
    crate::hw::stops_only();
    for _ in 0..4 {
        if let Some(cfg) = &opts.fr7 {
            match fr7::Link::connect(cfg) {
                Ok(mut link) => {
                    let _ = link.stop_all();
                }
                Err(_) => fr7::stop_unheard(cfg),
            }
        }
        if let Some(host) = &opts.autopod {
            let _ = autopod::Link::new(host).stop();
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

impl Run<'_> {
    fn stop_everything(&mut self) {
        if let Some(cfg) = &self.cfg {
            if let Ok(mut link) = fr7::Link::connect(cfg) {
                let _ = link.stop_all();
            }
        }
        if let Some(mut pod) = self.pod() {
            let _ = pod.stop();
        }
    }
}
