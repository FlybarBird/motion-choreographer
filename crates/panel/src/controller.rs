//! The panel as a station of the show: a client of the show server exactly as
//! the app is, holding the edit lease under its own name. Everything here runs
//! on one thread, fed by one queue. E-Stop and the joystick do not wait in that
//! queue; see `link`.
//!
//! One rule keeps the displays honest: what a key shows and what a key does
//! both come from the same `Face`, worked out from the state at that moment.

use crate::codec::{Color, Lit, Text, Trigger};
use crate::link::{Cell, Change, Event, Shared, REST_FIRST};
use crate::map::{Mode, PanelMap, Role, CAMERA_KEYS, SOFT_ENCODERS, SOFT_KEYS};
use crate::topology::Topology;
use ptz_core::api::{
    CapturePreset, Captured, MediaInfo, MediaKind, PanelState, PutWorking, Refit, SongSummary, Status, StorePreset,
    TransportRequest, WorkingSong,
};
use ptz_core::client::{Api, ApiErr};
use ptz_core::edit::{self, Refusal};
use ptz_core::pose::{Axis, Preset, Project};
use ptz_core::song::{ClipBody, Ease, MotionClip, Song, Track};
use ptz_core::time::Timecode;
use ptz_core::transport::{Action, Output, Run};
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// What the app writes on its Arm button.
pub const ARM_LABEL: &str = "Arm (moves rig to playhead)";
pub const HOLD: Duration = Duration::from_millis(1000);
pub const HOLD_RESET: Duration = Duration::from_millis(1500);
/// A Back or Forward key let go within this time is a tap: a jump. Held longer, it winds.
pub const TAP: Duration = Duration::from_millis(300);
/// How far a tap jumps.
pub const JUMP_SECONDS: f64 = 5.0;
/// Winding speed against real time, and what it becomes once the key has been down for a while.
pub const WIND: f64 = 8.0;
pub const WIND_FAST: f64 = 24.0;
const WIND_FAST_AFTER: Duration = Duration::from_secs(3);
/// With Shift a held key steps this many frames a second.
pub const FRAME_STEPS_A_SECOND: f64 = 10.0;
const WIND_EVERY: Duration = Duration::from_millis(100);
const ARMED_PROGRAM: &str = "Disarm to program. Moving the playhead while armed moves the rig";
const MESSAGE_MIN: Duration = Duration::from_secs(4);
const PAGE_TIME: Duration = Duration::from_millis(1500);
const CONFIRM_TIME: Duration = Duration::from_secs(10);
/// Characters per line: a 128 px tile and a 64 px tile in the small font.
const WIDE: usize = 21;
const NARROW: usize = 10;
const FEWER_KEYS: f64 = 3.0;
const MORE_KEYS: f64 = 0.35;

const LOCK_SOURCES: [(&str, &str); 3] = [("ltc", "LTC input"), ("mtc", "MTC input"), ("artnet", "Art-Net timecode")];

pub struct Settings {
    pub api: Api,
    pub station: String,
    pub map: PanelMap,
    pub lease_grace: Duration,
    pub quiet: bool,
}

#[derive(Clone, PartialEq, Debug)]
enum Act {
    None,
    /// Nothing can be done here; pressing says why.
    Say(String),
    Mode(Mode),
    Camera(usize),
    Page(i32),
    Confirm,
    Cancel,
    Back,
    LeaseTake,
    LeaseRelease,
    LeaseTakeOver,
    NewSong,
    SongList,
    LoadSong(String),
    Save,
    Commit,
    SaveCopy,
    Revert,
    Transport(Action),
    Lock,
    RecordToggle,
    StorePreset,
    PlacePreset(String),
    DeletePreset(String),
    SyncPresets,
    PrevClip,
    NextClip,
    CycleEase,
    AutoTime,
    Duplicate,
    DeleteClip,
    Undo,
    Redo,
    ToStart,
    ToEnd,
    ToKeys,
    Accept,
    Withdraw,
    AddMarker,
    DeleteMarker,
    ToggleAxis(Axis),
    Refit(f64),
    /// Back (-1) or Forward (1): a tap jumps, a hold winds.
    Wind(i64),
    /// Program mode's camera key: the first press captures the pose, the second places the move.
    MarkCamera(usize),
    CancelMark,
    PlaceMedia(String),
    NextMediaClip,
    Mute,
    DeleteMediaClip,
    LockToFile,
}

/// One key: what it shows and what it does.
#[derive(Clone, Debug)]
struct Face {
    title: String,
    line1: String,
    line2: String,
    color: Color,
    lit: Lit,
    act: Act,
    /// The key must be held this long, and the text says what will then happen.
    hold: Option<(Duration, String)>,
}

impl Face {
    fn dark() -> Face {
        Face {
            title: String::new(),
            line1: String::new(),
            line2: String::new(),
            color: Color::Default,
            lit: Lit::Off,
            act: Act::None,
            hold: None,
        }
    }

    /// A dark key that explains itself when pressed.
    fn why(text: impl Into<String>) -> Face {
        Face { act: Act::Say(text.into()), ..Face::dark() }
    }

    /// An available action: a dim white key.
    fn new(title: &str, text: &str, act: Act) -> Face {
        let (line1, line2) = two_lines(text, NARROW);
        Face { title: title.to_string(), line1, line2, color: Color::White, lit: Lit::Dim, act, hold: None }
    }

    fn color(mut self, color: Color) -> Face {
        self.color = color;
        self
    }

    fn lit(mut self, lit: Lit) -> Face {
        self.lit = lit;
        self
    }

    fn on(self, color: Color) -> Face {
        self.color(color).lit(Lit::On)
    }

    /// A key that must be held says so in its title.
    fn hold(mut self, time: Duration, text: impl Into<String>) -> Face {
        self.title = "Hold".into();
        self.hold = Some((time, text.into()));
        self
    }

    fn text(&self) -> Text {
        Text { title: self.title.clone(), label: true, line1: self.line1.clone(), line2: self.line2.clone(), small: true }
    }

    fn name(&self) -> String {
        [self.line1.as_str(), self.line2.as_str()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Knob {
    None,
    LockSource,
    Focus,
    Elevation,
    TopSpeed,
    MoveClip,
    Length,
    Edge,
    Marker,
}

struct KnobFace {
    title: String,
    value: String,
    knob: Knob,
}

impl KnobFace {
    fn dark() -> KnobFace {
        KnobFace { title: String::new(), value: String::new(), knob: Knob::None }
    }

    fn new(title: &str, value: impl Into<String>, knob: Knob) -> KnobFace {
        KnobFace { title: title.to_string(), value: value.into(), knob }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Note,
    Refused,
}

struct Message {
    kind: Kind,
    text: String,
    since: Instant,
}

struct Holding {
    role: Role,
    since: Instant,
    need: Duration,
    act: Act,
    text: String,
    name: String,
    fired: bool,
}

/// A Back or Forward key that is down.
struct Wind {
    role: Role,
    dir: i64,
    /// Shift was down when it was pressed: single frames.
    by_frame: bool,
    since: Instant,
    /// It has been down longer than a tap, and the panel has been heard from since.
    started: bool,
    stepped: Instant,
    /// Where the winding has got to, in frames.
    at: f64,
    sent: i64,
}

/// A camera's first press in Program mode: where it stood and where the playhead was.
#[derive(Clone)]
struct Mark {
    preset: Preset,
    frame: i64,
    /// That press stored the preset; cancelling the mark removes it again.
    made_new: bool,
    at: Instant,
}

struct Asking {
    question: String,
    act: Act,
    until: Instant,
}

pub struct Controller {
    api: Api,
    shared: Arc<Shared>,
    map: PanelMap,
    station: String,
    lease_grace: Duration,
    quiet: bool,

    server_up: bool,
    server_error: String,
    panel_up: bool,
    panel_model: String,
    panel_name: String,
    panel_lost: Option<Instant>,
    grace_spent: bool,
    map_problems: Vec<String>,

    status: Option<Status>,
    project: Project,
    presets: Vec<Preset>,
    media: Vec<MediaInfo>,
    songs: Vec<SongSummary>,
    song: Option<WorkingSong>,
    undo: Vec<Song>,
    redo: Vec<Song>,
    /// The last edit, so a run of knob steps on one clip is one undo step.
    run: Option<(String, Instant)>,

    mode: Mode,
    shift: bool,
    camera: String,
    page: usize,
    song_list: bool,
    clip: Option<String>,
    media_clip: Option<String>,
    lock_source: usize,
    record_axes: Vec<Axis>,
    speed: f64,

    seek_to: Option<i64>,
    last_turn: Option<Instant>,
    message: Option<Message>,
    holding: Option<Holding>,
    asking: Option<Asking>,
    wind: Option<Wind>,
    /// Cameras marked in Program mode and waiting for their second press, by camera id.
    marks: BTreeMap<String, Mark>,
    /// E-Stop is down; true when it was already latched at the press, so the hold may reset it.
    estop_down: Option<(Instant, bool, bool)>,
    estop_waiting: bool,
    /// When the panel last said anything.
    heard: Instant,
    /// The last press found E-Stop not yet latched, so its answer is news.
    estop_fresh: bool,
    /// When the server was last asked whether it is back.
    server_asked: Instant,
    stick_hint: Option<Instant>,

    started: Instant,
    last_report: Option<(Instant, PanelState)>,
    rendered: BTreeMap<u32, Cell>,
}

// ---- text helpers ----

fn fit(s: &str, width: usize) -> String {
    s.chars().take(width).collect()
}

/// Break `text` into lines of at most `width`, at spaces where it can.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let mut word = word.to_string();
        loop {
            let room = if line.is_empty() { width } else { width.saturating_sub(line.chars().count() + 1) };
            if word.chars().count() <= room {
                if !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&word);
                break;
            }
            if line.is_empty() {
                let head: String = word.chars().take(width).collect();
                word = word.chars().skip(width).collect();
                lines.push(head);
            } else {
                lines.push(std::mem::take(&mut line));
            }
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn two_lines(text: &str, width: usize) -> (String, String) {
    let mut lines = wrap(text, width).into_iter();
    (lines.next().unwrap_or_default(), lines.next().unwrap_or_default())
}

fn hold_time(time: Duration) -> String {
    if time == Duration::from_secs(1) {
        "1 second".into()
    } else {
        format!("{:.1} seconds", time.as_secs_f64())
    }
}

fn seconds(frames: i64, fps: f64) -> String {
    format!("{:.2} s", frames as f64 / fps)
}

fn now_text() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (h, m, sec) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    // Civil date from days since 1970-01-01.
    let z = (secs / 86400) as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{sec:02}Z")
}

/// The start a new song gets: the ten-minute step after the latest song, as the app offers.
fn next_start(songs: &[SongSummary], project: &Project) -> String {
    let rate = project.rate;
    let latest = songs.iter().filter_map(|x| Timecode::parse_at(&x.start_tc, rate)).map(|tc| tc.to_frames(rate)).max();
    match latest {
        Some(frames) => {
            let ten_min = (600.0 * rate.fps()).round() as i64;
            let step = (frames / ten_min + 1) * ten_min;
            Timecode::from_frames(step % rate.frames_per_day(), rate).to_string()
        }
        None => "01:00:00:00".into(),
    }
}

fn transport_word(s: &Status) -> &'static str {
    let t = &s.transport;
    if t.estop {
        "E-STOP"
    } else if t.run == Run::WaitingForCode {
        "Waiting"
    } else if t.run == Run::Playing {
        match (t.output, t.record) {
            (Output::Rehearse, true) => "Record",
            (Output::Rehearse, false) => "Rehearse",
            _ => "LIVE",
        }
    } else {
        match t.output {
            Output::Positioning => "Arming",
            Output::Armed => "Armed",
            Output::Rehearse => "Stopped",
        }
    }
}

/// The name Play has now, by the server's own rule.
fn play_label(s: &Status) -> &'static str {
    if s.transport.locked() {
        "Play to code"
    } else if s.transport.output == Output::Rehearse {
        "Rehearse"
    } else {
        "Play Live"
    }
}

impl Controller {
    pub fn new(settings: Settings, shared: Arc<Shared>) -> Controller {
        Controller {
            api: settings.api,
            shared,
            map: settings.map,
            station: settings.station,
            lease_grace: settings.lease_grace,
            quiet: settings.quiet,
            server_up: false,
            server_error: "Connecting to the server".into(),
            panel_up: false,
            panel_model: String::new(),
            panel_name: String::new(),
            panel_lost: None,
            grace_spent: false,
            map_problems: Vec::new(),
            status: None,
            project: Project::default(),
            presets: Vec::new(),
            media: Vec::new(),
            songs: Vec::new(),
            song: None,
            undo: Vec::new(),
            redo: Vec::new(),
            run: None,
            mode: Mode::Show,
            shift: false,
            camera: String::new(),
            page: 0,
            song_list: false,
            clip: None,
            media_clip: None,
            lock_source: 0,
            record_axes: vec![Axis::Pan, Axis::Tilt],
            speed: 0.25,
            seek_to: None,
            last_turn: None,
            message: None,
            holding: None,
            asking: None,
            wind: None,
            marks: BTreeMap::new(),
            estop_down: None,
            heard: Instant::now(),
            estop_waiting: false,
            estop_fresh: true,
            server_asked: Instant::now(),
            stick_hint: None,
            started: Instant::now(),
            last_report: None,
            rendered: BTreeMap::new(),
        }
    }

    pub fn run(mut self, events: Receiver<Event>) {
        while !self.shared.stopped() {
            match events.recv_timeout(Duration::from_millis(25)) {
                Ok(event) => self.handle(event),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
            while let Ok(event) = events.try_recv() {
                self.handle(event);
            }
            self.flush_seek();
            self.tick();
            self.sync_hand();
            self.render();
            self.report();
        }
        self.shared.hand().enabled = false;
        if self.holds_lease() {
            let _ = self.api.release_lease();
        }
    }

    fn log(&self, text: &str) {
        if !self.quiet {
            eprintln!("panel: {text}");
        }
    }

    // ---- state ----

    fn holds_lease(&self) -> bool {
        self.status.as_ref().is_some_and(|s| s.you_hold_lease)
    }

    fn frame(&self) -> i64 {
        self.seek_to.unwrap_or_else(|| self.status.as_ref().map(|s| s.frame).unwrap_or(0))
    }

    fn fps(&self) -> f64 {
        self.project.rate.fps()
    }

    fn camera_index(&self) -> usize {
        self.project.camera_index(&self.camera).unwrap_or(0)
    }

    fn timecode(&self, frame: i64) -> String {
        match &self.song {
            Some(w) => w.song.clock(self.project.rate).timecode_at(frame).to_string(),
            None => Timecode::from_frames(frame, self.project.rate).to_string(),
        }
    }

    /// The selected camera's moves, in time order.
    fn lane(&self) -> Vec<&MotionClip> {
        let Some(w) = &self.song else { return Vec::new() };
        let mut clips: Vec<&MotionClip> = w.song.motion.iter().filter(|c| c.camera == self.camera).collect();
        clips.sort_by_key(|c| (c.start_frame, c.id.clone()));
        clips
    }

    fn selected(&self) -> Option<&MotionClip> {
        let id = self.clip.as_deref()?;
        self.song.as_ref()?.song.motion_clip(id)
    }

    fn selected_media(&self) -> Option<&ptz_core::song::MediaClip> {
        let id = self.media_clip.as_deref()?;
        self.song.as_ref()?.song.media_clip(id)
    }

    fn problems_of(&self, clip: &str) -> Vec<&ptz_core::compile::Issue> {
        let Some(w) = &self.song else { return Vec::new() };
        w.issues.iter().filter(|i| i.clip_id() == Some(clip)).collect()
    }

    fn camera_presets(&self) -> Vec<&Preset> {
        self.presets.iter().filter(|p| p.camera == self.camera).collect()
    }

    fn lock_sources(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = LOCK_SOURCES.iter().map(|(k, l)| (k.to_string(), l.to_string())).collect();
        if let Some(w) = &self.song {
            out.extend(w.song.media.iter().map(|m| (format!("file:{}", m.id), format!("File {}", m.name))));
        }
        out
    }

    /// One word for a camera's state, and the colour that goes with it.
    fn camera_state(&self, index: usize) -> (&'static str, Color) {
        let Some(s) = &self.status else { return ("", Color::Default) };
        let Some(c) = s.cameras.get(index) else { return ("", Color::Default) };
        if c.fault.is_some() {
            ("Fault", Color::Red)
        } else if c.held {
            ("Held", Color::Amber)
        } else if c.hardware && c.adapters.iter().any(|a| !a.connected) {
            ("No answer", Color::Amber)
        } else if c.hardware && s.transport.output != Output::Rehearse {
            ("Live", Color::Green)
        } else if c.hardware {
            ("Rig", Color::Blue)
        } else {
            ("Sim", Color::Blue)
        }
    }

    // ---- messages ----

    fn say(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.log(&text);
        self.message = Some(Message { kind: Kind::Note, text, since: Instant::now() });
    }

    fn refuse(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.log(&format!("refused: {text}"));
        self.message = Some(Message { kind: Kind::Refused, text, since: Instant::now() });
    }

    fn failed(&mut self, e: ApiErr) {
        match e {
            ApiErr::Refused { msg, .. } => self.refuse(msg),
            ApiErr::Network(msg) => self.server_lost(msg),
        }
    }

    fn server_lost(&mut self, why: String) {
        if self.server_up {
            self.log(&format!("server lost: {why}"));
        }
        self.server_up = false;
        self.server_asked = Instant::now();
        self.server_error = why;
        self.seek_to = None;
        self.holding = None;
        self.asking = None;
        self.wind = None;
    }

    fn ask(&mut self, question: impl Into<String>, act: Act) {
        self.asking = Some(Asking { question: question.into(), act, until: Instant::now() + CONFIRM_TIME });
        self.message = None;
        self.wind = None;
    }

    // ---- events ----

    fn handle(&mut self, event: Event) {
        match event {
            Event::Panel { hwc, trigger } => self.on_panel(hwc, trigger),
            Event::PanelUp { model, name, topology } => self.panel_arrived(model, name, topology),
            Event::PanelDown(why) => {
                if self.panel_up {
                    self.log(&format!("panel lost: {why}"));
                    self.panel_lost = Some(Instant::now());
                }
                self.panel_up = false;
                self.shift = false;
                self.holding = None;
                self.wind = None;
                self.estop_down = None;
            }
            Event::Status(status) => {
                if self.server_up {
                    self.take_status(*status);
                }
            }
            Event::Changed(change) => {
                if self.server_up {
                    self.changed(change);
                }
            }
            Event::ServerUp => self.refresh(),
            Event::ServerDown(why) => self.server_lost(why),
            Event::Estop(Ok(status)) => {
                self.estop_waiting = false;
                if self.server_up {
                    self.take_status(*status);
                }
                // A press while it is latched is sent all the same, and is about resetting: that message stays.
                if self.estop_fresh {
                    self.say("E-Stop sent. Output is held");
                }
            }
            Event::Estop(Err(why)) => {
                self.estop_waiting = false;
                self.refuse(why);
            }
            Event::EstopWaiting(why) => {
                self.estop_waiting = true;
                self.refuse(format!("E-Stop has not reached the server yet. Still trying. {why}"));
            }
            Event::HandRefused(why) => self.refuse(why),
            Event::RestFirst => self.say(REST_FIRST),
            Event::Heard(at) => self.heard = at,
            Event::PanelQuiet => {
                let counting =
                    self.holding.as_ref().is_some_and(|h| !h.fired) || matches!(self.estop_down, Some((_, true, false)));
                self.holding = None;
                self.estop_down = None;
                self.shift = false;
                if self.wind.take().is_some_and(|w| w.started) {
                    self.refuse("The panel stopped answering. Winding stopped");
                }
                if counting {
                    self.refuse("The panel stopped answering during the hold. Nothing was done");
                }
            }
        }
    }

    fn panel_arrived(&mut self, model: String, name: String, topology: Option<Topology>) {
        let problems: Vec<String> = self.map.problems(topology.as_ref()).into_iter().map(|p| p.text).collect();
        if !self.panel_up {
            self.log(&format!("panel connected: {} {}", if model.is_empty() { "unknown model" } else { &model }, name));
        }
        if problems != self.map_problems {
            for p in &problems {
                self.log(&format!("map: {p}"));
            }
            if let Some(first) = problems.first() {
                let more = if problems.len() > 1 { format!(" and {} more", problems.len() - 1) } else { String::new() };
                self.refuse(format!("Panel map: {first}{more}. Run ptz-panel map"));
            }
        }
        self.map_problems = problems;
        self.panel_up = true;
        self.panel_model = model;
        self.panel_name = name;
        self.panel_lost = None;
        self.grace_spent = false;
        self.shift = false;
        self.holding = None;
        self.estop_down = None;
    }

    /// Read everything from the server: at the start, and whenever it comes back.
    fn refresh(&mut self) {
        let status = match self.api.status() {
            Ok(s) => s,
            Err(e) => return self.server_lost(e.to_string()),
        };
        if !self.server_up {
            self.log("server connected");
        }
        self.server_up = true;
        self.server_error.clear();
        if let Ok(p) = self.api.project() {
            self.project = p;
        }
        if let Ok(p) = self.api.presets() {
            self.presets = p;
        }
        if let Ok(m) = self.api.media() {
            self.media = m;
        }
        if let Ok(s) = self.api.songs() {
            self.songs = s;
        }
        // A restarted server counts edits from zero again: read the song afresh.
        self.song = None;
        self.record_axes = status.record.axes.clone();
        self.take_status(status);
        self.keep_camera();
    }

    fn keep_camera(&mut self) {
        if self.project.camera(&self.camera).is_none() {
            self.camera = self.project.first().id.clone();
        }
        let project = &self.project;
        self.marks.retain(|camera, _| project.camera(camera).is_some());
    }

    fn take_status(&mut self, status: Status) {
        if status.transport.record {
            self.record_axes = status.record.axes.clone();
        }
        let want = status.song.as_ref().map(|s| (s.id.clone(), s.edit_counter));
        // A mark is half of an edit: without the lease it cannot be finished from here.
        if !status.you_hold_lease && !self.marks.is_empty() {
            self.marks.clear();
            self.say("The panel no longer holds the edit lease: the marks are dropped. Their presets are kept");
        }
        self.status = Some(status);
        let have = self.song.as_ref().map(|w| (w.song.id.clone(), w.edit_counter));
        if want != have {
            match want {
                Some((id, _)) => self.reload(&id),
                None => self.adopt(None),
            }
        }
    }

    /// Ask for the status now instead of waiting for the next one pushed.
    fn refresh_status(&mut self) {
        match self.api.status() {
            Ok(status) => self.take_status(status),
            Err(e) => self.failed(e),
        }
    }

    fn reload(&mut self, id: &str) {
        match self.api.working(id) {
            Ok(w) => self.adopt(Some(w)),
            Err(ApiErr::Network(msg)) => self.server_lost(msg),
            Err(ApiErr::Refused { .. }) => self.adopt(None),
        }
    }

    /// Take the server's copy of the song when it is not the one held here.
    fn adopt(&mut self, next: Option<WorkingSong>) {
        let same_song = match (&self.song, &next) {
            (Some(a), Some(b)) => a.song.id == b.song.id,
            _ => false,
        };
        if !same_song {
            self.undo.clear();
            self.redo.clear();
            self.marks.clear();
            self.clip = None;
            self.media_clip = None;
            self.song = next;
            return;
        }
        let (Some(old), Some(new)) = (self.song.take(), next) else { return };
        if old.edit_counter != new.edit_counter && old.song != new.song {
            // A record pass lands by itself: keep it one Undo away and select it.
            let pass =
                new.song.motion.iter().find(|c| c.recorded_pass.is_some() && old.song.motion_clip(&c.id).is_none_or(|o| o != *c));
            match pass {
                Some(clip) if self.holds_lease() => {
                    self.clip = Some(clip.id.clone());
                    self.camera = clip.camera.clone();
                    self.undo.push(old.song);
                    self.redo.clear();
                    self.say("The pass is on the timeline as a keys clip. Undo removes it");
                }
                _ => {
                    self.undo.clear();
                    self.redo.clear();
                }
            }
        }
        self.song = Some(new);
    }

    fn changed(&mut self, change: Change) {
        let result = match change {
            Change::Songs => self.api.songs().map(|s| self.songs = s),
            Change::Presets => self.api.presets().map(|p| self.presets = p),
            Change::Media => self.api.media().map(|m| self.media = m),
            Change::Project => self.api.project().map(|p| self.project = p),
            Change::Song { id, edit_counter, .. } => {
                let have = self.song.as_ref().map(|w| (w.song.id.as_str(), w.edit_counter));
                if self.song.as_ref().is_some_and(|w| w.song.id == id) && have != Some((id.as_str(), edit_counter)) {
                    self.reload(&id);
                }
                Ok(())
            }
        };
        let ok = result.is_ok();
        if let Err(ApiErr::Network(msg)) = result {
            self.server_lost(msg);
        }
        self.keep_camera();
        // The project's limits decide which moves are problems.
        if ok && self.project_changed_issues() {
            if let Some(id) = self.song.as_ref().map(|w| w.song.id.clone()) {
                self.reload(&id);
            }
        }
    }

    fn project_changed_issues(&self) -> bool {
        let Some(w) = &self.song else { return false };
        let Some(s) = self.status.as_ref().and_then(|s| s.song.as_ref()) else { return false };
        s.id == w.song.id && s.issues != w.issues.len()
    }

    // ---- input ----

    fn on_panel(&mut self, hwc: u32, trigger: Trigger) {
        let Some(role) = self.map.role_of(hwc) else { return };
        match (role, trigger) {
            (Role::Estop, Trigger::Down) => self.estop_pressed(true),
            (Role::Estop, Trigger::Press) => self.estop_pressed(false),
            (Role::Estop, Trigger::Up) => self.estop_released(),
            (Role::Estop, _) => {}
            (Role::Shift, Trigger::Down) => self.shift = true,
            (Role::Shift, Trigger::Up) => self.shift = false,
            (Role::JoyPan | Role::JoyTilt | Role::JoyZoom | Role::ZoomRocker, Trigger::Speed(v)) => self.stick_moved(v),
            (Role::Playhead, Trigger::Enc(n)) => self.jog_playhead(n),
            (Role::Enc(i), Trigger::Enc(n)) => self.turned(i, n),
            (Role::Enc(i), Trigger::Down | Trigger::Press) => self.pushed(i),
            (role, Trigger::Down) => self.button_down(role),
            (role, Trigger::Up) => self.button_up(role),
            (role, Trigger::Press) => {
                self.button_down(role);
                self.button_up(role);
            }
            _ => {}
        }
    }

    /// The E-Stop itself has already been sent by the panel link. This side only
    /// decides whether a hold may reset it: never the press that latched it.
    fn estop_pressed(&mut self, can_hold: bool) {
        let latched = self.server_up && self.status.as_ref().is_some_and(|s| s.transport.estop);
        self.estop_fresh = !latched;
        self.estop_down = can_hold.then(|| (Instant::now(), latched, false));
        self.holding = None;
        self.asking = None;
        self.wind = None;
        if latched && !can_hold {
            self.say("E-Stop is latched. Hold E-Stop to reset it");
        }
    }

    fn estop_released(&mut self) {
        if let Some((_, true, false)) = self.estop_down.take() {
            self.say(format!("E-Stop is latched. Hold E-Stop for {} to reset it", hold_time(HOLD_RESET)));
        }
    }

    fn stick_moved(&mut self, value: i32) {
        if value == 0 || self.hand_mode() {
            return;
        }
        let now = Instant::now();
        if self.stick_hint.is_none_or(|t| now.duration_since(t) > Duration::from_secs(3)) {
            self.stick_hint = Some(now);
            self.say("Press Camera to drive by hand");
        }
    }

    fn hand_mode(&self) -> bool {
        matches!(self.mode, Mode::Camera | Mode::Presets | Mode::Record | Mode::Program) && self.asking.is_none()
    }

    fn button_down(&mut self, role: Role) {
        let Some(face) = self.face(role) else { return };
        if !self.server_up && !matches!(face.act, Act::Mode(_)) {
            let why = self.server_error.clone();
            return self.refuse(format!("No server. {why}"));
        }
        if let Act::Wind(dir) = face.act {
            let now = Instant::now();
            self.wind =
                Some(Wind { role, dir, by_frame: self.shift, since: now, started: false, stepped: now, at: 0.0, sent: -1 });
            return;
        }
        match face.hold.clone() {
            Some((need, text)) => {
                self.message = None;
                self.holding = Some(Holding {
                    role,
                    since: Instant::now(),
                    need,
                    act: face.act.clone(),
                    text,
                    name: face.name(),
                    fired: false,
                });
            }
            None => self.perform(face.act, false),
        }
    }

    fn button_up(&mut self, role: Role) {
        if self.wind.as_ref().is_some_and(|w| w.role == role) {
            if let Some(w) = self.wind.take() {
                if !w.started {
                    self.jump_by(w.dir, w.by_frame);
                }
            }
            return;
        }
        if self.holding.as_ref().is_some_and(|h| h.role == role) {
            if let Some(h) = self.holding.take() {
                if !h.fired {
                    self.say(format!("Hold {} for {}. {}", h.name, hold_time(h.need), h.text));
                }
            }
        }
    }

    /// The face of any button: the one place that says what a press does now.
    fn face(&self, role: Role) -> Option<Face> {
        Some(match role {
            Role::Go => self.go_face(),
            Role::Key(n) => self.soft_keys().into_iter().nth(n.checked_sub(1)? as usize)?,
            Role::Mode(mode) => {
                // Program has no button of its own on the PTZ Pro: Shift and Presets.
                let mode = if mode == Mode::Presets && self.shift && !self.program_has_a_button() { Mode::Program } else { mode };
                let f = Face::new("", mode.name(), Act::Mode(mode));
                if mode == self.mode {
                    f.on(Color::Blue)
                } else {
                    f
                }
            }
            Role::Cam(n) => self.camera_key(n),
            Role::PageUp => self.page_face(-1),
            Role::PageDown => self.page_face(1),
            _ => return None,
        })
    }

    fn camera_key(&self, n: u8) -> Face {
        // Shift reaches cameras past the last key: 11 and 12 on the first two.
        let (plain, shifted) = (n as usize - 1, CAMERA_KEYS as usize + n as usize - 1);
        // In Program Shift is also held to step by frames, and the number pressed then is still
        // that camera's, unless the show has a camera for the key to reach with Shift.
        let stays = self.mode == Mode::Program && shifted >= self.project.cameras.len();
        let index = if self.shift && !stays { shifted } else { plain };
        let Some(cam) = self.project.cameras.get(index) else { return Face::dark() };
        let (word, color) = self.camera_state(index);
        let act = match self.mode {
            Mode::Program if self.status.as_ref().is_some_and(|s| s.transport.output != Output::Rehearse) => {
                Act::Say(ARMED_PROGRAM.into())
            }
            Mode::Program => Act::MarkCamera(index),
            _ => Act::Camera(index),
        };
        let f = Face::new(&format!("Cam {}", index + 1), &cam.name, act).color(color);
        let f = Face { line2: word.to_string(), ..f };
        if self.mode == Mode::Program && self.marks.contains_key(&cam.id) {
            // Marked: it flashes until its second press says where the move ends.
            f.color(Color::Amber).lit(Lit::Blink)
        } else if cam.id == self.camera {
            f.lit(Lit::On)
        } else {
            f
        }
    }

    fn program_has_a_button(&self) -> bool {
        self.map.hwc(Role::Mode(Mode::Program)).is_some()
    }

    fn pages(&self) -> usize {
        let (count, per) = match (self.mode, self.song_list) {
            (Mode::Show, true) => (self.songs.len(), 6),
            (Mode::Camera, _) => (self.project.cameras.len(), 6),
            (Mode::Presets, _) => (self.camera_presets().len(), 5),
            (Mode::Media, _) => (self.media.len(), 4),
            _ => (0, 1),
        };
        count.div_ceil(per).max(1)
    }

    fn page(&self) -> usize {
        self.page.min(self.pages() - 1)
    }

    fn page_face(&self, step: i32) -> Face {
        let to = self.page() as i32 + step;
        if self.asking.is_some() || to < 0 || to >= self.pages() as i32 {
            return Face::dark();
        }
        Face::new("", if step < 0 { "Page up" } else { "Page down" }, Act::Page(step))
    }

    fn go_face(&self) -> Face {
        let Some(s) = self.status.as_ref().filter(|_| self.server_up) else { return Face::dark() };
        let g = &s.guidance;
        let label = g.primary_label.as_str();
        match g.primary {
            // Without the lease or a song the server names E-Stop, which has its own key.
            Some(Action::Estop) | None => Face::why(g.hint.clone()),
            Some(Action::ResetEstop) => Face::new("", label, Act::Transport(Action::ResetEstop))
                .color(Color::Red)
                .lit(Lit::Blink)
                .hold(HOLD_RESET, g.hint.clone()),
            Some(Action::Play) if s.transport.output != Output::Rehearse => {
                Face::new("", label, Act::Transport(Action::Play)).on(Color::Green).hold(HOLD, g.hint.clone())
            }
            Some(Action::Disarm) => Face::new("", label, Act::Transport(Action::Disarm)).on(Color::Amber),
            Some(action) => {
                let live = s.transport.output != Output::Rehearse;
                Face::new("", label, Act::Transport(action)).on(if live { Color::Green } else { Color::White })
            }
        }
    }

    /// A transport key: lit when the server allows the action, labelled either way.
    fn transport_key(&self, title: &str, label: &str, action: Action) -> Face {
        let allowed = self.status.as_ref().is_some_and(|s| s.guidance.allows(action));
        Face::new(title, label, Act::Transport(action)).lit(if allowed { Lit::Dim } else { Lit::Off })
    }

    fn soft_keys(&self) -> Vec<Face> {
        let mut keys: Vec<Face> = (0..SOFT_KEYS).map(|_| Face::dark()).collect();
        if !self.server_up || self.status.is_none() {
            return keys;
        }
        if self.asking.is_some() {
            keys[0] = Face::new("", "Confirm", Act::Confirm).on(Color::Green);
            keys[6] = Face::new("", "Cancel", Act::Cancel);
            return keys;
        }
        match self.mode {
            Mode::Show => self.show_keys(&mut keys),
            Mode::Transport => self.transport_keys(&mut keys),
            Mode::Camera => self.camera_keys(&mut keys),
            Mode::Presets => self.preset_keys(&mut keys),
            Mode::Timeline => self.timeline_keys(&mut keys),
            Mode::Record => self.record_keys(&mut keys),
            Mode::Media => self.media_keys(&mut keys),
            Mode::Program => self.program_keys(&mut keys),
        }
        keys
    }

    /// Program: the timeline as a tape. Back and Forward jump on a tap and wind on a hold, one key
    /// plays and pauses, and the camera keys mark moves.
    fn program_keys(&self, keys: &mut [Face]) {
        let Some(s) = &self.status else { return };
        let all = |keys: &mut [Face], why: &str| keys.iter_mut().for_each(|key| *key = Face::why(why));
        if self.song.is_none() {
            return all(keys, "Create or load a song first");
        }
        if s.transport.output != Output::Rehearse {
            return all(keys, ARMED_PROGRAM);
        }
        let step = if self.shift { "1 frame".to_string() } else { format!("{JUMP_SECONDS:.0} s") };
        let lit = if s.guidance.allows(Action::Seek) { Lit::Dim } else { Lit::Off };
        keys[0] = Face::new("Back", &step, Act::Wind(-1)).lit(lit);
        keys[1] = if s.transport.run == Run::Stopped {
            self.transport_key("", "Play", Action::Play)
        } else {
            self.transport_key("", "Pause", Action::Stop).on(Color::Green)
        };
        keys[2] = Face::new("Forward", &step, Act::Wind(1)).lit(lit);
        keys[3] = Face::new("Playhead", "To start", Act::ToStart).lit(lit);
        keys[4] = if self.marks.is_empty() {
            Face::why("No camera is marked. Press a camera's number to mark where it stands")
        } else {
            Face::new("Mark", "Cancel", Act::CancelMark).on(Color::Amber)
        };
        let save = Face::new("Song", "Save", Act::Save);
        keys[5] = if s.song.as_ref().is_some_and(|song| song.dirty) { save.on(Color::Amber) } else { save.lit(Lit::Off) };
        keys[6] = self.history_key();
    }

    fn history_key(&self) -> Face {
        if self.shift {
            let f = Face::new("Edit", "Redo", Act::Redo);
            if self.redo.is_empty() {
                f.lit(Lit::Off)
            } else {
                f
            }
        } else {
            let f = Face::new("Edit", "Undo", Act::Undo);
            if self.undo.is_empty() {
                f.lit(Lit::Off)
            } else {
                f
            }
        }
    }

    fn show_keys(&self, keys: &mut [Face]) {
        let Some(s) = &self.status else { return };
        if self.song_list {
            let page = self.page();
            for (slot, song) in self.songs.iter().skip(page * 6).take(6).enumerate() {
                let f = Face::new(if song.loaded { "Loaded" } else { "Load" }, &song.name, Act::LoadSong(song.id.clone()));
                keys[slot] = if song.loaded { f.on(Color::Blue) } else { f };
            }
            keys[6] = Face::new("Songs", "Back", Act::Back);
            return;
        }
        keys[0] = match &s.lease {
            Some(_) if s.you_hold_lease => Face::new("Lease", "Release", Act::LeaseRelease).on(Color::Blue),
            Some(l) => Face::new("", &format!("Take over {}", l.holder), Act::LeaseTakeOver)
                .on(Color::Amber)
                .hold(HOLD, format!("{} holds the edit lease. Taking it over ends their editing", l.holder)),
            None => Face::new("Lease", "Take", Act::LeaseTake),
        };
        keys[1] = Face::new("Song", "New", Act::NewSong);
        keys[2] = Face::new("Song", &format!("Browse {}", self.songs.len()), Act::SongList);
        let Some(song) = &s.song else {
            for key in keys.iter_mut().skip(3) {
                *key = Face::why("Create or load a song first");
            }
            return;
        };
        let save = Face::new("Song", "Save", Act::Save);
        keys[3] = if song.dirty { save.on(Color::Amber) } else { save.lit(Lit::Off) };
        let commit = Face::new("Song", "Commit", Act::Commit);
        keys[4] = if !song.dirty && !song.committed_clean { commit.on(Color::Amber) } else { commit.lit(Lit::Off) };
        keys[5] = Face::new("Song", "Save copy", Act::SaveCopy);
        let revert = Face::new("Song", "Revert", Act::Revert).color(Color::Red);
        keys[6] = if song.dirty { revert } else { revert.lit(Lit::Off) };
    }

    fn transport_keys(&self, keys: &mut [Face]) {
        let Some(s) = &self.status else { return };
        let t = &s.transport;
        let play = self.transport_key("Play", play_label(s), Action::Play);
        keys[0] = if t.run != Run::Stopped {
            play.on(Color::Green)
        } else if t.output != Output::Rehearse {
            play.color(Color::Green).hold(HOLD, format!("{} moves the rig", play_label(s)))
        } else {
            play
        };
        keys[1] = self.transport_key("", "Stop", Action::Stop);
        keys[2] = self.transport_key("", "Cue", Action::Cue);
        keys[3] = match t.output {
            Output::Rehearse => self.transport_key("", "Arm", Action::Arm).color(Color::Green).hold(HOLD, ARM_LABEL),
            Output::Positioning => self.transport_key("", "Arming", Action::Arm).color(Color::Amber).lit(Lit::Blink),
            Output::Armed => self.transport_key("", "Armed", Action::Arm).on(Color::Green),
        };
        keys[4] = self.transport_key("", "Disarm", Action::Disarm);
        keys[5] = if t.locked() {
            self.transport_key("Locked", "Unlock", Action::Unlock).on(Color::Blue)
        } else {
            let allowed = s.guidance.allows(Action::Lock);
            Face::new("Lock to", &self.lock_label(), Act::Lock).lit(if allowed { Lit::Dim } else { Lit::Off })
        };
        keys[6] = self.record_key();
    }

    fn lock_label(&self) -> String {
        let sources = self.lock_sources();
        sources.get(self.lock_source.min(sources.len() - 1)).map(|(_, l)| l.clone()).unwrap_or_default()
    }

    fn record_key(&self) -> Face {
        let on = self.status.as_ref().is_some_and(|s| s.transport.record);
        if on {
            Face::new("Record", "On", Act::RecordToggle).on(Color::Red)
        } else {
            Face::new("Record", "Off", Act::RecordToggle).color(Color::Red)
        }
    }

    fn camera_keys(&self, keys: &mut [Face]) {
        let page = self.page();
        for (slot, index) in (page * 6..self.project.cameras.len()).take(6).enumerate() {
            let cam = &self.project.cameras[index];
            let (word, color) = self.camera_state(index);
            let f = Face::new(&format!("Cam {}", index + 1), &cam.name, Act::Camera(index)).color(color);
            let f = Face { line2: word.to_string(), ..f };
            keys[slot] = if cam.id == self.camera { f.lit(Lit::On) } else { f };
        }
        keys[6] = Face::new("Preset", "Store", Act::StorePreset);
    }

    fn preset_keys(&self, keys: &mut [Face]) {
        let real = self.status.as_ref().and_then(|s| s.cameras.get(self.camera_index())).is_some_and(|c| c.hardware);
        keys[0] = if self.shift {
            Face::new("Presets", if real { "Sync from rig" } else { "Sync from sim" }, Act::SyncPresets).color(Color::Amber)
        } else {
            Face::new("Preset", "Store", Act::StorePreset)
        };
        let page = self.page();
        for (slot, preset) in self.camera_presets().into_iter().skip(page * 5).take(5).enumerate() {
            keys[slot + 1] = if self.shift {
                Face::new("Delete", &preset.name, Act::DeletePreset(preset.id.clone())).color(Color::Red)
            } else {
                Face::new("Place", &preset.name, Act::PlacePreset(preset.id.clone())).color(Color::Blue)
            };
        }
        keys[6] = self.history_key();
    }

    fn timeline_keys(&self, keys: &mut [Face]) {
        if self.song.is_none() {
            for key in keys.iter_mut() {
                *key = Face::why("Create or load a song first");
            }
            return;
        }
        let clip = self.selected();
        let none = || Face::why("Select a clip first: Prev clip or Next clip");
        if self.shift {
            keys[0] = Face::new("Playhead", "To start", Act::ToStart);
            keys[1] = Face::new("Playhead", "To end", Act::ToEnd);
            keys[2] = match clip {
                Some(c) if c.is_preset_move() => Face::new("Clip", "To keys", Act::ToKeys),
                Some(_) => Face::why("This clip is keys already"),
                None => none(),
            };
            keys[3] = match clip {
                Some(c) if c.accepted.is_some() => Face::new("Problem", "Withdraw accept", Act::Withdraw).color(Color::Amber),
                Some(c) => {
                    let problems = self.problems_of(&c.id);
                    if problems.is_empty() {
                        Face::why("This clip has no problem to accept")
                    } else if problems.iter().all(|p| p.acceptable()) {
                        Face::new("Problem", "Accept as is", Act::Accept).on(Color::Amber)
                    } else {
                        Face::why("Range, jump and keep-out problems cannot be accepted. Change the move")
                    }
                }
                None => none(),
            };
            keys[4] = Face::new("Marker", "Add", Act::AddMarker);
            keys[5] = Face::new("Marker", "Delete", Act::DeleteMarker).color(Color::Red);
        } else {
            let lane = self.lane().len();
            let step = |text: &str, act: Act| {
                if lane == 0 {
                    Face::why("This camera has no clips yet")
                } else {
                    Face::new("Clip", text, act)
                }
            };
            keys[0] = step("Prev", Act::PrevClip);
            keys[1] = step("Next", Act::NextClip);
            keys[2] = match clip.map(|c| &c.body) {
                Some(ClipBody::PresetMove { ease, .. }) => Face::new("Ease", ease.label(), Act::CycleEase),
                Some(_) => Face::why("Only a preset move has an ease"),
                None => none(),
            };
            keys[3] = match clip.map(|c| (&c.body, c)) {
                Some((ClipBody::PresetMove { auto_duration: false, .. }, c)) => {
                    let fast = self.problems_of(&c.id).iter().any(|p| p.acceptable());
                    let f = Face::new("Time", if fast { "Use auto time" } else { "Auto time" }, Act::AutoTime);
                    if fast {
                        f.on(Color::Amber)
                    } else {
                        f
                    }
                }
                Some((ClipBody::PresetMove { .. }, _)) => Face::why("This move already takes its auto time"),
                Some(_) => Face::why("Only a preset move has an auto time"),
                None => none(),
            };
            keys[4] = if clip.is_some() { Face::new("Clip", "Duplicate", Act::Duplicate) } else { none() };
            keys[5] = if clip.is_some() { Face::new("Clip", "Delete", Act::DeleteClip).color(Color::Red) } else { none() };
        }
        keys[6] = self.history_key();
    }

    fn record_keys(&self, keys: &mut [Face]) {
        if self.shift {
            let recorded = self.selected().is_some_and(|c| c.recorded_pass.is_some());
            let refit = |text: &str, scale: f64| {
                if recorded {
                    Face::new("Keys", text, Act::Refit(scale))
                } else {
                    Face::why("Select a recorded clip first. A new pass is selected when it lands")
                }
            };
            keys[0] = refit("Fewer keys", FEWER_KEYS);
            keys[1] = refit("More keys", MORE_KEYS);
        } else {
            for (slot, axis) in Axis::ALL.into_iter().enumerate() {
                let f = Face::new("Record", axis.label(), Act::ToggleAxis(axis));
                keys[slot] = if self.record_axes.contains(&axis) { f.on(Color::Red) } else { f };
            }
            keys[5] = self.record_key();
        }
        keys[6] = self.history_key();
    }

    fn media_keys(&self, keys: &mut [Face]) {
        let page = self.page();
        for (slot, file) in self.media.iter().skip(page * 4).take(4).enumerate() {
            keys[slot] = Face::new("Place", &file.name, Act::PlaceMedia(file.id.clone())).color(Color::Blue);
        }
        let count = self.song.as_ref().map(|w| w.song.media.len()).unwrap_or(0);
        let clip = self.selected_media();
        let none =
            || Face::why(if count == 0 { "No media is on the timeline yet" } else { "Select a media clip first: press Clip" });
        if self.shift {
            keys[4] = if clip.is_some() { Face::new("Clip", "Delete", Act::DeleteMediaClip).color(Color::Red) } else { none() };
            let locked = self.status.as_ref().is_some_and(|s| s.transport.locked());
            keys[5] = if locked {
                self.transport_key("Locked", "Unlock", Action::Unlock).on(Color::Blue)
            } else if clip.is_some() {
                Face::new("Lock", "To this file", Act::LockToFile)
            } else {
                none()
            };
        } else {
            keys[4] = match (clip, count) {
                (_, 0) => none(),
                (Some(c), _) => Face::new("Clip", &c.name, Act::NextMediaClip).on(Color::Blue),
                (None, _) => Face::new("Clip", "Select", Act::NextMediaClip),
            };
            keys[5] = match clip {
                Some(c) if c.muted => Face::new("Muted", "Unmute", Act::Mute).on(Color::Amber),
                Some(_) => Face::new("Clip", "Mute", Act::Mute),
                None => none(),
            };
        }
        keys[6] = self.history_key();
    }

    fn knobs(&self) -> Vec<KnobFace> {
        let mut knobs: Vec<KnobFace> = (0..SOFT_ENCODERS).map(|_| KnobFace::dark()).collect();
        if !self.server_up || self.status.is_none() || self.asking.is_some() {
            return knobs;
        }
        match self.mode {
            Mode::Transport => knobs[0] = KnobFace::new("Lock to", self.lock_label(), Knob::LockSource),
            Mode::Camera | Mode::Presets | Mode::Record | Mode::Program => {
                let pose =
                    self.status.as_ref().and_then(|s| s.cameras.get(self.camera_index())).map(|c| c.pose).unwrap_or_default();
                knobs[0] = KnobFace::new("Focus", Axis::Focus.format(pose.focus), Knob::Focus);
                knobs[1] = KnobFace::new("Elevation", Axis::Elevation.format(pose.elevation), Knob::Elevation);
                knobs[2] = KnobFace::new("Top speed", format!("{:.0}%", self.speed * 100.0), Knob::TopSpeed);
            }
            Mode::Timeline => {
                if let Some(c) = self.selected() {
                    knobs[0] = KnobFace::new("Move clip", self.timecode(c.start_frame), Knob::MoveClip);
                    knobs[1] = KnobFace::new("Length", seconds(c.duration_frames, self.fps()), Knob::Length);
                }
                knobs[2] = KnobFace::new("Clip edge", "Jump", Knob::Edge);
                let frame = self.frame();
                let here = self.song.as_ref().and_then(|w| w.song.markers.iter().find(|m| m.frame == frame));
                knobs[3] = KnobFace::new(
                    "Marker",
                    here.map(|m| m.name.clone()).unwrap_or_else(|| "Jump. Push: add".into()),
                    Knob::Marker,
                );
            }
            _ => {}
        }
        knobs
    }

    // ---- knobs ----

    fn step(&mut self, n: i32) -> i64 {
        // A fast turn covers more ground; Shift makes every step a second.
        let now = Instant::now();
        let fast = self.last_turn.is_some_and(|t| now.duration_since(t) < Duration::from_millis(60));
        self.last_turn = Some(now);
        let n = n as i64;
        if self.shift {
            n * self.project.rate.nominal()
        } else if n.abs() > 1 || fast {
            n * n.abs().max(2)
        } else {
            n
        }
    }

    fn jog_playhead(&mut self, n: i32) {
        if !self.server_up {
            return;
        }
        let step = self.step(n);
        let length = self.song.as_ref().map(|w| w.song.length_frames).unwrap_or(0);
        self.seek_to = Some((self.frame() + step).clamp(0, length.max(0)));
    }

    fn flush_seek(&mut self) {
        let Some(frame) = self.seek_to else { return };
        self.transport(Action::Seek, TransportRequest { frame: Some(frame), ..TransportRequest::default() });
        self.seek_to = None;
    }

    fn seek(&mut self, frame: i64) {
        self.seek_to = Some(frame.max(0));
        self.flush_seek();
    }

    fn length(&self) -> i64 {
        self.song.as_ref().map(|w| w.song.length_frames).unwrap_or(0).max(0)
    }

    /// A tap on Back or Forward.
    fn jump_by(&mut self, dir: i64, by_frame: bool) {
        let step = if by_frame { 1 } else { (JUMP_SECONDS * self.fps()).round() as i64 };
        self.seek((self.frame() + dir * step).clamp(0, self.length()));
    }

    /// Carry a held Back or Forward on. It begins only once the panel has been heard from after
    /// the time of a tap, so a release lost on the way does not become a wind, and it stops with
    /// the first seek the server refuses.
    fn wind_on(&mut self, now: Instant) {
        let (heard, here, length, fps) = (self.heard, self.frame(), self.length(), self.fps());
        let Some(w) = &mut self.wind else { return };
        if !w.started {
            if heard < w.since + TAP {
                return;
            }
            w.started = true;
            w.stepped = now;
            w.sent = here;
            // The first step of a wind is taken at once: a frame, or a tenth of a second of winding.
            w.at = here as f64 + w.dir as f64 * if w.by_frame { 1.0 } else { WIND * fps * WIND_EVERY.as_secs_f64() };
        } else {
            let dt = now.duration_since(w.stepped);
            if dt < WIND_EVERY {
                return;
            }
            w.stepped = now;
            let speed = if w.by_frame {
                FRAME_STEPS_A_SECOND
            } else if now.duration_since(w.since) >= WIND_FAST_AFTER {
                WIND_FAST * fps
            } else {
                WIND * fps
            };
            w.at += w.dir as f64 * speed * dt.as_secs_f64().min(0.25);
        }
        w.at = w.at.clamp(0.0, length as f64);
        let frame = w.at.round() as i64;
        if frame == w.sent {
            return;
        }
        w.sent = frame;
        if !self.transport(Action::Seek, TransportRequest { frame: Some(frame), ..TransportRequest::default() }) {
            self.wind = None;
        }
    }

    /// What a wind in progress is doing, for the display.
    fn winding(&self) -> Option<String> {
        let w = self.wind.as_ref().filter(|w| w.started)?;
        let way = if w.dir < 0 { "back" } else { "forward" };
        Some(if w.by_frame {
            format!("Stepping {way} frame by frame. Let go to stop")
        } else {
            let speed = if w.since.elapsed() >= WIND_FAST_AFTER { WIND_FAST } else { WIND };
            format!("Winding {way} at {speed:.0} times speed. Let go to stop")
        })
    }

    // ---- marks ----

    /// A camera key in Program mode. The first press captures where the rig stands, as a preset it
    /// has or a new one, and the key flashes. The second places the move: it leaves where the
    /// playhead was at the first press and has arrived where it is now.
    fn mark_camera(&mut self, index: usize) {
        let Some(cam) = self.project.cameras.get(index) else { return };
        let (camera, label) = (cam.id.clone(), format!("Cam {}", index + 1));
        self.camera = camera.clone();
        if self.selected().is_some_and(|c| c.camera != camera) {
            self.clip = None;
        }
        if self.song.is_none() {
            return self.refuse("Create or load a song first");
        }
        let frame = self.frame();
        let Some(mark) = self.marks.get(&camera).cloned() else {
            let req = CapturePreset { camera: Some(camera.clone()), name: None };
            return match self.api.post::<_, Captured>("/presets/capture", &req) {
                Ok(got) => {
                    let at = self.timecode(frame);
                    let name = got.preset.name.clone();
                    if !self.presets.iter().any(|p| p.id == got.preset.id) {
                        self.presets.push(got.preset.clone());
                    }
                    self.marks.insert(camera, Mark { preset: got.preset, frame, made_new: !got.reused, at: Instant::now() });
                    let n = index + 1;
                    self.say(if got.reused {
                        format!("{label} is within range of {name}: using it. Marked at {at}. Press {n} again where it must have arrived")
                    } else {
                        format!("{label} marked at {at} as new {name}. Press {n} again where it must have arrived")
                    });
                }
                Err(e) => self.failed(e),
            };
        };
        let mut placed = None;
        let done = self.edit(|song, project| {
            let (song, clip) = edit::insert_marked_move(song, project, &mark.preset, mark.frame, frame)?;
            placed = Some(clip);
            Ok(song)
        });
        if !done {
            // Refused, with the reason on the display. The mark stays for another place.
            return;
        }
        self.marks.remove(&camera);
        self.clip = placed.clone();
        let name = &mark.preset.name;
        let text = if frame > mark.frame {
            let problem = placed
                .as_deref()
                .and_then(|id| self.problems_of(id).first().map(|p| p.message()))
                .map(|p| format!(". PROBLEM {p}. Mark it longer, or fix it in Timeline"))
                .unwrap_or_default();
            format!(
                "{label} moves to {name} from {} to {}, {}{problem}",
                self.timecode(mark.frame),
                self.timecode(frame),
                seconds(frame - mark.frame, self.fps())
            )
        } else {
            format!("{label}: move to {name} placed at {}. It takes its auto time", self.timecode(frame))
        };
        self.say(text);
    }

    /// Drop the newest mark, and the preset with it when that press stored it.
    fn cancel_mark(&mut self) {
        let Some(camera) = self.marks.iter().max_by_key(|(_, m)| m.at).map(|(camera, _)| camera.clone()) else {
            return self.say("No camera is marked");
        };
        let Some(mark) = self.marks.remove(&camera) else { return };
        let label = self.project.camera_index(&camera).map(|i| format!("Cam {}", i + 1)).unwrap_or_default();
        if !mark.made_new {
            return self.say(format!("{label} is no longer marked. {} is kept", mark.preset.name));
        }
        match self.api.delete::<Vec<Preset>>(&format!("/presets/{}", mark.preset.id)) {
            Ok(list) => {
                self.presets = list;
                self.say(format!("{label} is no longer marked. {} is removed", mark.preset.name));
            }
            Err(e) => self.failed(e),
        }
    }

    fn turned(&mut self, index: u8, n: i32) {
        let Some(knob) = self.knobs().get(index.saturating_sub(1) as usize).map(|k| k.knob) else { return };
        match knob {
            Knob::None => {}
            Knob::LockSource => {
                let count = self.lock_sources().len() as i32;
                self.lock_source = (self.lock_source as i32 + n.signum()).rem_euclid(count) as usize;
            }
            Knob::Focus | Knob::Elevation => {
                let speed = (n as f64 * 0.5).clamp(-1.0, 1.0);
                let until = Instant::now() + Duration::from_millis(250);
                let mut hand = self.shared.hand();
                if knob == Knob::Focus {
                    hand.focus = Some((speed, until));
                } else {
                    hand.elevation = Some((speed, until));
                }
            }
            Knob::TopSpeed => self.speed = (self.speed + n as f64 * 0.05).clamp(0.05, 1.0),
            Knob::MoveClip => {
                let step = self.step(n);
                let Some((id, start)) = self.selected().map(|c| (c.id.clone(), c.start_frame)) else { return };
                self.edit_run(&format!("move {id}"), |song, project| edit::move_clip(song, project, &id, start + step));
            }
            Knob::Length => {
                let step = self.step(n);
                let Some((id, end)) = self.selected().map(|c| (c.id.clone(), c.end_frame())) else { return };
                self.edit_run(&format!("length {id}"), |song, project| edit::trim_clip_end(song, project, &id, end + step));
            }
            Knob::Edge => self.jump_edge(n),
            Knob::Marker => self.jump_marker(n),
        }
    }

    fn pushed(&mut self, index: u8) {
        let Some(knob) = self.knobs().get(index.saturating_sub(1) as usize).map(|k| k.knob) else { return };
        if knob == Knob::Marker {
            self.perform(Act::AddMarker, false);
        }
    }

    fn jump_edge(&mut self, n: i32) {
        let Some(w) = &self.song else { return };
        let mut edges: Vec<i64> = w
            .song
            .motion
            .iter()
            .filter(|c| c.camera == self.camera)
            .flat_map(|c| [c.start_frame, c.end_frame()])
            .chain(w.song.media.iter().flat_map(|c| [c.start_frame, c.end_frame()]))
            .collect();
        edges.sort();
        self.jump(&edges, n, "There is no clip edge that way");
    }

    fn jump_marker(&mut self, n: i32) {
        let Some(w) = &self.song else { return };
        let mut frames: Vec<i64> = w.song.markers.iter().map(|m| m.frame).collect();
        frames.sort();
        if frames.is_empty() {
            return self.say("This song has no markers. Push the Marker knob to add one at the playhead");
        }
        self.jump(&frames, n, "There is no marker that way");
    }

    fn jump(&mut self, frames: &[i64], n: i32, none: &str) {
        let here = self.frame();
        let to =
            if n > 0 { frames.iter().copied().find(|f| *f > here) } else { frames.iter().rev().copied().find(|f| *f < here) };
        match to {
            Some(frame) => self.seek(frame),
            None => self.say(none),
        }
    }

    // ---- actions ----

    fn transport(&mut self, action: Action, req: TransportRequest) -> bool {
        match self.api.act(action, &req) {
            Ok(status) => {
                self.take_status(status);
                true
            }
            Err(e) => {
                self.failed(e);
                false
            }
        }
    }

    /// Apply one edit to the song and send it, as the app does.
    fn edit(&mut self, f: impl FnOnce(&Song, &Project) -> Result<Song, Refusal>) -> bool {
        self.run = None;
        self.edit_inner(f)
    }

    /// An edit that joins the one before it when it is the same kind on the same clip, moments apart.
    fn edit_run(&mut self, key: &str, f: impl FnOnce(&Song, &Project) -> Result<Song, Refusal>) -> bool {
        let now = Instant::now();
        let joins = self.run.as_ref().is_some_and(|(k, at)| k == key && now.duration_since(*at) < Duration::from_secs(1));
        let depth = self.undo.len();
        let done = self.edit_inner(f);
        if done {
            if joins && self.undo.len() > depth {
                self.undo.pop();
            }
            self.run = Some((key.to_string(), now));
        }
        done
    }

    fn edit_inner(&mut self, f: impl FnOnce(&Song, &Project) -> Result<Song, Refusal>) -> bool {
        let Some(w) = &self.song else {
            self.refuse("Create or load a song first");
            return false;
        };
        let next = match f(&w.song, &self.project) {
            Ok(song) => song,
            Err(refusal) => {
                self.refuse(refusal.to_string());
                return false;
            }
        };
        if next == w.song {
            return true;
        }
        let before = w.song.clone();
        if self.put(next) {
            self.undo.push(before);
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
            self.redo.clear();
            true
        } else {
            false
        }
    }

    fn put(&mut self, song: Song) -> bool {
        let Some(w) = &self.song else { return false };
        match self.api.put_working(&PutWorking { expected_counter: w.edit_counter, song }) {
            Ok(next) => {
                self.song = Some(next);
                true
            }
            Err(e) => {
                self.failed(e);
                false
            }
        }
    }

    fn history(&mut self, back: bool) {
        self.run = None;
        let Some(current) = self.song.as_ref().map(|w| w.song.clone()) else {
            return self.refuse("Create or load a song first");
        };
        let from = if back { self.undo.pop() } else { self.redo.pop() };
        let Some(song) = from else {
            return self.say(if back { "Nothing to undo" } else { "Nothing to redo" });
        };
        let kept = song.clone();
        if self.put(song) {
            if back {
                self.redo.push(current);
            } else {
                self.undo.push(current);
            }
            self.say(if back { "Undone. Shift and Undo is Redo" } else { "Redone" });
        } else if back {
            self.undo.push(kept);
        } else {
            self.redo.push(kept);
        }
    }

    fn song_id(&mut self) -> Option<String> {
        let id = self.song.as_ref().map(|w| w.song.id.clone());
        if id.is_none() {
            self.refuse("Create or load a song first");
        }
        id
    }

    fn song_name(&self) -> String {
        self.song.as_ref().map(|w| w.song.name.clone()).unwrap_or_default()
    }

    fn set_mode(&mut self, mode: Mode) {
        self.asking = None;
        self.holding = None;
        self.wind = None;
        self.song_list = false;
        self.page = 0;
        self.mode = mode;
        // A note has been read once the operator moves on; a refusal stays its time.
        if self.message.as_ref().is_some_and(|m| m.kind == Kind::Note) {
            self.message = None;
        }
    }

    fn select_clip(&mut self, step: i32) {
        let frame = self.frame();
        let lane: Vec<(String, i64)> = self.lane().into_iter().map(|c| (c.id.clone(), c.start_frame)).collect();
        if lane.is_empty() {
            return self.say("This camera has no clips yet");
        }
        let at = self.clip.as_ref().and_then(|id| lane.iter().position(|(c, _)| c == id));
        let next = match at {
            Some(i) => (i as i32 + step).clamp(0, lane.len() as i32 - 1) as usize,
            // With nothing selected, start from the playhead.
            None if step > 0 => lane.iter().position(|(_, start)| *start >= frame).unwrap_or(lane.len() - 1),
            None => lane.iter().rposition(|(_, start)| *start <= frame).unwrap_or(0),
        };
        if at == Some(next) {
            return self.say(if step > 0 {
                "This is the last clip of this camera"
            } else {
                "This is the first clip of this camera"
            });
        }
        self.clip = Some(lane[next].0.clone());
        self.seek(lane[next].1);
    }

    fn perform(&mut self, act: Act, confirmed: bool) {
        match act {
            Act::None => {}
            Act::Say(text) => self.say(text),
            Act::Mode(mode) => self.set_mode(mode),
            Act::Camera(index) => {
                if let Some(cam) = self.project.cameras.get(index) {
                    self.camera = cam.id.clone();
                    if self.mode != Mode::Camera {
                        self.page = 0;
                    }
                    if self.selected().is_some_and(|c| c.camera != self.camera) {
                        self.clip = None;
                    }
                }
            }
            Act::Page(step) => self.page = (self.page() as i32 + step).max(0) as usize,
            Act::Confirm => {
                if let Some(asking) = self.asking.take() {
                    self.perform(asking.act, true);
                }
            }
            Act::Cancel => {
                self.asking = None;
                self.say("Cancelled. Nothing was changed");
            }
            Act::Back => {
                self.song_list = false;
                self.page = 0;
            }
            Act::LeaseTake | Act::LeaseTakeOver => match self.api.take_lease(&self.station, act == Act::LeaseTakeOver) {
                Ok(status) => {
                    self.take_status(status);
                    self.say(format!("{} holds the edit lease", self.station));
                }
                Err(e) => self.failed(e),
            },
            Act::LeaseRelease => match self.api.release_lease() {
                Ok(status) => {
                    self.take_status(status);
                    self.say("The edit lease is released");
                }
                Err(e) => self.failed(e),
            },
            Act::NewSong => self.new_song(),
            Act::SongList => {
                self.song_list = true;
                self.page = 0;
                match self.api.songs() {
                    Ok(songs) => self.songs = songs,
                    Err(e) => self.failed(e),
                }
            }
            Act::LoadSong(id) => match self.api.load_song(&id) {
                Ok(w) => {
                    let name = w.song.name.clone();
                    self.adopt(Some(w));
                    self.song_list = false;
                    self.page = 0;
                    self.refresh_status();
                    self.say(format!("Loaded {name}"));
                }
                Err(e) => self.failed(e),
            },
            Act::Save => {
                let Some(id) = self.song_id() else { return };
                match self.api.save(&id) {
                    Ok(_) => {
                        self.refresh_status();
                        self.say(format!("Saved {}", self.song_name()));
                    }
                    Err(e) => self.failed(e),
                }
            }
            Act::Commit => {
                let Some(id) = self.song_id() else { return };
                match self.api.commit(&id) {
                    Ok(info) => {
                        self.refresh_status();
                        self.say(format!("Committed {} as revision {}", self.song_name(), info.number));
                    }
                    Err(e) => self.failed(e),
                }
            }
            Act::SaveCopy => {
                let Some(id) = self.song_id() else { return };
                let name = format!("{} copy", self.song_name());
                match self.api.copy_song(&id, &name) {
                    Ok(w) => self.say(format!("Saved a copy as {}. Browse to load it", w.song.name)),
                    Err(e) => self.failed(e),
                }
            }
            Act::Revert => {
                let Some(id) = self.song_id() else { return };
                if !confirmed {
                    return self
                        .ask(format!("Revert {} to its last Save? Changes since then are lost", self.song_name()), Act::Revert);
                }
                match self.api.revert(&id) {
                    Ok(w) => {
                        self.undo.clear();
                        self.redo.clear();
                        self.song = Some(w);
                        self.refresh_status();
                        self.say(format!("Reverted {} to its last Save", self.song_name()));
                    }
                    Err(e) => self.failed(e),
                }
            }
            Act::Transport(action) => {
                self.transport(action, TransportRequest::default());
            }
            Act::Lock => {
                let sources = self.lock_sources();
                let (source, label) = sources[self.lock_source.min(sources.len() - 1)].clone();
                if self.transport(Action::Lock, TransportRequest { source: Some(source), ..TransportRequest::default() }) {
                    self.say(format!("Locked to {label}"));
                }
            }
            Act::LockToFile => {
                let Some((id, name)) = self.selected_media().map(|c| (c.id.clone(), c.name.clone())) else { return };
                if self.transport(
                    Action::Lock,
                    TransportRequest { source: Some(format!("file:{id}")), ..TransportRequest::default() },
                ) {
                    self.say(format!("Locked to {name}"));
                }
            }
            Act::RecordToggle => {
                if self.status.as_ref().is_some_and(|s| s.transport.record) {
                    self.transport(Action::RecordOff, TransportRequest::default());
                } else {
                    self.record_on();
                }
            }
            Act::ToggleAxis(axis) => self.toggle_axis(axis),
            Act::Wind(dir) => self.jump_by(dir, self.shift),
            Act::MarkCamera(index) => self.mark_camera(index),
            Act::CancelMark => self.cancel_mark(),
            Act::StorePreset => {
                let name = format!("Preset {}", self.camera_presets().len() + 1);
                let req = StorePreset { name, pose: None, camera: Some(self.camera.clone()) };
                match self.api.post::<_, Preset>("/presets", &req) {
                    Ok(p) => {
                        self.say(format!("Stored {} for {}", p.name, self.project.label(&p.camera)));
                        self.presets.push(p);
                    }
                    Err(e) => self.failed(e),
                }
            }
            Act::PlacePreset(id) => {
                let Some(preset) = self.presets.iter().find(|p| p.id == id).cloned() else { return };
                let frame = self.frame();
                let mut placed = None;
                let done = self.edit(|song, project| {
                    let (song, clip) = edit::insert_preset_move(song, project, &preset, frame)?;
                    placed = Some(clip);
                    Ok(song)
                });
                if done {
                    self.clip = placed;
                    self.say(format!("Move to {} placed at {}. It takes its auto time", preset.name, self.timecode(frame)));
                }
            }
            Act::DeletePreset(id) => {
                let Some(name) = self.presets.iter().find(|p| p.id == id).map(|p| p.name.clone()) else { return };
                if !confirmed {
                    return self.ask(
                        format!("Delete {name}? This cannot be undone. Moves already on the timeline keep their pose"),
                        Act::DeletePreset(id),
                    );
                }
                match self.api.delete::<Vec<Preset>>(&format!("/presets/{id}")) {
                    Ok(list) => {
                        self.presets = list;
                        self.say(format!("Deleted {name}"));
                    }
                    Err(e) => self.failed(e),
                }
            }
            Act::SyncPresets => {
                let real = self.status.as_ref().and_then(|s| s.cameras.get(self.camera_index())).is_some_and(|c| c.hardware);
                let label = self.project.label(&self.camera);
                if real && !confirmed {
                    return self.ask(format!("Read presets from camera {label}? The camera moves to each one"), Act::SyncPresets);
                }
                match self.api.sync_presets(&self.camera) {
                    Ok(list) => {
                        self.presets = list;
                        self.say(if real {
                            format!("Reading presets from camera {label}. It moves while it does")
                        } else {
                            format!("Presets read from the simulator for camera {label}")
                        });
                    }
                    Err(e) => self.failed(e),
                }
            }
            Act::PrevClip => self.select_clip(-1),
            Act::NextClip => self.select_clip(1),
            Act::CycleEase => {
                let Some((id, ease)) = self.selected().and_then(|c| match &c.body {
                    ClipBody::PresetMove { ease, .. } => Some((c.id.clone(), *ease)),
                    _ => None,
                }) else {
                    return;
                };
                let at = Ease::ALL.iter().position(|e| *e == ease).unwrap_or(0);
                let next = Ease::ALL[(at + 1) % Ease::ALL.len()];
                self.edit_run(&format!("ease {id}"), |song, project| edit::set_ease(song, project, &id, next));
            }
            Act::AutoTime => {
                let Some(id) = self.clip.clone() else { return };
                if self.edit(|song, project| edit::set_auto_duration(song, project, &id)) {
                    self.say("The move takes its auto time again");
                }
            }
            Act::Duplicate => {
                let Some(id) = self.clip.clone() else { return };
                let mut copy = None;
                let done = self.edit(|song, project| {
                    let (song, ids) = edit::duplicate(song, project, &[id])?;
                    copy = ids.into_iter().next();
                    Ok(song)
                });
                if done {
                    self.clip = copy;
                    self.say("Duplicated. The copy is selected");
                }
            }
            Act::DeleteClip => {
                let Some((id, label)) = self.selected().map(|c| (c.id.clone(), c.label())) else { return };
                if self.edit(|song, project| edit::delete(song, project, &[id])) {
                    self.clip = None;
                    self.say(format!("Deleted {label}. Undo brings it back"));
                }
            }
            Act::Undo => self.history(true),
            Act::Redo => self.history(false),
            Act::ToStart => self.seek(0),
            Act::ToEnd => {
                let end = self.song.as_ref().map(|w| w.song.content_end()).unwrap_or(0);
                self.seek(end);
            }
            Act::ToKeys => {
                let Some(id) = self.clip.clone() else { return };
                if self.edit(|song, project| edit::convert_to_keys(song, project, &id)) {
                    self.say("The move is keys now. Undo brings the preset move back");
                }
            }
            Act::Accept => {
                let Some(id) = self.clip.clone() else { return };
                let (by, at) = (self.station.clone(), now_text());
                if self.edit(|song, project| edit::accept_issues(song, project, &id, &by, &at)) {
                    self.say("Accepted as programmed. The rig will lag behind the timeline here");
                }
            }
            Act::Withdraw => {
                let Some(id) = self.clip.clone() else { return };
                if self.edit(|song, project| edit::clear_acceptance(song, project, &id)) {
                    self.say("Acceptance withdrawn");
                }
            }
            Act::AddMarker => {
                let frame = self.frame();
                let name = format!("Marker {}", self.song.as_ref().map(|w| w.song.markers.len()).unwrap_or(0) + 1);
                if self.edit(|song, project| edit::add_marker(song, project, frame, &name)) {
                    self.say(format!("{name} added at {}", self.timecode(frame)));
                }
            }
            Act::DeleteMarker => {
                let frame = self.frame();
                let found = self
                    .song
                    .as_ref()
                    .and_then(|w| w.song.markers.iter().find(|m| m.frame == frame))
                    .map(|m| (m.id.clone(), m.name.clone()));
                let Some((id, name)) = found else {
                    return self.refuse("There is no marker at the playhead. Turn the Marker knob to jump to one");
                };
                if self.edit(|song, project| edit::delete(song, project, &[id])) {
                    self.say(format!("Deleted {name}. Undo brings it back"));
                }
            }
            Act::Refit(scale) => {
                let Some(id) = self.clip.clone() else { return };
                let Some(before) = self.song.as_ref().map(|w| w.song.clone()) else { return };
                match self.api.post::<_, WorkingSong>(&format!("/record/{id}/refit"), &Refit { scale }) {
                    Ok(w) => {
                        let keys = |song: &Song| {
                            song.motion_clip(&id).map(|c| Axis::ALL.iter().map(|a| c.keys(*a).len()).sum::<usize>()).unwrap_or(0)
                        };
                        let (was, now) = (keys(&before), keys(&w.song));
                        self.song = Some(w);
                        self.undo.push(before);
                        self.redo.clear();
                        self.say(format!(
                            "{}: {was} keys before, {now} now",
                            if scale > 1.0 { "Fewer keys" } else { "More keys" }
                        ));
                    }
                    Err(e) => self.failed(e),
                }
            }
            Act::PlaceMedia(id) => {
                let Some(file) = self.media.iter().find(|m| m.id == id).cloned() else { return };
                let frame = self.frame();
                let track = if file.kind == MediaKind::Video { Track::Video } else { Track::Audio };
                let mut placed = None;
                let done = self.edit(|song, project| {
                    let (song, clip) =
                        edit::add_media_clip(song, project, &file.id, &file.name, track, file.duration_frames, frame)?;
                    placed = Some(clip);
                    Ok(song)
                });
                if done {
                    self.media_clip = placed;
                    self.say(format!("{} placed at {}", file.name, self.timecode(frame)));
                }
            }
            Act::NextMediaClip => {
                let Some(w) = &self.song else { return };
                let mut clips: Vec<(i64, String)> = w.song.media.iter().map(|c| (c.start_frame, c.id.clone())).collect();
                clips.sort();
                if clips.is_empty() {
                    return;
                }
                let at = self.media_clip.as_ref().and_then(|id| clips.iter().position(|(_, c)| c == id));
                let next = at.map(|i| (i + 1) % clips.len()).unwrap_or(0);
                self.media_clip = Some(clips[next].1.clone());
            }
            Act::Mute => {
                let Some((id, muted, name)) = self.selected_media().map(|c| (c.id.clone(), c.muted, c.name.clone())) else {
                    return;
                };
                if self.edit(|song, project| edit::set_media_muted(song, project, &id, !muted)) {
                    self.say(if muted { format!("{name} plays again") } else { format!("{name} is muted") });
                }
            }
            Act::DeleteMediaClip => {
                let Some((id, name)) = self.selected_media().map(|c| (c.id.clone(), c.name.clone())) else { return };
                if self.edit(|song, project| edit::delete(song, project, &[id])) {
                    self.media_clip = None;
                    self.say(format!("Took {name} off the timeline. Undo brings it back. The file stays imported"));
                }
            }
        }
    }

    fn new_song(&mut self) {
        let songs = match self.api.songs() {
            Ok(s) => s,
            Err(e) => return self.failed(e),
        };
        let name = format!("Song {}", songs.len() + 1);
        let start = next_start(&songs, &self.project);
        let created = match self.api.new_song(&name, Some(&start)) {
            Ok(w) => w,
            Err(e) => return self.failed(e),
        };
        match self.api.load_song(&created.song.id) {
            Ok(w) => {
                self.adopt(Some(w));
                self.refresh_status();
                self.say(format!("{name} starts at {start} and is loaded"));
            }
            Err(ApiErr::Refused { msg, .. }) => self.refuse(format!("{name} is created but not loaded. {msg}")),
            Err(e) => self.failed(e),
        }
        if let Ok(songs) = self.api.songs() {
            self.songs = songs;
        }
    }

    fn record_on(&mut self) {
        let req = TransportRequest {
            axes: Some(self.record_axes.clone()),
            camera: Some(self.camera.clone()),
            ..TransportRequest::default()
        };
        if self.transport(Action::RecordOn, req) {
            let axes: Vec<&str> = Axis::ALL.iter().filter(|a| self.record_axes.contains(a)).map(|a| a.label()).collect();
            self.say(format!(
                "Record is on for {}: {}. Play, then drive by hand",
                self.project.label(&self.camera),
                axes.join(", ")
            ));
        }
    }

    fn toggle_axis(&mut self, axis: Axis) {
        let (recording, running) =
            self.status.as_ref().map(|s| (s.transport.record, s.transport.run != Run::Stopped)).unwrap_or_default();
        if recording && running {
            return self.refuse("Stop to change which axes Record writes");
        }
        let mut axes = self.record_axes.clone();
        if axes.contains(&axis) {
            axes.retain(|a| *a != axis);
        } else {
            axes.push(axis);
        }
        if recording {
            if axes.is_empty() {
                return self.refuse("Record needs at least one axis. Turn Record off instead");
            }
            // The server takes the axes when Record turns on.
            if !self.transport(Action::RecordOff, TransportRequest::default()) {
                return;
            }
            self.record_axes = axes;
            self.record_on();
        } else {
            self.record_axes = axes;
        }
    }

    // ---- time ----

    fn tick(&mut self) {
        let now = Instant::now();
        // The status stream says when a restarted server is back. A single call that failed
        // while the stream stayed up would otherwise leave the panel dark for good.
        if !self.server_up && now.duration_since(self.server_asked) >= Duration::from_secs(2) {
            self.server_asked = now;
            self.refresh();
        }
        // A hold counts only once the panel has been heard from after its time is up:
        // a release that the link lost on the way must not turn into a full hold.
        if let Some((since, true, false)) = self.estop_down {
            if self.heard >= since + HOLD_RESET {
                self.estop_down = Some((since, true, true));
                if self.transport(Action::ResetEstop, TransportRequest::default()) {
                    self.say("E-Stop is reset");
                }
            }
        }
        self.wind_on(now);
        let due = self.holding.as_ref().is_some_and(|h| !h.fired && self.heard >= h.since + h.need);
        if due {
            let act = self.holding.as_mut().map(|h| {
                h.fired = true;
                h.act.clone()
            });
            if let Some(act) = act {
                self.perform(act, false);
            }
        }
        if self.asking.as_ref().is_some_and(|a| now >= a.until) {
            self.asking = None;
            self.say("Not confirmed in time. Nothing was changed");
        }
        if let Some(m) = &self.message {
            let pages = wrap(&m.text, WIDE).len().div_ceil(2).max(1) as u32;
            if now.duration_since(m.since) >= MESSAGE_MIN.max(PAGE_TIME * pages) {
                self.message = None;
            }
        }
        // A panel that stays away gives the lease back, so another station can carry on.
        if let Some(lost) = self.panel_lost {
            if !self.grace_spent && now.duration_since(lost) >= self.lease_grace {
                self.grace_spent = true;
                if self.server_up && self.holds_lease() {
                    self.log("the panel has been away too long: releasing the edit lease");
                    match self.api.release_lease() {
                        Ok(status) => self.take_status(status),
                        Err(e) => self.failed(e),
                    }
                }
            }
        }
    }

    fn sync_hand(&mut self) {
        let enabled = self.hand_mode() && self.server_up && self.panel_up;
        // A key that is down for a hold or a wind: the panel is watched closely until it is let go.
        let counting = self.holding.as_ref().is_some_and(|h| !h.fired)
            || matches!(self.estop_down, Some((_, true, false)))
            || self.wind.is_some();
        self.shared.watch.store(counting, Ordering::SeqCst);
        let handed_over = {
            let mut hand = self.shared.hand();
            // The joystick is given the rig, or another camera, while it is already over.
            let handed_over = enabled && (!hand.enabled || hand.camera != self.camera) && hand.stick_deflected();
            if handed_over {
                hand.rest_first = true;
            }
            hand.enabled = enabled;
            hand.server_up = self.server_up;
            hand.speed = self.speed;
            if hand.camera != self.camera {
                hand.camera = self.camera.clone();
            }
            handed_over
        };
        if handed_over {
            self.say(REST_FIRST);
        }
    }

    fn report(&mut self) {
        if !self.server_up {
            return;
        }
        let state = PanelState {
            station: self.station.clone(),
            connected: self.panel_up,
            model: self.panel_model.clone(),
            name: self.panel_name.clone(),
            mode: self.mode.name().to_string(),
            camera: self.camera.clone(),
            clips: self.clip.iter().chain(self.media_clip.iter()).cloned().collect(),
            markers: Vec::new(),
            message: self.said(),
        };
        let now = Instant::now();
        let due = match &self.last_report {
            None => true,
            Some((at, last)) => {
                let since = now.duration_since(*at);
                since >= Duration::from_secs(1) || (*last != state && since >= Duration::from_millis(100))
            }
        };
        if due {
            self.last_report = Some((now, state.clone()));
            if let Err(ApiErr::Network(msg)) = self.api.put_panel(&state) {
                self.server_lost(msg);
            }
        }
    }

    /// What the main display is saying, in full.
    fn said(&self) -> String {
        if let Some((_, true, false)) = self.estop_down {
            return "Keep holding E-Stop to reset it".into();
        }
        if let Some(h) = self.holding.as_ref().filter(|h| !h.fired) {
            return format!("{}. Keep holding", h.text);
        }
        if let Some(a) = &self.asking {
            return a.question.clone();
        }
        if let Some(winding) = self.winding() {
            return winding;
        }
        self.message.as_ref().map(|m| m.text.clone()).unwrap_or_default()
    }

    // ---- display ----

    fn status_text(&self) -> Text {
        let wide = |title: &str, line1: &str, line2: &str| Text {
            title: fit(title, WIDE),
            label: false,
            line1: fit(line1, WIDE),
            line2: fit(line2, WIDE),
            small: true,
        };
        if !self.server_up {
            return wide("No server", "Reconnecting", &self.server_error);
        }
        let Some(s) = &self.status else { return wide("PTZ Programmer", "Starting", "") };
        let title = match &s.song {
            Some(song) => format!("{}{}", fit(&song.name, WIDE - 1), if song.dirty { "*" } else { "" }),
            None => "No song".to_string(),
        };
        let line1 = format!("{} {}", s.timecode, transport_word(s));
        let mut lines: Vec<String> = Vec::new();
        lines.push(format!("{}, {}", self.mode.name(), self.project.label(&self.camera)));
        lines.push(match &s.lease {
            Some(_) if s.you_hold_lease => format!("Lease: {} (panel)", self.station),
            Some(l) => format!("Lease: {}", l.holder),
            None => "Lease: free".into(),
        });
        if let Some(song) = &s.song {
            lines.push(if song.dirty {
                "Unsaved changes".into()
            } else if song.committed_clean {
                format!("Revision {}", song.revision.unwrap_or(0))
            } else {
                "Saved, not committed".into()
            });
            lines.push(match (song.issues, song.blocking) {
                (0, _) => "No problems".into(),
                (n, 0) => format!("{n} problems, accepted"),
                (n, b) => format!("{n} problems, {b} block"),
            });
        }
        lines.push(match &s.lock.source {
            Some(source) => {
                let label =
                    self.lock_sources().into_iter().find(|(k, _)| k == source).map(|(_, l)| l).unwrap_or_else(|| source.clone());
                format!("Lock: {label}")
            }
            None => "Free run".into(),
        });
        lines.push(match &s.audio.output {
            Some(device) => format!("Audio: {device}"),
            None => "Audio: silent".into(),
        });
        if let Some(fault) = &s.telemetry.fault {
            lines.push(format!("FAULT {fault}"));
        }
        let turn = (self.started.elapsed().as_secs() / 2) as usize % lines.len();
        wide(&title, &line1, &lines[turn])
    }

    /// The wide tile: a question, a hold, a message, or what the mode is looking at.
    fn message_text(&self) -> Text {
        let paged = |title: &str, text: &str, since: Instant| {
            let lines = wrap(text, WIDE);
            let pages = lines.len().div_ceil(2).max(1);
            let page = (since.elapsed().as_millis() / PAGE_TIME.as_millis()) as usize % pages;
            let title = if pages > 1 { format!("{title} {}/{pages}", page + 1) } else { title.to_string() };
            Text {
                title,
                label: false,
                line1: lines.get(page * 2).cloned().unwrap_or_default(),
                line2: lines.get(page * 2 + 1).cloned().unwrap_or_default(),
                small: true,
            }
        };
        if let Some((since, true, false)) = self.estop_down {
            return paged("Hold", &self.said(), since);
        }
        if let Some(h) = self.holding.as_ref().filter(|h| !h.fired) {
            return paged("Hold", &self.said(), h.since);
        }
        if let Some(a) = &self.asking {
            return paged("Confirm?", &a.question, a.until - CONFIRM_TIME);
        }
        if let (Some(winding), Some(w)) = (self.winding(), &self.wind) {
            return paged("Wind", &winding, w.since);
        }
        if let Some(m) = &self.message {
            return paged(if m.kind == Kind::Refused { "Refused" } else { "Note" }, &m.text, m.since);
        }
        if !self.server_up {
            return paged(
                "No server",
                &format!("{}. E-Stop cannot reach the rig from this panel until the server answers", self.server_error),
                self.started,
            );
        }
        let (title, text) = self.context();
        paged(&title, &text, self.started)
    }

    /// What the mode is looking at, when there is nothing more urgent to say.
    fn context(&self) -> (String, String) {
        let Some(s) = &self.status else { return (String::new(), String::new()) };
        match self.mode {
            Mode::Show if self.song_list => ("Songs".into(), format!("{} songs. Press one to load it", self.songs.len())),
            Mode::Show | Mode::Transport => (self.mode.name().into(), s.guidance.hint.clone()),
            Mode::Program if s.transport.output != Output::Rehearse => ("Program".into(), ARMED_PROGRAM.into()),
            Mode::Program => {
                let marked: Vec<String> = self
                    .project
                    .cameras
                    .iter()
                    .enumerate()
                    .filter_map(|(i, c)| {
                        self.marks
                            .get(&c.id)
                            .map(|m| format!("Cam {} to {} from {}", i + 1, m.preset.name, self.timecode(m.frame)))
                    })
                    .collect();
                if marked.is_empty() {
                    (
                        "Program".into(),
                        "Set a camera, then press its number. Go to where it must have arrived and press it again".into(),
                    )
                } else {
                    (
                        format!("Marked {}", marked.len()),
                        format!("{}. Press the number again where it must have arrived", marked.join(". ")),
                    )
                }
            }
            Mode::Camera | Mode::Presets | Mode::Record => {
                let index = self.camera_index();
                let (word, _) = self.camera_state(index);
                let fault = s.cameras.get(index).and_then(|c| c.fault.clone()).map(|f| format!(". {f}")).unwrap_or_default();
                let extra = match self.mode {
                    Mode::Presets => format!(". {} presets", self.camera_presets().len()),
                    Mode::Record if s.transport.record => format!(". Record is on, {} frames", s.record.frames),
                    Mode::Record => ". Record is off".into(),
                    _ => String::new(),
                };
                (
                    format!("Camera {}", index + 1),
                    format!("{}: {word}{fault}{extra}", self.project.cameras.get(index).map(|c| c.name.as_str()).unwrap_or("")),
                )
            }
            Mode::Timeline => match self.selected() {
                Some(c) => {
                    let lane = self.lane();
                    let at = lane.iter().position(|l| l.id == c.id).map(|i| i + 1).unwrap_or(0);
                    let time = match &c.body {
                        ClipBody::PresetMove { auto_duration: true, ease, .. } => format!("auto time, {}", ease.label()),
                        ClipBody::PresetMove { ease, .. } => format!("custom time, {}", ease.label()),
                        ClipBody::Keys { .. } => "keys".into(),
                    };
                    let problems = self.problems_of(&c.id);
                    let problem = match (problems.first(), &c.accepted) {
                        (Some(p), Some(_)) => format!(". Accepted: {}", p.message()),
                        (Some(p), None) => format!(". PROBLEM {}", p.message()),
                        (None, _) => String::new(),
                    };
                    (
                        format!("Clip {at}/{}", lane.len()),
                        format!(
                            "{} at {}, {}, {time}{problem}",
                            c.label(),
                            self.timecode(c.start_frame),
                            seconds(c.duration_frames, self.fps())
                        ),
                    )
                }
                None => (
                    "Timeline".into(),
                    format!("{} clips for {}. Prev or Next selects one", self.lane().len(), self.project.label(&self.camera)),
                ),
            },
            Mode::Media => match self.selected_media() {
                Some(c) => (
                    "Media clip".into(),
                    format!(
                        "{} at {}, {}{}",
                        c.name,
                        self.timecode(c.start_frame),
                        seconds(c.duration(), self.fps()),
                        if c.muted { ", muted" } else { "" }
                    ),
                ),
                None => ("Media".into(), format!("{} files imported. Press one to place it at the playhead", self.media.len())),
            },
        }
    }

    fn estop_face(&self) -> Face {
        let face = |text: &str| Face::new("E-STOP", text, Act::None);
        if !self.server_up {
            // Honest: from here E-Stop cannot reach the rig now. A press is still tried.
            return face("No server").on(Color::Amber);
        }
        if self.estop_waiting {
            return face("Sending").color(Color::Red).lit(Lit::Blink);
        }
        if self.status.as_ref().is_some_and(|s| s.transport.estop) {
            return face("Hold: reset").color(Color::Red).lit(Lit::Blink);
        }
        face("Press").on(Color::Red)
    }

    fn render(&mut self) {
        let mut cells: BTreeMap<u32, Cell> = BTreeMap::new();
        let map = &self.map;
        let put = |cells: &mut BTreeMap<u32, Cell>, role: Role, face: &Face| {
            if let Some(hwc) = map.hwc(role) {
                let cell = cells.entry(hwc).or_default();
                cell.lit = face.lit;
                cell.color = face.color;
            }
            if let Some(hwc) = map.text_hwc(role) {
                cells.entry(hwc).or_default().text =
                    if face.title.is_empty() && face.line1.is_empty() { Text::default() } else { face.text() };
            }
        };
        put(&mut cells, Role::Estop, &self.estop_face());
        put(&mut cells, Role::Go, &self.go_face());
        for (i, face) in self.soft_keys().iter().enumerate() {
            put(&mut cells, Role::Key(i as u8 + 1), face);
        }
        let light = |cells: &mut BTreeMap<u32, Cell>, role: Role, lit: Lit, color: Color| {
            if let Some(hwc) = map.hwc(role) {
                let cell = cells.entry(hwc).or_default();
                cell.lit = lit;
                cell.color = color;
            }
        };
        for mode in Mode::ALL {
            // Without a button of its own, Program lights the Presets button it shares.
            let on = mode == self.mode || (mode == Mode::Presets && self.mode == Mode::Program && !self.program_has_a_button());
            light(&mut cells, Role::Mode(mode), if on { Lit::On } else { Lit::Dim }, if on { Color::Blue } else { Color::White });
        }
        light(&mut cells, Role::Shift, if self.shift { Lit::On } else { Lit::Dim }, Color::White);
        for n in 1..=CAMERA_KEYS {
            let face = if self.server_up && self.status.is_some() { self.camera_key(n) } else { Face::dark() };
            light(&mut cells, Role::Cam(n), face.lit, face.color);
        }
        for (role, step) in [(Role::PageUp, -1), (Role::PageDown, 1)] {
            let face = if self.server_up { self.page_face(step) } else { Face::dark() };
            light(&mut cells, role, face.lit, face.color);
        }
        let seekable = self.server_up && self.song.is_some();
        light(&mut cells, Role::Playhead, if seekable { Lit::Dim } else { Lit::Off }, Color::White);
        for (i, knob) in self.knobs().iter().enumerate() {
            let n = i as u8 + 1;
            let used = knob.knob != Knob::None;
            light(
                &mut cells,
                Role::Enc(n),
                if used { Lit::Dim } else { Lit::Off },
                if used { Color::White } else { Color::Default },
            );
            if let Some(hwc) = map.hwc(Role::EncDisplay(n)) {
                cells.entry(hwc).or_default().text = if used {
                    Text {
                        title: fit(&knob.title, NARROW),
                        label: false,
                        line1: fit(&knob.value, 11),
                        line2: String::new(),
                        small: true,
                    }
                } else {
                    Text::default()
                };
            }
        }
        if let Some(hwc) = map.hwc(Role::Status) {
            cells.entry(hwc).or_default().text = self.status_text();
        }
        if let Some(hwc) = map.hwc(Role::Message) {
            cells.entry(hwc).or_default().text = self.message_text();
        }
        if cells != self.rendered {
            let mut surface = self.shared.surface();
            surface.0 += 1;
            surface.1 = cells.clone();
            drop(surface);
            self.rendered = cells;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_wraps_at_spaces_and_breaks_a_word_that_is_too_long() {
        assert_eq!(wrap("Take the edit lease to control the show", 21), ["Take the edit lease", "to control the show"]);
        assert_eq!(wrap("abcdefghijklmnopqrstuvwxyz now", 10), ["abcdefghij", "klmnopqrst", "uvwxyz now"]);
        assert!(wrap("", 10).is_empty());
        assert_eq!(two_lines("Play to code", 10), ("Play to".to_string(), "code".to_string()));
    }

    #[test]
    fn a_new_song_starts_ten_minutes_after_the_latest_as_in_the_app() {
        let project = Project::default();
        let song = |start: &str| SongSummary {
            id: "s".into(),
            name: "S".into(),
            start_tc: start.into(),
            length_frames: 100,
            revision: None,
            dirty: false,
            loaded: false,
        };
        assert_eq!(next_start(&[], &project), "01:00:00:00");
        assert_eq!(next_start(&[song("01:00:00:00")], &project), "01:10:00:00");
        assert_eq!(next_start(&[song("01:10:00:00"), song("01:03:20:00")], &project), "01:20:00:00");
    }
}
