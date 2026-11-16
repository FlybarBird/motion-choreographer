//! The show: one song loaded, one transport, up to twelve cameras. Everything that changes
//! show state goes through `Show`, behind a single mutex, so there is no lock
//! order to get wrong.

use crate::audio::{AudioEngine, LtcOut, Mix, PlayClip};
use crate::lock::{self, CodeSource, Kind};
use crate::media::{self, Ingested};
use crate::record::{self, Capture, RecordedPass};
use crate::rig::{DevicePreset, Drive, Ride, RigAdapter};
use crate::runlog::RunLog;
use crate::storage::{self, Storage};
use ptz_core::api::*;
use ptz_core::camera::OnFault;
use ptz_core::chase::{Chase, ChaseState};
use ptz_core::codes::CodeRate;
use ptz_core::compile::{compile, Compiled, Issue};
use ptz_core::edit;
use ptz_core::eval::{keys_value, Evaluator};
use ptz_core::fit::{default_tolerance, overruns};
use ptz_core::pose::{Axis, Pose, Preset, Project, Routing};
use ptz_core::song::{ClipBody, MotionClip, Song};
use ptz_core::time::{frame_to_sample, FrameRate, Timecode};
use ptz_core::transport::{self, Action, Context, Event, Output, Run, TimeMode, TransportState};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub const NO_LEASE: &str = "Take the edit lease to control the show";

/// A panel controller that has not reported for this long is gone.
const PANEL_TTL: Duration = Duration::from_secs(3);

/// Which audio output to open.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioChoice {
    /// The system's default output.
    Default,
    /// The first output whose name contains this text.
    Named(String),
    /// No device: run silent on the monotonic clock.
    Off,
}

impl AudioChoice {
    /// The output to play through: the one the project names, or else the one the server was
    /// started with. A server started with no audio stays silent whatever the project names.
    pub fn for_project(&self, project: &Project) -> AudioChoice {
        match (self, project.audio_device.trim()) {
            (AudioChoice::Off, _) | (_, "") => self.clone(),
            (_, named) => AudioChoice::Named(named.to_string()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub data: PathBuf,
    pub seed: bool,
    pub lease_ttl: Duration,
    pub audio: AudioChoice,
}

impl Config {
    pub fn new(data: impl Into<PathBuf>) -> Config {
        Config { data: data.into(), seed: true, lease_ttl: Duration::from_secs(10), audio: AudioChoice::Default }
    }
}

/// A refused request, with the HTTP status it maps to.
#[derive(Clone, PartialEq, Debug)]
pub struct Fail {
    pub code: u16,
    pub msg: String,
}

impl Fail {
    /// Policy refusal: the request was understood and is not allowed now.
    pub fn refused(msg: impl Into<String>) -> Fail {
        Fail { code: 409, msg: msg.into() }
    }
    pub fn bad(msg: impl Into<String>) -> Fail {
        Fail { code: 400, msg: msg.into() }
    }
    pub fn invalid(msg: impl Into<String>) -> Fail {
        Fail { code: 422, msg: msg.into() }
    }
    pub fn not_found(msg: impl Into<String>) -> Fail {
        Fail { code: 404, msg: msg.into() }
    }
    pub fn io(e: std::io::Error) -> Fail {
        Fail { code: 500, msg: format!("Could not write to the data folder: {e}") }
    }
}

pub type Res<T> = Result<T, Fail>;

/// What the WebSocket tasks hear about.
#[derive(Clone, Debug)]
pub enum Push {
    Msg(WsMsg),
    /// State changed; send a fresh status now instead of waiting for the timer.
    Poke,
}

pub struct Entry {
    pub song: Song,
    pub counter: u64,
    pub compiled: Compiled,
    pub draft: Option<Song>,
    pub committed: Option<(u32, Song)>,
}

impl Entry {
    pub fn dirty(&self) -> bool {
        self.draft.as_ref() != Some(&self.song)
    }

    pub fn committed_clean(&self) -> bool {
        self.committed.as_ref().is_some_and(|(_, s)| *s == self.song)
    }

    pub fn revision(&self) -> Option<u32> {
        self.committed.as_ref().map(|(n, _)| *n)
    }

    fn working(&self) -> WorkingSong {
        WorkingSong {
            song: self.song.clone(),
            edit_counter: self.counter,
            dirty: self.dirty(),
            revision: self.revision(),
            issues: self.compiled.issues.clone(),
        }
    }
}

/// A lock to LTC, MTC or Art-Net: the input, and the chase that decides when to trust it.
struct External {
    kind: Kind,
    source: Box<dyn CodeSource>,
    chase: Chase,
    /// Readings in a row whose rate family is not the project's.
    mismatched: u32,
    /// The chase state last written to the run log.
    logged: ChaseState,
    /// What the input last said was wrong with it, as written to the run log.
    problem: Option<String>,
}

/// Readings of the wrong rate family before Lock is given up. One could be a damaged frame.
const MISMATCH_READINGS: u32 = 3;

/// A start timecode to put on a media file.
#[derive(Clone, PartialEq, Debug)]
pub enum Stamp {
    /// What the file itself carries, or nothing: undoes a typed or decoded stamp.
    FileOwn {
        start_tc: Option<String>,
        source: Option<String>,
    },
    Typed(Timecode),
    /// Decoded from LTC on a channel (0 is the first), which then leaves Program.
    Ltc {
        channel: u32,
        start: Timecode,
    },
}

/// A file's sound read for playing: the file, the channel that was left out as timecode, and the mono sound.
type LoadedAudio = (String, Option<u32>, Result<Vec<f32>, String>);

struct Lease {
    client: String,
    name: String,
    last_seen: Instant,
}

pub struct Show {
    pub storage: Storage,
    pub project: Project,
    pub presets: Vec<Preset>,
    pub songs: BTreeMap<String, Entry>,
    pub current: Option<String>,
    pub transport: TransportState,
    pub media: Vec<MediaInfo>,
    /// Ids handed to imports that are still decoding.
    media_reserved: HashSet<String>,
    audio: AudioEngine,
    /// The output the server was started with: what an empty choice in the project means.
    audio_started_with: AudioChoice,
    /// Each media file as one channel at the device rate, ready to play.
    audio_cache: HashMap<String, Arc<Vec<f32>>>,
    audio_loading: HashSet<String>,
    audio_failed: HashSet<String>,
    loaded_tx: mpsc::Sender<LoadedAudio>,
    loaded_rx: mpsc::Receiver<LoadedAudio>,
    audio_drift: f64,
    clock: SteadyClock,
    frame_f: f64,
    lease: Option<Lease>,
    lease_ttl: Duration,
    /// One per camera of the project, in the project's order.
    units: Vec<Unit>,
    ticks: u64,
    /// The camera Record writes for.
    record_camera: String,
    /// Axes that Record writes.
    record_axes: Vec<Axis>,
    /// One capture per camera while Record and Play are both on. A pass is kept for each camera that is moved.
    captures: Vec<Capture>,
    lock_source: Option<String>,
    external: Option<External>,
    /// Why the last lock was refused or let go, until the operator's next action.
    lock_problem: Option<String>,
    /// What the hardware panel controller last reported, who sent it and when.
    panel: Option<(PanelState, String, Instant)>,
    /// The panel this server runs for itself, when it was asked to run one.
    panel_host: Option<PanelHost>,
    started: Instant,
    last_tick: Instant,
    log: RunLog,
    tx: broadcast::Sender<Push>,
}

pub type Shared = Arc<Mutex<Show>>;

/// One camera's rig, and what the show remembers about it.
struct Unit {
    /// The camera's id in the project.
    id: String,
    rig: Box<dyn RigAdapter>,
    live: Live,
    /// Faulted while live and left standing where it stopped, while the other cameras carry on.
    held: bool,
    /// E-Stop was pressed at this moment and the rig has not yet read as standing still.
    halting: Option<Instant>,
    /// Which adapters answered when last looked at, to log each connect and disconnect.
    adapters_were: Vec<(String, bool)>,
    jog_deadline: Option<Instant>,
    /// The simulator was positioned by hand; the monitor shows it until the playhead moves.
    manual: bool,
}

impl Unit {
    fn new(project: &Project, camera: &ptz_core::camera::CameraConfig) -> Unit {
        Unit {
            id: camera.id.clone(),
            rig: crate::hw::build(&project.rig_of(camera), &camera.axes),
            live: Live::default(),
            held: false,
            halting: None,
            adapters_were: Vec::new(),
            jog_deadline: None,
            manual: false,
        }
    }
}

/// What one turn of the live loop found for one camera.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Step {
    /// Positioning, and at the pose under the playhead.
    Arrived,
    /// Positioning, and still on its way.
    Travelling,
    Tracking,
    Faulted,
}

/// A project file edited by hand can name more cameras than the show takes, or the same one twice.
fn usable(mut project: Project) -> Project {
    if let Err(why) = project.validate() {
        eprintln!("warning: project.json: {why}");
        let mut seen = HashSet::new();
        project.cameras.retain(|c| !c.id.is_empty() && seen.insert(c.id.clone()));
        project.cameras.truncate(ptz_core::camera::MAX_CAMERAS);
        if project.cameras.is_empty() {
            project.cameras.push(ptz_core::camera::CameraConfig::default());
        }
    }
    project
}

/// A stage model that has been received and checked, waiting to replace the one in use.
pub struct StageUpload {
    pub name: String,
    pub unit: ptz_core::camera::Unit,
    pub up: ptz_core::camera::UpAxis,
    pub triangles: u64,
    pub min: [f64; 3],
    pub max: [f64; 3],
    /// The mesh file, written in the stage folder under a temporary name.
    pub file: PathBuf,
}

/// What the live loop remembers between ticks.
#[derive(Default)]
struct Live {
    /// Where the rig has been told to be: the timeline pose, held to each axis's top speed.
    commanded: Option<Pose>,
    /// Recent commanded poses, to compare a reading with what was asked when it was taken.
    history: VecDeque<(Instant, Pose)>,
    /// Since when the rig has been outside tolerance.
    over_since: Option<Instant>,
    /// The pose being positioned to, and when the rig should have reached it.
    positioning: Option<(Pose, Instant)>,
    /// The rig ran out of time on the way to that pose and was let through.
    late: bool,
    /// Rig position minus commanded position.
    error: Pose,
    /// Why output was last cut, until the operator arms or plays again.
    fault: Option<String>,
    pose_logged: Option<Instant>,
}

impl Live {
    fn clear(&mut self) {
        let fault = self.fault.take();
        *self = Live { fault, ..Live::default() };
    }

    /// The commanded pose at `t`, by straight lines between the ticks either side.
    fn commanded_at(&self, t: Instant) -> Option<Pose> {
        let i = self.history.partition_point(|(at, _)| *at <= t);
        match (i.checked_sub(1).and_then(|j| self.history.get(j)), self.history.get(i)) {
            (Some((t0, a)), Some((t1, b))) => {
                let span = t1.duration_since(*t0).as_secs_f64();
                let share = if span > 0.0 { t.duration_since(*t0).as_secs_f64() / span } else { 1.0 };
                Some(lerp_pose(a, b, share))
            }
            (Some((_, only)), None) | (None, Some((_, only))) => Some(*only),
            (None, None) => None,
        }
    }
}

/// How long positioning may take beyond the time the travel needs.
const POSITIONING_GRACE: Duration = Duration::from_secs(3);
/// E-Stop's stop is sent again if the rig still reads as moving after this long.
const HALT_AGAIN: Duration = Duration::from_secs(2);

/// What an import needs to do its work without holding the show.
#[derive(Clone, Debug)]
pub struct MediaTicket {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    /// Folder for the upload while it arrives.
    pub incoming: PathBuf,
    pub rate: FrameRate,
    pub device_rate: u32,
}

fn lerp_pose(a: &Pose, b: &Pose, t: f64) -> Pose {
    Pose::from_fn(|ax| a.get(ax) + (b.get(ax) - a.get(ax)) * t)
}

impl Show {
    pub fn open(cfg: &Config) -> std::io::Result<(Show, broadcast::Receiver<Push>)> {
        let storage = Storage::open(&cfg.data)?;
        if cfg.seed && storage.is_fresh() {
            crate::seed::seed(&storage)?;
        }
        let mut project = usable(storage.load_project());
        if project.stage.model.is_some() && !storage.stage_model_path().is_file() {
            eprintln!("warning: the stage model file is missing; the built-in stage is shown instead");
            project.stage = ptz_core::camera::Stage::default();
        }
        if storage.is_fresh() {
            storage.save_project(&project)?;
        }
        let presets = storage.load_presets();
        let mut songs = BTreeMap::new();
        for id in storage.song_ids() {
            let Some(song) = storage.load_working(&id) else {
                eprintln!("warning: songs/{id}/working.json does not parse; skipped");
                continue;
            };
            let compiled = compile(&song, &project);
            let draft = storage.load_draft(&id);
            let committed = storage.latest_revision(&id).map(|(info, s)| (info.number, s));
            songs.insert(id, Entry { song, counter: 0, compiled, draft, committed });
        }
        let current = storage.load_current().filter(|id| songs.contains_key(id)).or_else(|| songs.keys().next().cloned());
        let (tx, rx) = broadcast::channel(256);
        let now = Instant::now();
        let mut log = RunLog::open(&storage.log_dir());
        log.append("server_start", json!({ "version": env!("CARGO_PKG_VERSION"), "songs": songs.len() }));
        let media = media::load_all(&storage.media_root(), project.rate);
        let audio = AudioEngine::start(cfg.audio.for_project(&project));
        let (loaded_tx, loaded_rx) = mpsc::channel();
        let units = project.cameras.iter().map(|c| Unit::new(&project, c)).collect();
        let record_camera = project.first().id.clone();
        let mut show = Show {
            storage,
            project,
            presets,
            songs,
            current,
            transport: TransportState::IDLE,
            media,
            media_reserved: HashSet::new(),
            audio,
            audio_started_with: cfg.audio.clone(),
            audio_cache: HashMap::new(),
            audio_loading: HashSet::new(),
            audio_failed: HashSet::new(),
            loaded_tx,
            loaded_rx,
            audio_drift: 0.0,
            clock: SteadyClock::default(),
            frame_f: 0.0,
            lease: None,
            lease_ttl: cfg.lease_ttl,
            units,
            ticks: 0,
            record_camera,
            record_axes: vec![Axis::Pan, Axis::Tilt, Axis::Zoom, Axis::Elevation],
            captures: Vec::new(),
            lock_source: None,
            external: None,
            lock_problem: None,
            panel: None,
            panel_host: None,
            started: now,
            last_tick: now,
            log,
            tx,
        };
        show.rebuild_mix();
        Ok((show, rx))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Push> {
        self.tx.subscribe()
    }

    fn push(&self, msg: WsMsg) {
        let _ = self.tx.send(Push::Msg(msg));
    }

    fn poke(&self) {
        let _ = self.tx.send(Push::Poke);
    }

    fn log(&mut self, kind: &str, data: serde_json::Value) {
        let record = self.log.append(kind, data);
        self.push(WsMsg::Log { record });
    }

    pub fn log_since(&self, seq: u64) -> Vec<LogRecord> {
        self.log.since(seq)
    }

    pub fn rate(&self) -> FrameRate {
        self.project.rate
    }

    fn entry(&self) -> Option<&Entry> {
        self.current.as_ref().and_then(|id| self.songs.get(id))
    }

    fn length(&self) -> i64 {
        self.entry().map(|e| e.song.length_frames).unwrap_or(0)
    }

    pub fn is_playing(&self) -> bool {
        self.transport.run != Run::Stopped
            || self.transport.output == Output::Positioning
            || self.units.iter().any(|u| u.rig.is_moving())
    }

    // ---- cameras ----

    /// The unit of a camera, or of the first when none is named.
    fn unit_index(&self, camera: Option<&str>) -> Res<usize> {
        match camera {
            None => Ok(0),
            Some(id) => {
                self.units.iter().position(|u| u.id == id).ok_or_else(|| Fail::not_found(format!("No camera with id {id}")))
            }
        }
    }

    /// `"3 Stage left: "` in front of a message about a camera, when there are several to tell apart.
    fn prefix(&self, i: usize) -> String {
        self.project.prefix(&self.units[i].id)
    }

    fn clear_manual(&mut self) {
        for u in &mut self.units {
            u.manual = false;
        }
    }

    // ---- lease ----

    pub fn holds_lease(&self, client: &str) -> bool {
        self.lease.as_ref().is_some_and(|l| l.client == client)
    }

    pub fn require_lease(&self, client: &str) -> Res<()> {
        if self.holds_lease(client) {
            Ok(())
        } else {
            Err(Fail::refused(NO_LEASE))
        }
    }

    /// Any request or heartbeat from the holder keeps the lease alive.
    pub fn touch(&mut self, client: &str) {
        if let Some(l) = &mut self.lease {
            if l.client == client {
                l.last_seen = Instant::now();
            }
        }
    }

    pub fn take_lease(&mut self, client: &str, req: &LeaseRequest) -> Res<()> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Fail::bad("Give this station a name"));
        }
        if client.is_empty() {
            return Err(Fail::bad("The x-client-id header is required to hold the lease"));
        }
        if let Some(l) = &self.lease {
            if l.client != client && !req.force {
                return Err(Fail::refused(format!("{} holds the edit lease. Take it over to edit", l.name)));
            }
        }
        let previous = self.lease.as_ref().filter(|l| l.client != client).map(|l| l.name.clone());
        self.lease = Some(Lease { client: client.to_string(), name: name.to_string(), last_seen: Instant::now() });
        self.log("lease", json!({ "holder": name, "took_from": previous }));
        self.poke();
        Ok(())
    }

    pub fn release_lease(&mut self, client: &str) -> Res<()> {
        if self.holds_lease(client) {
            let name = self.lease.take().map(|l| l.name);
            self.log("lease", json!({ "released_by": name }));
            self.poke();
        }
        Ok(())
    }

    // ---- hardware panel ----

    /// A panel controller says what it is doing. It keeps saying so about once a
    /// second; `tick` forgets it when it stops.
    pub fn set_panel(&mut self, client: &str, state: PanelState) -> Res<PanelState> {
        if state.station.trim().is_empty() {
            return Err(Fail::bad("Give the panel a station name"));
        }
        let was = self.panel.as_ref().map(|(p, _, _)| p);
        let arrived = was.is_none_or(|p| p.station != state.station || p.connected != state.connected);
        let changed = was != Some(&state);
        if arrived {
            self.log(
                "panel",
                json!({ "station": state.station, "connected": state.connected, "model": state.model, "name": state.name }),
            );
        }
        self.panel = Some((state.clone(), client.to_string(), Instant::now()));
        if changed {
            self.poke();
        }
        Ok(state)
    }

    /// The panel this server runs for itself, or `None` when it runs none.
    pub fn set_panel_host(&mut self, host: Option<PanelHost>) {
        if self.panel_host != host {
            self.log("panel_host", json!({ "host": host }));
            self.panel_host = host;
            self.poke();
        }
    }

    /// The server stopped the controller it was running: its last report is not kept until it times out.
    pub fn forget_panel(&mut self) {
        if let Some((state, _, _)) = self.panel.take() {
            self.log("panel", json!({ "station": state.station, "gone": true }));
            self.poke();
        }
    }

    // ---- songs ----

    pub fn list_songs(&self) -> Vec<SongSummary> {
        self.songs
            .values()
            .map(|e| SongSummary {
                id: e.song.id.clone(),
                name: e.song.name.clone(),
                start_tc: e.song.start_tc.clone(),
                length_frames: e.song.length_frames,
                revision: e.revision(),
                dirty: e.dirty(),
                loaded: self.current.as_deref() == Some(e.song.id.as_str()),
            })
            .collect()
    }

    fn song_entry(&self, id: &str) -> Res<&Entry> {
        self.songs.get(id).ok_or_else(|| Fail::not_found(format!("No song with id {id}")))
    }

    pub fn working(&self, id: &str) -> Res<WorkingSong> {
        Ok(self.song_entry(id)?.working())
    }

    pub fn create_song(&mut self, client: &str, req: &NewSong) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Fail::bad("Give the song a name"));
        }
        let rate = self.rate();
        let start = req.start_tc.clone().unwrap_or_else(|| "01:00:00:00".into());
        let Some(tc) = Timecode::parse_at(&start, rate) else {
            return Err(Fail::invalid(format!("{start} is not a timecode at {} fps", rate.label())));
        };
        let id = self.storage.unique_song_id(name);
        let length = (rate.fps() * 60.0).round() as i64;
        let song = Song::new(&id, name, &tc.to_string(), length);
        self.storage.save_working(&song).map_err(Fail::io)?;
        self.storage.save_draft(&song).map_err(Fail::io)?;
        let compiled = compile(&song, &self.project);
        let entry = Entry { song: song.clone(), counter: 0, compiled, draft: Some(song), committed: None };
        let out = entry.working();
        self.songs.insert(id.clone(), entry);
        if self.current.is_none() {
            self.current = Some(id.clone());
            self.reset_for_new_song();
        }
        self.log("song_created", json!({ "song": id }));
        self.push(WsMsg::SongListChanged);
        Ok(out)
    }

    fn require_parked(&self, what: &str) -> Res<()> {
        if self.transport.run != Run::Stopped {
            return Err(Fail::refused(format!("Stop before {what}")));
        }
        if self.transport.output != Output::Rehearse {
            return Err(Fail::refused(format!("Disarm before {what}")));
        }
        Ok(())
    }

    /// Load a song. Stopped is enough: a show that is armed stays armed and its rig travels to
    /// the new song, and a lock to outside code is kept, so the set list can be stepped through.
    pub fn load_song(&mut self, client: &str, id: &str) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let out = self.song_entry(id)?.working();
        if self.current.as_deref() != Some(id) {
            if self.transport.run != Run::Stopped {
                return Err(Fail::refused("Stop before loading another song"));
            }
            if self.transport.output != Output::Rehearse {
                self.fit_to_go_live(id).map_err(|why| Fail::refused(format!("{why}. Disarm to load it")))?;
            }
            let who = self.who(client);
            self.change_song(id, &who);
        }
        Ok(out)
    }

    /// Why a song cannot be loaded into a show that is armed. Arm asks the same of the song it arms.
    fn fit_to_go_live(&self, id: &str) -> Result<(), String> {
        let Some(e) = self.songs.get(id) else { return Err(format!("There is no song {id}")) };
        if !e.committed_clean() {
            return Err(format!("{} is not committed", e.song.name));
        }
        match e.compiled.blocking_count(&e.song) {
            0 => Ok(()),
            1 => Err(format!("{} has a timeline issue to fix or accept", e.song.name)),
            n => Err(format!("{} has {n} timeline issues to fix or accept", e.song.name)),
        }
    }

    /// Make another song the loaded one, at its start. A lock to outside code is kept, with Play
    /// still pressed if it was. An armed show stays armed: the rig travels to the new song's pose
    /// under the playhead, as it does after a seek.
    fn change_song(&mut self, id: &str, by: &str) {
        let before = self.transport;
        let kept = (before.time == TimeMode::LockedExternal).then(|| (self.external.take(), self.lock_source.take()));
        self.current = Some(id.to_string());
        self.reset_for_new_song();
        let mut after = self.transport;
        if let Some((external, source)) = kept {
            (self.external, self.lock_source) = (external, source);
            after.time = TimeMode::LockedExternal;
            if before.run != Run::Stopped {
                after.run = Run::WaitingForCode;
            }
        }
        if before.output != Output::Rehearse {
            after.output = Output::Positioning;
        }
        self.transport = after;
        let revision = self.entry().and_then(|e| e.revision());
        self.log("song_loaded", json!({ "song": id, "by": by, "revision": revision, "state": after }));
        self.sync_capture();
        self.push(WsMsg::SongListChanged);
        self.poke();
    }

    /// Code has left the loaded song. With Play pressed, load the song the code is in now: the
    /// first such in the set list, or among every song while the set list is empty. True when
    /// another song was loaded.
    fn follow_code(&mut self, code: f64) -> bool {
        if self.transport.run == Run::Stopped || self.transport.record {
            return false;
        }
        let rate = self.rate();
        let holds = |e: &Entry| (0.0..e.song.length_frames as f64).contains(&e.song.clock(rate).offset_of(code));
        let listed: Vec<&String> =
            if self.project.set_list.is_empty() { self.songs.keys().collect() } else { self.project.set_list.iter().collect() };
        let next = listed
            .into_iter()
            .find(|id| self.current.as_deref() != Some(id.as_str()) && self.songs.get(*id).is_some_and(holds))
            .cloned();
        let Some(id) = next else { return false };
        if self.transport.output != Output::Rehearse {
            if let Err(why) = self.fit_to_go_live(&id) {
                // Said once, not every tick the code stays there.
                let why = format!("Code is in a song that cannot go live: {why}");
                if self.lock_problem.as_deref() != Some(why.as_str()) {
                    self.log("follow_refused", json!({ "song": id, "reason": why }));
                    self.lock_problem = Some(why);
                    self.poke();
                }
                return false;
            }
        }
        // The song the code left stops first; the next starts when the code is found inside it.
        self.event(Event::CodeLost);
        self.change_song(&id, "timecode");
        true
    }

    fn reset_for_new_song(&mut self) {
        self.audio.stop();
        self.audio.seek(0.0, self.rate());
        self.frame_f = 0.0;
        self.clear_manual();
        self.lock_source = None;
        self.external = None;
        self.lock_problem = None;
        let _ = self.storage.save_current(self.current.as_deref());
        self.transport = TransportState { estop: self.transport.estop, ..TransportState::IDLE };
        self.rebuild_mix();
    }

    pub fn delete_song(&mut self, client: &str, id: &str) -> Res<()> {
        self.require_lease(client)?;
        self.song_entry(id)?;
        if self.current.as_deref() == Some(id) {
            self.require_parked("deleting the loaded song")?;
            self.current = self.songs.keys().find(|k| k.as_str() != id).cloned();
            self.reset_for_new_song();
        }
        self.storage.delete_song(id).map_err(Fail::io)?;
        self.songs.remove(id);
        if self.project.set_position(id).is_some() {
            self.project.set_list.retain(|listed| listed != id);
            self.storage.save_project(&self.project).map_err(Fail::io)?;
            self.push(WsMsg::ProjectChanged);
        }
        self.log("song_deleted", json!({ "song": id }));
        self.push(WsMsg::SongListChanged);
        self.poke();
        Ok(())
    }

    fn require_editable(&self, id: &str) -> Res<()> {
        if self.current.as_deref() == Some(id) && self.transport.output != Output::Rehearse {
            return Err(Fail::refused("Disarm to edit the song"));
        }
        Ok(())
    }

    fn replace_working(&mut self, id: &str, song: Song, reason: &str) -> Res<WorkingSong> {
        self.storage.save_working(&song).map_err(Fail::io)?;
        let compiled = compile(&song, &self.project);
        let entry = self.songs.get_mut(id).ok_or_else(|| Fail::not_found("No such song"))?;
        entry.song = song;
        entry.compiled = compiled;
        entry.counter += 1;
        let out = entry.working();
        let counter = entry.counter;
        if self.current.as_deref() == Some(id) {
            let length = self.length() as f64;
            if self.frame_f > length {
                self.frame_f = length;
                self.audio.seek(length, self.rate());
            }
            self.rebuild_mix();
            self.check_file_lock();
        }
        self.push(WsMsg::SongChanged { song_id: id.to_string(), edit_counter: counter, reason: reason.to_string() });
        Ok(out)
    }

    pub fn put_working(&mut self, client: &str, id: &str, put: PutWorking) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let entry = self.song_entry(id)?;
        self.require_editable(id)?;
        if put.expected_counter != entry.counter {
            return Err(Fail::refused(format!(
                "This song changed on the server (edit {}, this window has edit {}). Reload it",
                entry.counter, put.expected_counter
            )));
        }
        if put.song.id != id {
            return Err(Fail::bad("The song id does not match the address"));
        }
        put.song.validate(self.rate()).map_err(|e| Fail::invalid(e.to_string()))?;
        // Accepting an overrun is a decision someone made: it goes in the run log.
        let mut decisions = Vec::new();
        for clip in &put.song.motion {
            let was = entry.song.motion_clip(&clip.id).and_then(|c| c.accepted.as_ref());
            match (&clip.accepted, was) {
                (Some(now), was) if was != Some(now) => {
                    let issues: Vec<String> = compile(&put.song, &self.project)
                        .issues
                        .iter()
                        .filter(|i| i.clip_id() == Some(clip.id.as_str()))
                        .map(|i| i.message_in(self.project.units))
                        .collect();
                    decisions.push((
                        "issue_accepted",
                        json!({ "song": id, "clip": clip.id, "by": now.by, "at": now.at, "issues": issues }),
                    ));
                }
                (None, Some(was)) => {
                    decisions.push(("acceptance_withdrawn", json!({ "song": id, "clip": clip.id, "was_by": was.by })));
                }
                _ => {}
            }
        }
        let out = self.replace_working(id, put.song, "edit")?;
        for (kind, data) in decisions {
            self.log(kind, data);
        }
        Ok(out)
    }

    pub fn save_song(&mut self, client: &str, id: &str) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let entry = self.song_entry(id)?;
        let song = entry.song.clone();
        self.storage.save_draft(&song).map_err(Fail::io)?;
        let entry = self.songs.get_mut(id).unwrap();
        entry.draft = Some(song);
        let out = entry.working();
        self.push(WsMsg::SongListChanged);
        self.poke();
        Ok(out)
    }

    /// Save the draft of every song that has unsaved changes, whichever is loaded.
    pub fn save_all(&mut self, client: &str) -> Res<SavedAll> {
        self.require_lease(client)?;
        let unsaved: Vec<String> = self.songs.iter().filter(|(_, e)| e.dirty()).map(|(id, _)| id.clone()).collect();
        let mut songs = Vec::new();
        for id in &unsaved {
            let song = self.songs[id].song.clone();
            self.storage.save_draft(&song).map_err(Fail::io)?;
            songs.push(song.name.clone());
            self.songs.get_mut(id).unwrap().draft = Some(song);
        }
        if !songs.is_empty() {
            self.push(WsMsg::SongListChanged);
            self.poke();
        }
        Ok(SavedAll { songs, working: self.entry().map(|e| e.working()) })
    }

    pub fn revert_song(&mut self, client: &str, id: &str) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let entry = self.song_entry(id)?;
        self.require_editable(id)?;
        let Some(draft) = entry.draft.clone() else {
            return Err(Fail::refused("There is no saved draft to revert to"));
        };
        let out = self.replace_working(id, draft, "revert")?;
        self.push(WsMsg::SongListChanged);
        Ok(out)
    }

    pub fn copy_song(&mut self, client: &str, id: &str, req: &NewSong) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Fail::bad("Give the copy a name"));
        }
        let mut song = self.song_entry(id)?.song.clone();
        let new_id = self.storage.unique_song_id(name);
        song.id = new_id.clone();
        song.name = name.to_string();
        self.storage.save_working(&song).map_err(Fail::io)?;
        self.storage.save_draft(&song).map_err(Fail::io)?;
        let compiled = compile(&song, &self.project);
        let entry = Entry { song: song.clone(), counter: 0, compiled, draft: Some(song), committed: None };
        let out = entry.working();
        self.songs.insert(new_id.clone(), entry);
        self.log("song_copied", json!({ "from": id, "to": new_id }));
        self.push(WsMsg::SongListChanged);
        Ok(out)
    }

    pub fn commit_song(&mut self, client: &str, id: &str) -> Res<RevisionInfo> {
        self.require_lease(client)?;
        let entry = self.song_entry(id)?;
        if entry.dirty() {
            return Err(Fail::refused("Save the song before committing"));
        }
        if entry.committed_clean() {
            return Err(Fail::refused(format!("Revision {} already holds this song", entry.revision().unwrap_or(0))));
        }
        let song = entry.song.clone();
        let by = self.lease.as_ref().map(|l| l.name.clone()).unwrap_or_default();
        let info = self.storage.commit(&song, &by).map_err(Fail::io)?;
        self.songs.get_mut(id).unwrap().committed = Some((info.number, song));
        self.log("song_committed", json!({ "song": id, "revision": info.number, "sha256": info.sha256, "by": by }));
        self.push(WsMsg::SongListChanged);
        self.poke();
        Ok(info)
    }

    pub fn revisions(&self, id: &str) -> Res<Vec<RevisionInfo>> {
        self.song_entry(id)?;
        Ok(self.storage.revisions(id))
    }

    // ---- project and presets ----

    pub fn put_project(&mut self, client: &str, mut project: Project) -> Res<Project> {
        self.require_lease(client)?;
        // The set list names songs there are, each once.
        let mut listed = HashSet::new();
        project.set_list.retain(|id| self.songs.contains_key(id) && listed.insert(id.clone()));
        if self.transport.output != Output::Rehearse {
            return Err(Fail::refused("Disarm to change project settings"));
        }
        if project.rate != self.project.rate {
            self.require_parked("changing the frame rate")?;
            if self.transport.locked() {
                return Err(Fail::refused("Unlock before changing the frame rate"));
            }
            for e in self.songs.values() {
                if let Err(err) = e.song.validate(project.rate) {
                    return Err(Fail::invalid(format!("{} does not fit {} fps: {err}", e.song.name, project.rate.label())));
                }
            }
        }
        project.validate().map_err(Fail::invalid)?;
        if !self.project.stage.same_model(&project.stage) {
            return Err(Fail::refused("The stage model is changed by importing a file or removing it, not with the project"));
        }
        let same_cameras = project.cameras.iter().map(|c| &c.id).eq(self.project.cameras.iter().map(|c| &c.id));
        let rebuilt: Vec<String> = project
            .cameras
            .iter()
            .filter(|c| self.project.camera(&c.id).is_some_and(|was| self.project.rig_of(was) != project.rig_of(c)))
            .map(|c| c.id.clone())
            .collect();
        if !same_cameras {
            self.require_parked("adding, removing or reordering cameras")?;
        }
        if !rebuilt.is_empty() {
            self.require_parked("changing the rig")?;
        }
        if (!same_cameras || !rebuilt.is_empty()) && self.units.iter().any(|u| u.rig.preset_sync_progress().is_some()) {
            return Err(Fail::refused("Wait for the preset sync to finish before changing the rig"));
        }
        self.storage.save_project(&project).map_err(Fail::io)?;
        let rate_changed = project.rate != self.project.rate;
        let before = std::mem::replace(&mut self.project, project.clone());
        if !same_cameras || !rebuilt.is_empty() {
            let mut old: Vec<Unit> = std::mem::take(&mut self.units);
            for cam in &project.cameras {
                let kept = old.iter().position(|u| u.id == cam.id).filter(|_| !rebuilt.contains(&cam.id));
                self.units.push(match kept {
                    Some(at) => old.remove(at),
                    None => Unit::new(&project, cam),
                });
            }
            // The adapters that are left stop their devices as they are dropped.
            for mut gone in old {
                gone.rig.stop();
            }
            for id in &rebuilt {
                if let Some(cam) = project.camera(id) {
                    self.log(
                        "rig_changed",
                        json!({ "camera": id, "kind": cam.rig.kind, "fr7": cam.rig.fr7_host, "autopod": cam.rig.autopod_host }),
                    );
                }
            }
        }
        if !same_cameras {
            let added: Vec<&String> = project.cameras.iter().map(|c| &c.id).filter(|id| before.camera(id).is_none()).collect();
            let removed: Vec<&String> = before.cameras.iter().map(|c| &c.id).filter(|id| project.camera(id).is_none()).collect();
            let order: Vec<&String> = project.cameras.iter().map(|c| &c.id).collect();
            self.log("cameras_changed", json!({ "added": added, "removed": removed, "cameras": order }));
            if project.camera(&self.record_camera).is_none() {
                self.record_camera = project.first().id.clone();
            }
            // A preset is one camera's pose: it goes with its camera.
            let orphans: Vec<String> =
                self.presets.iter().filter(|p| project.camera(&p.camera).is_none()).map(|p| p.id.clone()).collect();
            if !orphans.is_empty() {
                self.presets.retain(|p| !orphans.contains(&p.id));
                for id in &orphans {
                    if let Some(path) = self.storage.preset_thumbnail_path(id) {
                        let _ = std::fs::remove_file(path);
                    }
                }
                self.storage.save_presets(&self.presets).map_err(Fail::io)?;
                self.log("presets_removed", json!({ "count": orphans.len(), "reason": "their camera left the project" }));
                self.push(WsMsg::PresetsChanged);
            }
        }
        for e in self.songs.values_mut() {
            e.compiled = compile(&e.song, &project);
        }
        if rate_changed {
            for m in &mut self.media {
                media::restate(m, project.rate);
            }
            self.push(WsMsg::MediaChanged);
        }
        if let Some(ext) = &mut self.external {
            ext.chase.set_freewheel(project.freewheel_frames);
        }
        if project.audio_device != before.audio_device {
            self.audio.set_device(self.audio_started_with.for_project(&project));
            self.log("audio_output", json!({ "device": project.audio_device }));
        }
        self.rebuild_mix();
        self.log("project_changed", json!({ "by": self.lease.as_ref().map(|l| l.name.clone()) }));
        self.push(WsMsg::ProjectChanged);
        self.poke();
        Ok(project)
    }

    /// The lease and the state a stage model may be changed in. Checked before a large upload is read.
    pub fn may_change_stage(&self, client: &str) -> Res<()> {
        self.require_lease(client)?;
        if self.transport.output != Output::Rehearse {
            return Err(Fail::refused("Disarm to change the stage model"));
        }
        Ok(())
    }

    /// Take a checked mesh file, already written beside its final place, as the stage model.
    pub fn set_stage_model(&mut self, client: &str, upload: StageUpload) -> Res<Project> {
        if let Err(fail) = self.may_change_stage(client) {
            let _ = std::fs::remove_file(&upload.file);
            return Err(fail);
        }
        let path = self.storage.stage_model_path();
        if let Err(e) = std::fs::rename(&upload.file, &path) {
            let _ = std::fs::remove_file(&upload.file);
            return Err(Fail::io(e));
        }
        let was = self.project.stage.model.as_ref();
        let revision = was.map(|m| m.revision).unwrap_or(0) + 1;
        let model = ptz_core::camera::StageModel {
            name: upload.name,
            revision,
            triangles: upload.triangles,
            min: upload.min,
            max: upload.max,
            unit: upload.unit,
            up: upload.up,
            offset: [0.0; 3],
            rotation_deg: 0.0,
        };
        self.log(
            "stage_model",
            json!({ "name": model.name, "triangles": model.triangles, "revision": revision, "by": self.lease.as_ref().map(|l| l.name.clone()) }),
        );
        // A venue replaces the built-in stage until the operator asks for both.
        self.project.stage = ptz_core::camera::Stage { show_builtin: false, model: Some(model) };
        self.storage.save_project(&self.project).map_err(Fail::io)?;
        self.push(WsMsg::ProjectChanged);
        Ok(self.project.clone())
    }

    pub fn delete_stage_model(&mut self, client: &str) -> Res<Project> {
        self.may_change_stage(client)?;
        let Some(model) = self.project.stage.model.take() else {
            return Err(Fail::not_found("There is no stage model to remove"));
        };
        self.project.stage.show_builtin = true;
        self.storage.save_project(&self.project).map_err(Fail::io)?;
        let _ = std::fs::remove_file(self.storage.stage_model_path());
        self.log("stage_model_removed", json!({ "name": model.name, "by": self.lease.as_ref().map(|l| l.name.clone()) }));
        self.push(WsMsg::ProjectChanged);
        Ok(self.project.clone())
    }

    pub fn stage_model_file(&self) -> Res<PathBuf> {
        match &self.project.stage.model {
            Some(_) => Ok(self.storage.stage_model_path()),
            None => Err(Fail::not_found("No stage model has been imported")),
        }
    }

    fn unique_preset_id(&self, name: &str) -> String {
        let base = storage::slug(name);
        if !self.presets.iter().any(|p| p.id == base) {
            return base;
        }
        (2..).map(|n| format!("{base}-{n}")).find(|id| !self.presets.iter().any(|p| &p.id == id)).unwrap()
    }

    pub fn store_preset(&mut self, client: &str, req: &StorePreset) -> Res<Preset> {
        self.require_lease(client)?;
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Fail::bad("Give the preset a name"));
        }
        let i = self.unit_index(req.camera.as_deref())?;
        let unit = &self.units[i];
        // With no pose given, hardware stores where the rig stands; nothing moves.
        let pose = req.pose.unwrap_or_else(|| if unit.rig.is_simulator() { self.monitor_pose(i).0 } else { unit.rig.pose() });
        let preset = Preset { camera: unit.id.clone(), ..Preset::new(&self.unique_preset_id(name), name, pose) };
        self.presets.push(preset.clone());
        self.storage.save_presets(&self.presets).map_err(Fail::io)?;
        self.push(WsMsg::PresetsChanged);
        Ok(preset)
    }

    /// Capture where a camera's rig stands, to program a move to it. A pose within the project's
    /// range of a preset the camera already has gives that preset; otherwise a new one is stored.
    /// Nothing moves. A rig that does not answer is refused: its last reading may be old.
    pub fn capture_preset(&mut self, client: &str, req: &CapturePreset) -> Res<Captured> {
        self.require_lease(client)?;
        let i = self.unit_index(req.camera.as_deref())?;
        let unit = &self.units[i];
        let camera = unit.id.clone();
        let pose = if unit.rig.is_simulator() {
            self.monitor_pose(i).0
        } else if let Some(why) = unit.rig.status().iter().find(|a| !a.connected).map(|a| a.detail.clone()) {
            return Err(Fail::refused(format!("The rig's pose is not known: {why}")));
        } else {
            unit.rig.pose()
        };
        if let Some(found) = self.project.preset_match.nearest(&self.presets, &camera, &pose) {
            let preset = found.clone();
            self.log("preset_captured", json!({ "camera": camera, "preset": preset.id, "reused": true, "pose": pose }));
            return Ok(Captured { preset, reused: true, pose });
        }
        let name = match req.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => name.to_string(),
            None => {
                let taken = |name: &str| self.presets.iter().any(|p| p.camera == camera && p.name == name);
                (1..).map(|n| format!("Preset {n}")).find(|name| !taken(name)).unwrap_or_default()
            }
        };
        let preset = Preset { camera: camera.clone(), ..Preset::new(&self.unique_preset_id(&name), &name, pose) };
        self.presets.push(preset.clone());
        self.storage.save_presets(&self.presets).map_err(Fail::io)?;
        self.push(WsMsg::PresetsChanged);
        self.log("preset_captured", json!({ "camera": camera, "preset": preset.id, "reused": false, "pose": pose }));
        Ok(Captured { preset, reused: false, pose })
    }

    /// Change a stored preset: to a given pose, to a given column height, or, with neither,
    /// to where its camera's rig stands now. Nothing moves. A move on the timeline keeps
    /// the pose it was made with until the operator updates it.
    pub fn update_preset(&mut self, client: &str, id: &str, req: &UpdatePreset) -> Res<Preset> {
        self.require_lease(client)?;
        let Some(at) = self.presets.iter().position(|p| p.id == id) else {
            return Err(Fail::not_found(format!("No preset with id {id}")));
        };
        let camera = self.presets[at].camera.clone();
        let mut pose = match (req.pose, req.elevation) {
            (Some(pose), _) => pose,
            (None, Some(_)) => self.presets[at].pose,
            (None, None) => {
                let i = self.unit_index(Some(&camera))?;
                let unit = &self.units[i];
                if unit.rig.is_simulator() {
                    self.monitor_pose(i).0
                } else if let Some(why) = unit.rig.status().iter().find(|a| !a.connected).map(|a| a.detail.clone()) {
                    return Err(Fail::refused(format!("The rig's pose is not known: {why}")));
                } else {
                    unit.rig.pose()
                }
            }
        };
        if let Some(mm) = req.elevation {
            pose.elevation = mm;
        }
        if Axis::ALL.iter().any(|a| !pose.get(*a).is_finite()) {
            return Err(Fail::bad("That pose is not a usable number"));
        }
        let by = self.who(client);
        let preset = &mut self.presets[at];
        let before = preset.pose;
        preset.pose = pose;
        let preset = preset.clone();
        self.storage.save_presets(&self.presets).map_err(Fail::io)?;
        self.log("preset_updated", json!({ "preset": id, "camera": camera, "by": by, "from": before, "to": pose }));
        self.push(WsMsg::PresetsChanged);
        Ok(preset)
    }

    pub fn delete_preset(&mut self, client: &str, id: &str) -> Res<()> {
        self.require_lease(client)?;
        let before = self.presets.len();
        self.presets.retain(|p| p.id != id);
        if self.presets.len() == before {
            return Err(Fail::not_found(format!("No preset with id {id}")));
        }
        self.storage.save_presets(&self.presets).map_err(Fail::io)?;
        self.push(WsMsg::PresetsChanged);
        Ok(())
    }

    /// Pull presets from one camera's rig. The simulator hands them over; a camera
    /// has to recall each one to learn its pose, which moves it, so that runs in
    /// the background and the presets arrive when it ends.
    pub fn sync_presets(&mut self, client: &str, camera: Option<&str>) -> Res<Vec<Preset>> {
        self.require_lease(client)?;
        let i = self.unit_index(camera)?;
        let id = self.units[i].id.clone();
        if self.units[i].rig.is_simulator() {
            let from_rig = self.units[i].rig.presets().map_err(Fail::refused)?;
            let found = from_rig.into_iter().map(|preset| DevicePreset { preset, thumbnail: None }).collect();
            self.merge_presets(&id, found).map_err(Fail::io)?;
            return Ok(self.presets.clone());
        }
        if self.transport.estop {
            return Err(Fail::refused("Reset E-Stop to continue"));
        }
        self.require_parked("reading presets from the camera, which moves it")?;
        self.units[i].rig.begin_preset_sync().map_err(Fail::refused)?;
        self.log("presets_sync_started", json!({ "camera": id, "by": self.who(client) }));
        self.poke();
        Ok(self.presets.clone())
    }

    /// A camera's device presets are matched to its own by their number on the device; the rest are added.
    fn merge_presets(&mut self, camera: &str, found: Vec<DevicePreset>) -> std::io::Result<()> {
        let count = found.len();
        for DevicePreset { preset: mut p, thumbnail } in found {
            p.camera = camera.to_string();
            let known = self
                .presets
                .iter_mut()
                .find(|e| e.camera == camera && e.camera_number.is_some() && e.camera_number == p.camera_number);
            let id = match known {
                Some(existing) => {
                    // The camera stores pan, tilt, zoom and focus. The column height is the
                    // show's own, so one that was set for this preset is kept.
                    existing.pose = Pose { elevation: existing.pose.elevation, ..p.pose };
                    existing.name = p.name;
                    existing.has_thumbnail = thumbnail.is_some();
                    existing.id.clone()
                }
                None => {
                    p.id = self.unique_preset_id(&p.name);
                    p.has_thumbnail = thumbnail.is_some();
                    self.presets.push(p.clone());
                    p.id
                }
            };
            if let (Some(jpeg), Some(path)) = (thumbnail, self.storage.preset_thumbnail_path(&id)) {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                storage::write_atomic(&path, &jpeg)?;
            }
        }
        self.storage.save_presets(&self.presets)?;
        self.log("presets_synced", json!({ "camera": camera, "read": count, "count": self.presets.len() }));
        self.push(WsMsg::PresetsChanged);
        Ok(())
    }

    pub fn preset_thumbnail(&self, id: &str) -> Res<PathBuf> {
        let known = self.presets.iter().any(|p| p.id == id && p.has_thumbnail);
        self.storage
            .preset_thumbnail_path(id)
            .filter(|p| known && p.is_file())
            .ok_or_else(|| Fail::not_found("That preset has no picture"))
    }

    // ---- media and audio ----

    pub fn media_info(&self, id: &str) -> Res<&MediaInfo> {
        self.media.iter().find(|m| m.id == id).ok_or_else(|| Fail::not_found(format!("No media with id {id}")))
    }

    /// First half of an import: pick an id and a folder. The decode happens outside the show lock.
    pub fn reserve_media(&mut self, client: &str, name: &str) -> Res<MediaTicket> {
        self.require_lease(client)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(Fail::bad("The upload needs a file name: /media?name=<file name>"));
        }
        let ext = media::extension(name);
        if !media::AUDIO_EXTENSIONS.contains(&ext.as_str()) && !media::VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            return Err(Fail::invalid(format!(
                "{name} was not imported: only WAV, AIFF and MP3 audio and QuickTime or MP4 video can be used"
            )));
        }
        let stem = std::path::Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
        let base = storage::slug(stem);
        let taken = |id: &str| self.media.iter().any(|m| m.id == id) || self.media_reserved.contains(id);
        let id = if taken(&base) { (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).unwrap() } else { base };
        let dir = self.storage.media_dir(&id).ok_or_else(|| Fail::bad("That file name cannot be used"))?;
        self.media_reserved.insert(id.clone());
        Ok(MediaTicket {
            id,
            name: name.to_string(),
            dir,
            incoming: self.storage.media_root(),
            rate: self.rate(),
            device_rate: self.audio.rate(),
        })
    }

    pub fn cancel_media(&mut self, id: &str) {
        self.media_reserved.remove(id);
    }

    /// Second half of an import: the file decoded, add it to the library.
    pub fn register_media(&mut self, ingested: Ingested, playable: Option<Vec<f32>>) -> MediaInfo {
        let mut info = ingested.info;
        self.media_reserved.remove(&info.id);
        media::restate(&mut info, self.rate());
        if let Some(mono) = playable {
            self.audio_cache.insert(info.id.clone(), Arc::new(mono));
        }
        self.media.retain(|m| m.id != info.id);
        self.media.push(info.clone());
        self.media.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        self.log(
            "media_imported",
            json!({
                "media": info.id, "name": info.name, "seconds": info.duration_s, "channels": info.channels,
                "rate": info.sample_rate, "start_tc": info.start_tc, "start_tc_source": info.start_tc_source,
                "proxy_frames": info.proxy_frames,
            }),
        );
        self.push(WsMsg::MediaChanged);
        self.rebuild_mix();
        info
    }

    pub fn delete_media(&mut self, client: &str, id: &str) -> Res<()> {
        self.require_lease(client)?;
        self.media_info(id)?;
        if let Some(e) = self.songs.values().find(|e| e.song.media.iter().any(|c| c.media_id == id)) {
            return Err(Fail::refused(format!("{} uses this file. Remove it from the timeline first", e.song.name)));
        }
        if let Some(dir) = self.storage.media_dir(id) {
            if dir.exists() {
                std::fs::remove_dir_all(dir).map_err(Fail::io)?;
            }
        }
        self.media.retain(|m| m.id != id);
        self.audio_cache.remove(id);
        self.audio_failed.remove(id);
        self.log("media_deleted", json!({ "media": id }));
        self.push(WsMsg::MediaChanged);
        Ok(())
    }

    pub fn media_file(&self, id: &str, file: &str) -> Res<PathBuf> {
        self.media_info(id)?;
        Ok(self.storage.media_dir(id).ok_or_else(|| Fail::not_found("No such media"))?.join(file))
    }

    /// Put a start timecode on a file, or take a typed or decoded one off again.
    pub fn stamp_media(&mut self, client: &str, id: &str, stamp: Stamp) -> Res<MediaInfo> {
        self.require_lease(client)?;
        let path = self.media_file(id, "info.json")?;
        let Some(info) = self.media.iter_mut().find(|m| m.id == id) else {
            return Err(Fail::not_found(format!("No media with id {id}")));
        };
        let had_channel = info.ltc_channel;
        match stamp {
            Stamp::FileOwn { start_tc, source } => {
                info.start_tc = start_tc;
                info.start_tc_source = source;
                info.ltc_channel = None;
            }
            Stamp::Typed(tc) => {
                info.start_tc = Some(tc.to_string());
                info.start_tc_source = Some(media::STAMP_TYPED.into());
            }
            Stamp::Ltc { channel, start } => {
                info.start_tc = Some(start.to_string());
                info.start_tc_source = Some(format!("ltc channel {}", channel + 1));
                info.ltc_channel = Some(channel);
            }
        }
        let out = info.clone();
        storage::write_json(&path, &out).map_err(Fail::io)?;
        if out.ltc_channel != had_channel {
            // The timecode channel is not programme sound.
            self.reload_audio(id);
        }
        self.log("media_stamped", json!({ "media": id, "start_tc": out.start_tc, "source": out.start_tc_source }));
        self.push(WsMsg::MediaChanged);
        self.check_file_lock();
        self.poke();
        Ok(out)
    }

    /// Load a media file's audio for playback on a worker thread; `tick` collects it.
    fn load_audio(&mut self, id: &str) {
        if self.audio_loading.contains(id) || self.audio_failed.contains(id) {
            return;
        }
        let Some(dir) = self.storage.media_dir(id) else { return };
        let exclude = self.timecode_channel(id);
        let (tx, id, device_rate) = (self.loaded_tx.clone(), id.to_string(), self.audio.rate());
        self.audio_loading.insert(id.clone());
        let spawned = std::thread::Builder::new().name("audio-load".into()).spawn(move || {
            let result = media::read_audio(&dir.join("audio.f32"))
                .and_then(|d| crate::audio::resample(&d.mono(exclude), d.rate, device_rate));
            let _ = tx.send((id, exclude, result));
        });
        if spawned.is_err() {
            self.audio_loading.clear();
        }
    }

    /// The channel of a file that is read for its timecode, and so is never played.
    fn timecode_channel(&self, id: &str) -> Option<u32> {
        self.media.iter().find(|m| m.id == id).and_then(|m| m.ltc_channel)
    }

    fn collect_loaded_audio(&mut self) {
        let mut changed = false;
        while let Ok((id, excluded, result)) = self.loaded_rx.try_recv() {
            self.audio_loading.remove(&id);
            changed = true;
            // The timecode channel was chosen while the file was being read: what was read still has
            // the timecode in it, and is read again without.
            if excluded != self.timecode_channel(&id) {
                continue;
            }
            match result {
                Ok(mono) => {
                    self.audio_cache.insert(id, Arc::new(mono));
                }
                Err(e) => {
                    self.audio_failed.insert(id.clone());
                    self.log("media_error", json!({ "media": id, "error": e }));
                }
            }
        }
        if changed {
            self.rebuild_mix();
        }
    }

    /// The media changed (for example its timecode channel): load its audio again.
    pub fn reload_audio(&mut self, id: &str) {
        self.audio_cache.remove(id);
        self.audio_failed.remove(id);
        self.rebuild_mix();
    }

    /// Hand the audio thread what the loaded song plays.
    fn rebuild_mix(&mut self) {
        let rate = self.rate();
        let device_rate = self.audio.rate();
        let mut clips = Vec::new();
        let mut missing = Vec::new();
        let mut ltc = None;
        // Following timecode from outside, the sound is played where the timecode comes from: the
        // files here stay silent, or the room would hear the song twice.
        let outside = self.external.is_some();
        if let Some(e) = self.entry() {
            for c in e.song.media.iter().filter(|c| !c.muted && !outside) {
                let Some(data) = self.audio_cache.get(&c.media_id) else {
                    missing.push(c.media_id.clone());
                    continue;
                };
                let offset = frame_to_sample(c.in_frame, device_rate, rate);
                let len = (frame_to_sample(c.out_frame, device_rate, rate) - offset).min(data.len() as i64 - offset);
                if len > 0 {
                    clips.push(PlayClip {
                        start: frame_to_sample(c.start_frame, device_rate, rate),
                        offset,
                        len,
                        data: data.clone(),
                    });
                }
            }
            let start_code = Timecode::parse_at(&e.song.start_tc, rate).map(|tc| tc.to_frames(rate)).unwrap_or(0);
            ltc = Some(LtcOut { rate, start_code, level: self.project.ltc_level });
        }
        self.audio.set_mix(Mix { clips, routing: self.routing(), ltc });
        for id in missing {
            self.load_audio(&id);
        }
    }

    /// Where Program and the generated timecode go. A file that gives up a channel to timecode is
    /// most often a stereo file with the sound on one side only: while the song plays such a file,
    /// Program goes to both outputs and not to one alone.
    fn routing(&self) -> Routing {
        let plays_one =
            self.entry().is_some_and(|e| e.song.media.iter().any(|c| !c.muted && self.timecode_channel(&c.media_id).is_some()));
        if plays_one {
            self.project.routing.mirrored()
        } else {
            self.project.routing
        }
    }

    fn audio_status(&self) -> AudioStatus {
        let info = self.audio.info();
        let (peak_left, peak_right) = self.audio.peaks();
        AudioStatus {
            output: info.name,
            silent_reason: info.silent_reason,
            device_rate: self.audio.rate(),
            routing: self.routing(),
            callbacks: self.audio.callbacks(),
            position_frame: if self.audio.is_playing() { self.audio.position(self.rate()) } else { self.frame_f },
            drift_frames: self.audio_drift,
            peak_left,
            peak_right,
        }
    }

    // ---- transport ----

    fn context(&self, client: &str) -> Context {
        let entry = self.entry();
        Context {
            has_lease: self.holds_lease(client),
            song_loaded: entry.is_some(),
            committed_clean: entry.is_some_and(|e| e.committed_clean()),
            blocking_issues: entry.map(|e| e.compiled.blocking_count(&e.song)).unwrap_or(0),
            adapter_problem: (0..self.units.len())
                .find_map(|i| self.rig_problem(i).map(|why| format!("{}{why}", self.prefix(i)))),
        }
    }

    fn who(&self, client: &str) -> String {
        match &self.lease {
            _ if client == STOPPING => "the server, stopping".to_string(),
            Some(l) if l.client == client => l.name.clone(),
            _ => format!("watcher {client}"),
        }
    }

    pub fn act(&mut self, client: &str, action: Action, req: &TransportRequest) -> Res<()> {
        self.act_with(client, action, req, None)
    }

    /// Lock to a source the caller supplies instead of one opened from the
    /// project's inputs. The tests script timecode through this.
    pub fn lock_with(&mut self, client: &str, kind: Kind, source: Box<dyn CodeSource>) -> Res<()> {
        let req = TransportRequest { source: Some(kind.id().to_string()), ..Default::default() };
        self.act_with(client, Action::Lock, &req, Some(source))
    }

    fn act_with(
        &mut self,
        client: &str,
        action: Action,
        req: &TransportRequest,
        supplied: Option<Box<dyn CodeSource>>,
    ) -> Res<()> {
        self.clock.reset();
        let ctx = self.context(client);
        if let Err(blocked) = transport::check(&self.transport, &ctx, action) {
            self.log("refused", json!({ "action": action.key(), "by": self.who(client), "reason": blocked.text }));
            return Err(Fail::refused(blocked.text));
        }
        if action == Action::Arm && self.units.iter().any(|u| u.rig.preset_sync_progress().is_some()) {
            return Err(Fail::refused("Wait for the preset sync to finish before arming"));
        }
        let rate = self.rate();
        let length = self.length() as f64;
        let before = self.transport;
        match action {
            Action::Play => {
                match before.time {
                    TimeMode::FreeRun => {
                        if self.frame_f >= length - 0.5 {
                            self.frame_f = 0.0;
                        }
                        self.audio.play(self.frame_f, rate);
                    }
                    TimeMode::LockedFile => {
                        // The server rolls the file itself, from the playhead when that is inside the file.
                        let (first, end) = self.locked_file_span().map_err(Fail::refused)?;
                        if self.frame_f < first as f64 || self.frame_f >= end as f64 - 0.5 {
                            self.frame_f = first as f64;
                        }
                        self.audio.play(self.frame_f, rate);
                    }
                    // Code decides when the playhead and the sound start.
                    TimeMode::LockedExternal => {}
                }
                // A rig placed by hand goes back to the timeline, unless Record is on: then the
                // pass starts from where it was put, if it is moved again.
                if !before.record {
                    self.clear_manual();
                }
            }
            // While armed the rig keeps following the timeline, which now stands still.
            Action::Stop => self.audio.stop(),
            Action::Cue => {
                self.audio.stop();
                self.audio.seek(0.0, rate);
                self.frame_f = 0.0;
                self.clear_manual();
            }
            Action::Seek => {
                let Some(frame) = req.frame else {
                    return Err(Fail::bad("Seek needs a frame"));
                };
                self.frame_f = (frame as f64).clamp(0.0, length);
                self.clear_manual();
                self.audio.seek(self.frame_f, rate);
            }
            Action::Lock => {
                let source = req.source.clone().unwrap_or_default();
                if let Some(clip) = source.strip_prefix("file:") {
                    self.file_span(clip).map_err(Fail::refused)?;
                } else if let Some(kind) = Kind::parse(&source) {
                    let input: Box<dyn CodeSource> = match supplied {
                        Some(given) => given,
                        None => match lock::open(kind, &self.project.inputs) {
                            Ok(feed) => Box::new(feed),
                            Err(why) => {
                                self.lock_problem = Some(why.clone());
                                self.poke();
                                return Err(Fail::refused(why));
                            }
                        },
                    };
                    let chase = Chase::new(rate, self.project.freewheel_frames, input.interval_frames());
                    self.external =
                        Some(External { kind, source: input, chase, mismatched: 0, logged: ChaseState::NoCode, problem: None });
                } else {
                    return Err(Fail::bad("Lock needs a source: ltc, mtc, artnet or file:<clip id>"));
                }
                self.lock_source = Some(source);
                self.rebuild_mix();
            }
            Action::Unlock => {
                self.lock_source = None;
                self.external = None;
                self.rebuild_mix();
                // Playing carries on in free-run from where it is.
                if self.transport.run == Run::Playing && !self.audio.is_playing() {
                    self.audio.play(self.frame_f, rate);
                }
            }
            Action::Arm => {
                for u in &mut self.units {
                    u.live = Live { commanded: Some(u.rig.pose()), ..Live::default() };
                    u.held = false;
                }
                self.log_arm(client);
            }
            Action::Disarm => {
                for u in &mut self.units {
                    u.rig.stop();
                    u.live.clear();
                    u.held = false;
                }
            }
            Action::RecordOn => {
                if let Some(camera) = &req.camera {
                    let i = self.unit_index(Some(camera))?;
                    self.record_camera = self.units[i].id.clone();
                }
                let axes = match &req.axes {
                    Some(given) => Axis::ALL.into_iter().filter(|a| given.contains(a)).collect(),
                    None => self.record_axes.clone(),
                };
                if axes.is_empty() {
                    return Err(Fail::refused("Record-enable at least one axis lane first"));
                }
                self.record_axes = axes;
            }
            Action::RecordOff => {}
            Action::Estop => {
                self.audio.stop();
                let now = Instant::now();
                for u in &mut self.units {
                    u.rig.stop();
                    if u.rig.is_simulator() {
                        let _ = u.rig.jog(Jog::default());
                    }
                    u.jog_deadline = None;
                    u.live.clear();
                    u.held = false;
                    u.halting = Some(now);
                }
            }
            Action::ResetEstop => {}
        }
        let mut after = transport::apply(&before, action, self.external.is_some());
        if action == Action::Play && after.time == TimeMode::LockedFile {
            // A file's code is in the song the moment the server starts rolling it.
            after = transport::on_event(&after, Event::CodeEntered);
        }
        self.transport = after;
        self.lock_problem = None;
        if matches!(action, Action::Arm | Action::Play | Action::ResetEstop) {
            // A camera that is held keeps saying why until the show is armed again.
            for u in self.units.iter_mut().filter(|u| !u.held) {
                u.live.fault = None;
            }
        }
        let frame = self.frame_f.floor() as i64;
        self.log("transport", json!({ "action": action.key(), "by": self.who(client), "frame": frame, "state": self.transport }));
        self.sync_capture();
        // A virtual rig follows a moved playhead at once, not a tick later.
        if self.transport.output == Output::Rehearse && self.captures.is_empty() {
            for i in 0..self.units.len() {
                if self.units[i].rig.is_simulator() && !self.units[i].manual {
                    let pose = self.timeline_pose(&self.units[i].id);
                    self.units[i].rig.sync_virtual(&pose, &Axis::ALL);
                }
            }
        }
        self.poke();
        Ok(())
    }

    fn event(&mut self, event: Event) {
        let before = self.transport;
        let after = transport::on_event(&before, event);
        if after == before {
            return;
        }
        match event {
            Event::SongEnded => self.audio.stop(),
            Event::Fault => {
                self.audio.stop();
                for u in &mut self.units {
                    u.rig.stop();
                    u.live.clear();
                    u.held = false;
                }
            }
            _ => {}
        }
        self.transport = after;
        self.log("event", json!({ "event": format!("{event:?}"), "frame": self.frame_f.floor() as i64, "state": after }));
        self.sync_capture();
        self.poke();
    }

    // ---- live ----

    /// What is being armed: the song, its committed revision and every overrun someone accepted.
    fn log_arm(&mut self, client: &str) {
        let Some(e) = self.entry() else { return };
        let number = e.revision();
        let sha = number.and_then(|n| self.storage.revisions(&e.song.id).into_iter().find(|r| r.number == n)).map(|r| r.sha256);
        let accepted: Vec<serde_json::Value> = e
            .song
            .motion
            .iter()
            .filter_map(|c| c.accepted.as_ref().map(|a| json!({ "clip": c.id, "by": a.by, "at": a.at })))
            .collect();
        let data = json!({
            "song": e.song.id,
            "revision": number,
            "sha256": sha,
            "accepted": accepted,
            "by": self.who(client),
            "cameras": self
                .units
                .iter()
                .map(|u| {
                    json!({
                        "camera": u.id,
                        "rig": u.rig.status().iter().map(|a| a.name.clone()).collect::<Vec<_>>(),
                        "from": u.rig.pose(),
                    })
                })
                .collect::<Vec<_>>(),
        });
        self.log("arm", data);
    }

    /// A camera's rig cannot go on: say why and hold it. Whether the others carry
    /// on is the project's choice; when none is left, output is cut.
    fn fault(&mut self, i: usize, why: String) {
        let unit = &self.units[i];
        let data = json!({
            "camera": unit.id,
            "reason": why,
            "frame": self.frame_f.floor() as i64,
            "pose": unit.rig.pose(),
            "error": unit.live.error,
        });
        self.log("fault", data);
        let unit = &mut self.units[i];
        unit.live.fault = Some(why);
        if self.project.on_fault != OnFault::HoldAll && self.transport.hardware_live() {
            unit.rig.stop();
            unit.live.clear();
            unit.held = true;
            if self.units.iter().any(|u| !u.held) {
                let (camera, running) = (self.units[i].id.clone(), self.units.iter().filter(|u| !u.held).count());
                self.log("camera_held", json!({ "camera": camera, "still_running": running }));
                self.poke();
                return;
            }
        }
        self.event(Event::Fault);
    }

    /// The plain preset move of a camera under the playhead, when its rig is set to let the head run those itself.
    fn ride(&self, i: usize) -> Option<Ride> {
        let camera = &self.project.cameras[i];
        if !camera.rig.fr7_absolute_moves || self.transport.run != Run::Playing || self.transport.output != Output::Armed {
            return None;
        }
        let e = self.entry()?;
        let f = self.frame_f;
        let clip = e.song.motion.iter().find(|c| {
            c.camera == camera.id
                && matches!(c.body, ClipBody::PresetMove { .. })
                && (c.has_axis(Axis::Pan) || c.has_axis(Axis::Tilt))
                && f >= c.start_frame as f64
                && f < (c.start_frame + c.duration_frames) as f64
        })?;
        let end = clip.start_frame + clip.duration_frames;
        let pose = e.compiled.poses(&camera.id).get(end.max(0) as usize)?;
        Some(Ride { pan: pose.pan, tilt: pose.tilt, seconds: self.rate().frames_to_seconds(end as f64 - f) })
    }

    /// One turn of the live loop for every camera that is not held.
    fn tick_cameras(&mut self, now: Instant, dt: f64) {
        let positioning = self.transport.output == Output::Positioning;
        let mut waiting = 0;
        let mut arrived = 0;
        for i in 0..self.units.len() {
            // A fault that cut output ends the turn for the cameras after it too.
            if !self.transport.hardware_live() {
                return;
            }
            if self.units[i].held {
                continue;
            }
            let target = self.timeline_pose(&self.units[i].id);
            match self.tick_live(i, now, dt, target) {
                Step::Arrived => arrived += 1,
                Step::Travelling => waiting += 1,
                Step::Tracking | Step::Faulted => {}
            }
        }
        // Positioning ends when every camera that is still in the show stands on its pose.
        if positioning && self.transport.output == Output::Positioning && waiting == 0 && arrived > 0 {
            self.event(Event::RigArrived);
        }
    }

    /// Whether camera `i` stands on a column the song has no elevation for. Such a column is
    /// not the song's to move: it is left at the height it stands at.
    fn column_stays(&self, i: usize) -> bool {
        let camera = &self.project.cameras[i];
        match self.entry() {
            Some(e) => ptz_core::compile::column_stays(&e.song, camera),
            None => camera.has_column(),
        }
    }

    /// Why camera `i`'s rig cannot go live. A column that is left where it stands is not asked.
    fn rig_problem(&self, i: usize) -> Option<String> {
        if self.column_stays(i) {
            self.units[i].rig.head_problem()
        } else {
            self.units[i].rig.problem()
        }
    }

    /// One turn of the live loop for one camera: check the target, send it, compare the rig with it.
    fn tick_live(&mut self, i: usize, now: Instant, dt: f64, target: Pose) -> Step {
        if let Some(problem) = self.rig_problem(i) {
            self.fault(i, problem);
            return Step::Faulted;
        }
        // A column the song has no elevation for is at its target wherever it stands.
        let column = !self.column_stays(i);
        let target = if column { target } else { Pose { elevation: self.units[i].rig.pose().elevation, ..target } };
        let positioning = self.transport.output == Output::Positioning;
        let stopped = self.transport.run == Run::Stopped;
        // The project can say that a rig off the timeline is driven on instead of held.
        let carry_on = self.project.on_fault == OnFault::CarryOn;
        let camera = &self.project.cameras[i];
        let scale = if positioning { camera.rig.positioning_speed.clamp(0.01, 1.0) } else { 1.0 };
        let (speeds, limits) = (camera.axes, self.project.limits_of(camera));
        let (tol, window) = (camera.rig.tolerance, Duration::from_millis(camera.rig.tracking_window_ms));
        let before = self.units[i].live.commanded.unwrap_or_else(|| self.units[i].rig.pose());
        // The rig is told to be where the timeline is, no faster than each axis can go. A move
        // someone accepted as too fast therefore lags by design instead of tripping a fault.
        let next = Pose::from_fn(|a| {
            if a == Axis::Elevation && !column {
                return target.elevation;
            }
            let step = speeds.get(a).max_speed * scale * dt;
            before.get(a) + (target.get(a) - before.get(a)).clamp(-step, step)
        });
        // Nothing is sent to a column that stays where it stands, so its height is not held to
        // the limits: it may rest a little past the lowest or highest height that was measured.
        let asked = if column {
            next
        } else {
            Pose { elevation: next.elevation.clamp(limits.elevation.min, limits.elevation.max), ..next }
        };
        if let Some(why) = limits.out_of_range(&asked) {
            self.fault(i, format!("Limit reached: {why}. The rig is held"));
            return Step::Faulted;
        }
        if let Some(rule) = camera.keep_out.iter().find(|r| r.violated(&next)) {
            let why = format!("Keep-out rule broken: {}. The rig is held", rule.describe_in(self.project.units));
            self.fault(i, why);
            return Step::Faulted;
        }
        let velocity = if dt > 1e-6 { Pose::from_fn(|a| (next.get(a) - before.get(a)) / dt) } else { Pose::default() };
        let ride = self.ride(i);
        let unit = &mut self.units[i];
        unit.rig.drive(&Drive { target: next, velocity, scale, ride, column });
        unit.live.commanded = Some(next);
        unit.live.history.push_back((now, next));
        while unit.live.history.front().is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(1)) {
            unit.live.history.pop_front();
        }

        let reading = unit.rig.reading();
        // Each axis against what was asked of it at the moment it was read.
        let error = Pose::from_fn(|a| {
            let asked = unit.live.commanded_at(reading.at[a.index()]).unwrap_or(next);
            reading.pose.get(a) - asked.get(a)
        });
        unit.live.error = error;
        if positioning {
            unit.live.over_since = None;
            let there = Axis::ALL.iter().all(|a| (reading.pose.get(*a) - target.get(*a)).abs() <= tol.get(*a) * 0.25);
            if there {
                unit.live.positioning = None;
                unit.live.late = false;
                return Step::Arrived;
            }
            let moved = unit.live.positioning.is_none_or(|(to, _)| to != target && stopped);
            if moved {
                let travel = Axis::ALL
                    .iter()
                    .map(|a| (target.get(*a) - reading.pose.get(*a)).abs() / (speeds.get(*a).max_speed * scale).max(1e-9))
                    .fold(0.0, f64::max);
                unit.live.positioning = Some((target, now + Duration::from_secs_f64(travel * 1.5) + POSITIONING_GRACE));
                unit.live.late = false;
            } else if unit.live.positioning.is_some_and(|(_, deadline)| now > deadline) {
                if carry_on {
                    // It had its time. It counts as there, is driven on toward the pose, and the log says how far off it was.
                    if !unit.live.late {
                        unit.live.late = true;
                        let data = json!({ "camera": unit.id, "frame": self.frame_f.floor() as i64, "pose": reading.pose, "error": error });
                        let record = self.log.append("positioning_late", data);
                        self.push(WsMsg::Log { record });
                    }
                    return Step::Arrived;
                }
                self.fault(i, "The rig did not reach the pose at the playhead. Output is cut".into());
                return Step::Faulted;
            }
            return Step::Travelling;
        }
        unit.live.positioning = None;
        // While the head runs a move by its own ramp, pan and tilt are not expected to match the curve.
        let checked = |a: &Axis| ride.is_none() || !matches!(a, Axis::Pan | Axis::Tilt);
        let worst = Axis::ALL
            .into_iter()
            .filter(checked)
            .map(|a| (a, error.get(a).abs() / tol.get(a).max(1e-9)))
            .filter(|(_, share)| *share > 1.0 && !carry_on)
            .max_by(|a, b| a.1.total_cmp(&b.1));
        match worst {
            Some((axis, _)) => {
                let since = *unit.live.over_since.get_or_insert(now);
                if now.duration_since(since) > window {
                    self.fault(
                        i,
                        format!(
                            "Tracking fault: {} is {} from the timeline. The rig is held",
                            axis.label().to_lowercase(),
                            axis.format_in(error.get(axis).abs(), self.project.units)
                        ),
                    );
                    return Step::Faulted;
                }
            }
            None => unit.live.over_since = None,
        }
        if unit.live.pose_logged.is_none_or(|at| now.duration_since(at) >= Duration::from_secs(1)) {
            unit.live.pose_logged = Some(now);
            let data = json!({ "camera": unit.id, "frame": self.frame_f.floor() as i64, "pose": reading.pose, "error": error });
            let record = self.log.append("pose", data);
            self.push(WsMsg::Log { record });
        }
        Step::Tracking
    }

    /// Things a rig does on its own time: halting after E-Stop, connecting, finishing a preset sync.
    fn tick_rig(&mut self, now: Instant) {
        let poll = self.ticks.is_multiple_of(15);
        for i in 0..self.units.len() {
            let id = self.units[i].id.clone();
            if let Some(since) = self.units[i].halting {
                let pose = self.units[i].rig.pose();
                if !self.units[i].rig.is_moving() {
                    self.units[i].halting = None;
                    let ms = now.duration_since(since).as_millis() as u64;
                    self.log("estop_halted", json!({ "camera": id, "after_ms": ms, "pose": pose }));
                    self.poke();
                } else if now.duration_since(since) > HALT_AGAIN {
                    self.units[i].rig.stop();
                    self.units[i].halting = Some(now);
                    self.log("estop_still_moving", json!({ "camera": id, "pose": pose }));
                }
            }
            if poll {
                let seen: Vec<(String, bool, String)> =
                    self.units[i].rig.status().into_iter().map(|a| (a.name, a.connected, a.detail)).collect();
                for (name, connected, detail) in &seen {
                    let was = self.units[i].adapters_were.iter().find(|(n, _)| n == name).map(|(_, c)| *c);
                    if was != Some(*connected) {
                        self.log("adapter", json!({ "camera": id, "name": name, "connected": connected, "detail": detail }));
                        self.poke();
                    }
                }
                self.units[i].adapters_were = seen.into_iter().map(|(name, connected, _)| (name, connected)).collect();
            }
            if let Some(result) = self.units[i].rig.take_preset_sync() {
                match result {
                    Ok(found) => {
                        if let Err(e) = self.merge_presets(&id, found) {
                            self.log("presets_sync_failed", json!({ "camera": id, "reason": e.to_string() }));
                        }
                    }
                    Err(why) => {
                        self.units[i].live.fault = Some(format!("Preset sync: {why}"));
                        self.log("presets_sync_failed", json!({ "camera": id, "reason": why }));
                    }
                }
                self.poke();
            }
        }
    }

    // ---- lock ----

    fn locked_clip(&self) -> Option<&str> {
        self.lock_source.as_deref().and_then(|s| s.strip_prefix("file:"))
    }

    /// The stretch of the song a file covers, as `(first frame, end frame)`, when
    /// the playhead can be locked to it. A stamped file has to sit on its own code,
    /// so that the code it plays is the code the timeline shows.
    fn file_span(&self, clip_id: &str) -> Result<(i64, i64), String> {
        let Some(e) = self.entry() else {
            return Err("Create or load a song first".into());
        };
        let Some(clip) = e.song.media_clip(clip_id) else {
            return Err("That file is not on this song's timeline".into());
        };
        let rate = self.rate();
        let info = self.media.iter().find(|m| m.id == clip.media_id);
        if let Some((info, on_code)) = info.and_then(|m| Some((m, m.frame_in(&e.song, rate)?))) {
            let sits = clip.start_frame - clip.in_frame;
            if sits != on_code {
                return Err(format!(
                    "{} starts at {} but sits at {} on the timeline. Move it to its timecode to lock to it",
                    clip.name,
                    info.start_tc.as_deref().unwrap_or_default(),
                    e.song.clock(rate).timecode_at(sits)
                ));
            }
        }
        Ok((clip.start_frame, clip.end_frame().min(e.song.length_frames)))
    }

    fn locked_file_span(&self) -> Result<(i64, i64), String> {
        match self.locked_clip() {
            Some(clip) => self.file_span(clip),
            None => Err("The lock names no file".into()),
        }
    }

    /// An edit can take the locked file away or move it off its code.
    fn check_file_lock(&mut self) {
        if self.transport.time != TimeMode::LockedFile {
            return;
        }
        if let Err(why) = self.locked_file_span() {
            self.release_lock(format!("Unlocked. {why}"));
        }
    }

    /// Let go of a lock the operator did not release: back to free-run, stopped, with the reason kept.
    fn release_lock(&mut self, why: String) {
        let source = self.lock_source.take();
        self.external = None;
        self.rebuild_mix();
        let mut after = transport::apply(&self.transport, Action::Unlock, false);
        if after.run != Run::Stopped {
            after = transport::apply(&after, Action::Stop, false);
        }
        self.audio.stop();
        self.transport = after;
        self.log("lock_released", json!({ "source": source, "reason": why, "state": after }));
        self.lock_problem = Some(why);
        self.sync_capture();
        self.poke();
    }

    /// Follow LTC, MTC or Art-Net: feed the chase, then move the playhead and the sound with it.
    fn tick_external(&mut self, now: Instant) {
        let rate = self.rate();
        let started = self.started;
        let t = now.duration_since(started).as_secs_f64();
        let Some(ext) = &mut self.external else { return };
        for s in ext.source.poll() {
            ext.mismatched = if s.reading.rate.matches(rate) { 0 } else { ext.mismatched + 1 };
            ext.chase.on_reading(s.reading, s.at.saturating_duration_since(started).as_secs_f64());
        }
        ext.chase.tick(t);
        if ext.mismatched >= MISMATCH_READINGS {
            let carried = ext.chase.announced().map(CodeRate::label).unwrap_or("another");
            let why = format!(
                "Lock refused: the {} carries {carried} fps code and the project is {} fps",
                ext.kind.label(),
                rate.label()
            );
            self.release_lock(why);
            return;
        }
        let jumped = ext.chase.take_jump();
        let code = ext.chase.code_at(t);
        let state = ext.chase.state();
        if state != ext.logged {
            ext.logged = state;
            let data = json!({ "source": ext.kind.id(), "state": state, "jumped": jumped });
            self.log("chase", data);
            self.poke();
        }
        let Some(ext) = &mut self.external else { return };
        // An input that has failed says so until it is back; the run log hears of each change once.
        let problem = ext.source.problem();
        let was = std::mem::replace(&mut ext.problem, problem.clone());
        if problem != was {
            self.log("lock_input", json!({ "source": self.lock_source, "problem": problem, "recovered": problem.is_none() }));
            if problem.is_none() && self.lock_problem == was {
                self.lock_problem = None;
            }
            self.poke();
        }
        if problem.is_some() && self.lock_problem != problem {
            self.lock_problem = problem;
        }
        if jumped {
            self.event(Event::Relocated);
        }
        let Some(e) = self.entry() else { return };
        let (clock, length) = (e.song.clock(rate), e.song.length_frames as f64);
        // Code inside the song, as a playhead position.
        let at = code.map(|c| clock.offset_of(c)).filter(|f| (0.0..length).contains(f));
        if let (None, Some(code), true) = (at, code, self.project.follow_code && state == ChaseState::Locked) {
            if self.follow_code(code) {
                return;
            }
        }
        match (self.transport.run, at) {
            (Run::WaitingForCode, Some(frame)) => {
                self.frame_f = frame;
                self.clock.reset();
                self.audio.play(frame, rate);
                self.audio_drift = 0.0;
                self.event(Event::CodeEntered);
            }
            (Run::Playing, Some(frame)) => {
                self.frame_f = self.clock.follow(frame, now, rate.fps());
                // The sound follows code the server does not control: more than a frame out, it jumps back on.
                let drift = self.audio.position(rate) - frame;
                if !self.audio.is_playing() || drift.abs() > 1.0 {
                    self.audio.play(frame, rate);
                    self.log("audio_reseek", json!({ "frame": frame, "drift_frames": drift }));
                    self.audio_drift = 0.0;
                } else {
                    self.audio_drift = drift;
                }
            }
            (Run::Playing, None) => {
                // Code stopped or left the song: hold here with Play still pressed.
                self.audio.stop();
                self.audio_drift = 0.0;
                self.event(Event::CodeLost);
            }
            _ => self.audio_drift = 0.0,
        }
    }

    fn lock_status(&self) -> LockStatus {
        let rate = self.rate();
        let problem = self.lock_problem.clone();
        if let Some(ext) = &self.external {
            let t = self.started.elapsed().as_secs_f64();
            let last = ext.chase.last_reading();
            return LockStatus {
                source: self.lock_source.clone(),
                chase: ext.chase.state(),
                last_code: last.map(|(frames, _)| {
                    let counting = ext.chase.announced().map(CodeRate::counting_rate).unwrap_or(rate);
                    Timecode::from_frames(frames.floor() as i64, counting).to_string()
                }),
                frames_since_code: last.map(|_| ext.chase.silent_frames(t)),
                announced_rate: ext.chase.announced().map(|r| r.label().to_string()),
                problem,
            };
        }
        if self.transport.time == TimeMode::LockedFile {
            // The file is its own code: always there, and exact.
            let code = self.entry().map(|e| e.song.clock(rate).timecode_at(self.frame_f.floor() as i64).to_string());
            return LockStatus {
                source: self.lock_source.clone(),
                chase: ChaseState::Locked,
                last_code: code,
                frames_since_code: Some(0.0),
                announced_rate: Some(CodeRate::of(rate).label().to_string()),
                problem,
            };
        }
        LockStatus { problem, ..LockStatus::default() }
    }

    // ---- record ----

    fn recording(&self) -> bool {
        self.transport.record && self.transport.run == Run::Playing
    }

    /// Start a pass when Record and Play are both on; end it when either goes off.
    fn sync_capture(&mut self) {
        if self.recording() {
            if self.captures.is_empty() && self.entry().is_some() {
                // A virtual rig starts the pass where the timeline is, on the axes that are not in
                // someone's hand, so a playhead that moved with Play is not in the first frame.
                for i in 0..self.units.len() {
                    if self.units[i].rig.is_simulator() {
                        let target = self.timeline_pose(&self.units[i].id);
                        let axes = self.playing_axes(i);
                        self.units[i].rig.sync_virtual(&target, &axes);
                    }
                }
                // The pass includes the frame Play was pressed on. Every camera is captured together.
                let rate = self.rate();
                self.captures = self
                    .units
                    .iter()
                    .map(|unit| {
                        let mut capture = Capture::new(&unit.id, self.record_axes.clone());
                        capture.feed(self.frame_f - rate.seconds_to_frames(unit.rig.query_delay_s()), unit.rig.pose());
                        capture
                    })
                    .collect();
                let cameras: Vec<&str> = self.captures.iter().map(|c| c.camera.as_str()).collect();
                self.log(
                    "record_start",
                    json!({ "cameras": cameras, "frame": self.frame_f.ceil() as i64, "axes": self.record_axes }),
                );
            }
        } else if !self.captures.is_empty() {
            // Only a camera that was moved has a pass to keep. The others go back to the timeline.
            let captures = std::mem::take(&mut self.captures);
            let mut kept = 0;
            for capture in captures {
                if self.moved(&capture) {
                    kept += 1;
                    self.finish_pass(capture);
                } else if let Ok(i) = self.unit_index(Some(&capture.camera)) {
                    let unit = &mut self.units[i];
                    let _ = unit.rig.jog(Jog::default());
                    unit.jog_deadline = None;
                    unit.manual = false;
                }
            }
            if kept == 0 {
                let song = self.current.clone();
                self.log("record_discarded", json!({ "song": song, "reason": "No camera was moved during the pass" }));
            }
        }
    }

    /// Was this camera moved by hand during the pass? A virtual rig is, when it was driven. A
    /// real head is, when its position changed: in Rehearse only its own controller moves it.
    fn moved(&self, capture: &Capture) -> bool {
        let Ok(i) = self.unit_index(Some(&capture.camera)) else { return false };
        if self.units[i].rig.is_simulator() {
            capture.driven()
        } else {
            capture.axes.iter().any(|a| capture.travel(*a) > 2.0 * default_tolerance(*a))
        }
    }

    /// The axes of a virtual rig that play the timeline now: all of them, or all but the
    /// record-enabled ones while Record is on and the rig is in someone's hand.
    fn playing_axes(&self, i: usize) -> Vec<Axis> {
        let in_hand = self.units[i].manual && self.transport.record;
        Axis::ALL.into_iter().filter(|a| !(in_hand && self.record_axes.contains(a))).collect()
    }

    fn free_pass_id(&self, song: &str) -> String {
        (1..)
            .map(|n| format!("pass-{n:04}"))
            .find(|id| self.storage.pass_path(song, id).is_some_and(|p| !p.exists()))
            .unwrap_or_else(|| "pass".into())
    }

    /// Turn a finished capture into keys on the timeline. The pass replaces
    /// what was on its axes over the frames it covers.
    fn finish_pass(&mut self, capture: Capture) {
        let Ok(i) = self.unit_index(Some(&capture.camera)) else { return };
        let unit = &mut self.units[i];
        let _ = unit.rig.jog(Jog::default());
        unit.jog_deadline = None;
        unit.manual = false;
        let query_delay_s = unit.rig.query_delay_s();
        let Some(song_id) = self.current.clone() else { return };
        // Only what was moved is kept: the axes that left where they stood, from half a second
        // before the first of them did.
        let lead = (self.rate().fps() / 2.0).round() as usize;
        let (dropped, samples) = record::moved_part(&capture.samples(), lead).unwrap_or_default();
        let frames = samples.values().map(Vec::len).min().unwrap_or(0) as i64;
        if frames < 2 {
            let reason =
                if capture.frames() < 2 { "The pass was shorter than two frames" } else { "No axis was moved during the pass" };
            self.log("record_discarded", json!({ "song": song_id, "camera": capture.camera, "reason": reason }));
            return;
        }
        let pass = RecordedPass {
            id: self.free_pass_id(&song_id),
            song_id: song_id.clone(),
            camera: capture.camera.clone(),
            recorded_at: storage::now_rfc3339(),
            rate: self.rate(),
            start_frame: capture.start_frame() + dropped as i64,
            query_delay_s,
            samples,
        };
        let saved = match self.storage.pass_path(&song_id, &pass.id) {
            Some(path) => storage::write_json(&path, &pass).map_err(|e| e.to_string()),
            None => Err("the pass has no usable file name".into()),
        };
        if let Err(e) = &saved {
            self.log(
                "record_warning",
                json!({ "song": song_id, "pass": pass.id, "problem": format!("Raw samples were not kept: {e}") }),
            );
        }
        let curves = record::fit_pass(&pass.samples, 1.0);
        let keys: BTreeMap<Axis, usize> = curves.iter().map(|(a, k)| (*a, k.len())).collect();
        let Some(entry) = self.songs.get(&song_id) else { return };
        let punched = edit::punch_keys(
            &entry.song,
            &self.project,
            &pass.camera,
            pass.start_frame,
            frames - 1,
            curves,
            saved.is_ok().then(|| pass.id.clone()),
        );
        match punched {
            Ok((song, clip)) => match self.replace_working(&song_id, song, "record") {
                Ok(_) => self.log(
                    "record_pass",
                    json!({
                        "song": song_id, "camera": pass.camera, "clip": clip, "pass": pass.id,
                        "start_frame": pass.start_frame, "frames": frames, "axes": keys.keys().collect::<Vec<_>>(), "keys": keys,
                    }),
                ),
                Err(fail) => self.log("record_failed", json!({ "song": song_id, "pass": pass.id, "reason": fail.msg })),
            },
            Err(refusal) => self.log("record_failed", json!({ "song": song_id, "pass": pass.id, "reason": refusal.to_string() })),
        }
    }

    /// Fit a recorded clip again from its raw samples, tighter or looser.
    pub fn refit(&mut self, client: &str, clip_id: &str, req: &Refit) -> Res<WorkingSong> {
        self.require_lease(client)?;
        if !(req.scale.is_finite() && (0.05..=50.0).contains(&req.scale)) {
            return Err(Fail::bad("The fit tolerance scale must be between 0.05 and 50"));
        }
        let Some(entry) = self.entry() else {
            return Err(Fail::refused("Create or load a song first"));
        };
        let song_id = entry.song.id.clone();
        self.require_editable(&song_id)?;
        let Some(clip) = entry.song.motion_clip(clip_id) else {
            return Err(Fail::not_found(format!("No clip with id {clip_id} in {}", entry.song.name)));
        };
        let Some(pass_id) = &clip.recorded_pass else {
            return Err(Fail::refused("This clip was not recorded, so there is nothing to refit"));
        };
        let pass: Option<RecordedPass> = self.storage.pass_path(&song_id, pass_id).and_then(|p| storage::read_json(&p));
        let Some(pass) = pass else {
            return Err(Fail::refused(format!("The raw samples of {pass_id} are missing from the data folder")));
        };
        let whole = clip.axes.iter().all(|a| pass.samples.get(a).is_some_and(|v| v.len() as i64 == clip.duration_frames + 1));
        if !whole {
            return Err(Fail::refused("This clip was trimmed or split after it was recorded. Refit works on a whole pass"));
        }
        let samples: BTreeMap<Axis, Vec<f64>> = pass.samples.into_iter().filter(|(a, _)| clip.axes.contains(a)).collect();
        let curves = record::fit_pass(&samples, req.scale);
        let song = edit::replace_curves(&entry.song, &self.project, clip_id, curves).map_err(|r| Fail::invalid(r.to_string()))?;
        let out = self.replace_working(&song_id, song, "refit")?;
        self.log(
            "record_refit",
            json!({ "song": song_id, "clip": clip_id, "pass": pass_id_of(&out.song, clip_id), "scale": req.scale }),
        );
        Ok(out)
    }

    /// The raw samples of a recorded clip, when the clip still is the whole pass: one value
    /// per frame of the clip on each of its axes.
    fn raw_samples(&self, song_id: &str, clip: &MotionClip) -> Option<BTreeMap<Axis, Vec<f64>>> {
        let path = self.storage.pass_path(song_id, clip.recorded_pass.as_ref()?)?;
        let pass: RecordedPass = storage::read_json(&path)?;
        let whole = clip.axes.iter().all(|a| pass.samples.get(a).is_some_and(|v| v.len() as i64 == clip.duration_frames + 1));
        whole.then_some(pass.samples)
    }

    /// Smooth the recorded moves of the loaded song, or of one camera in it, until each is
    /// inside its rig's speed and acceleration limits, then accept the ones that still are
    /// not. A move is smoothed from its raw samples while it still is the whole pass, so
    /// smoothing again starts from the same take; a trimmed one from its keys. A move that was
    /// accepted while still over a limit is smoothed again, and stays accepted only if it still
    /// is over. Range, jump and keep-out problems are never accepted here:
    /// the report lists them as left. The rig's limits are read, never changed.
    pub fn smooth(&mut self, client: &str, req: &Smooth) -> Res<Smoothed> {
        self.require_lease(client)?;
        let Some(entry) = self.entry() else {
            return Err(Fail::refused("Create or load a song first"));
        };
        let song_id = entry.song.id.clone();
        self.require_editable(&song_id)?;
        if let Some(camera) = req.camera.as_ref().filter(|id| self.project.camera(id).is_none()) {
            return Err(Fail::not_found(format!("There is no camera {camera} in the project")));
        }
        let wanted = |c: &&MotionClip| c.recorded_pass.is_some() && req.camera.as_ref().is_none_or(|id| *id == c.camera);
        let recorded: Vec<MotionClip> = entry.song.motion.iter().filter(wanted).cloned().collect();
        if recorded.is_empty() {
            return Err(Fail::refused(match &req.camera {
                Some(camera) => format!("{} has no recorded moves in this song", self.project.label(camera)),
                None => "This song has no recorded moves to smooth".into(),
            }));
        }
        let fps = self.rate().fps();
        let mut report = SmoothReport {
            moves: recorded.len(),
            blocking_before: entry.compiled.blocking_count(&entry.song),
            ..SmoothReport::default()
        };
        let mut song = entry.song.clone();
        for clip in &recorded {
            let Some(camera) = self.project.camera(&clip.camera) else { continue };
            let fixable =
                |i: &&Issue| i.clip_id() == Some(clip.id.as_str()) && (i.acceptable() || matches!(i, Issue::Jump { .. }));
            if !entry.compiled.issues.iter().any(|i| fixable(&i)) {
                continue;
            }
            let raw = self.raw_samples(&song_id, clip);
            // Where the timeline has the camera when the take begins. A take that begins
            // somewhere else is a step no rig can make: it is given the move there instead.
            let before: BTreeMap<Axis, f64> = {
                let ev = Evaluator::new(&song, &clip.camera);
                clip.axes.iter().map(|a| (*a, ev.value_before(clip, *a))).collect()
            };
            let mut curves = BTreeMap::new();
            for &axis in &clip.axes {
                let (keys, limits) = (clip.keys(axis), camera.axes.get(axis));
                let plays: Vec<f64> = (0..=clip.duration_frames).filter_map(|f| keys_value(keys, f as f64)).collect();
                let steps = clip.start_frame > 0 && plays.first().is_some_and(|v| (v - before[&axis]).abs() > 1e-9);
                // An axis that is inside its limits and starts where the camera is keeps the keys it has.
                if !steps && !overruns(&plays, limits, fps) {
                    continue;
                }
                let mut samples = raw.as_ref().and_then(|r| r.get(&axis)).unwrap_or(&plays).clone();
                if let Some(first) = samples.first_mut().filter(|_| steps) {
                    *first = before[&axis];
                }
                let (smooth, departure) = record::smooth_axis(&samples, axis, limits, fps);
                if smooth != keys {
                    let furthest = report.departure.entry(axis).or_insert(0.0);
                    *furthest = furthest.max(departure);
                    curves.insert(axis, smooth);
                }
            }
            if !curves.is_empty() {
                song = edit::replace_curves(&song, &self.project, &clip.id, curves).map_err(|r| Fail::invalid(r.to_string()))?;
                report.smoothed.push(clip.id.clone());
            }
        }
        // What is still over a limit is accepted the way the Inspector accepts it: only on a
        // move whose every issue is one that may be accepted.
        let by = self.lease.as_ref().map(|l| l.name.clone()).unwrap_or_default();
        let at = storage::now_rfc3339();
        let compiled = compile(&song, &self.project);
        let mut decisions = Vec::new();
        for clip in &recorded {
            let issues: Vec<&Issue> = compiled.issues.iter().filter(|i| i.clip_id() == Some(clip.id.as_str())).collect();
            let accepted = song.motion_clip(&clip.id).is_some_and(|c| c.accepted.is_some());
            if issues.is_empty() || accepted || !issues.iter().all(|i| i.acceptable()) {
                continue;
            }
            let said: Vec<String> = issues.iter().map(|i| i.message_in(self.project.units)).collect();
            song = edit::accept_issues(&song, &self.project, &clip.id, &by, &at).map_err(|r| Fail::invalid(r.to_string()))?;
            decisions.push(json!({ "song": song_id, "clip": clip.id, "by": by, "at": at, "issues": said }));
            report.accepted.push(clip.id.clone());
        }
        report.left = compiled.blocking(&song).cloned().collect();
        if song == entry.song {
            return Ok(Smoothed { working: entry.working(), report });
        }
        let working = self.replace_working(&song_id, song, "smooth")?;
        for data in decisions {
            self.log("issue_accepted", data);
        }
        self.log(
            "record_smooth",
            json!({
                "song": song_id, "camera": req.camera, "by": by, "moves": report.moves,
                "smoothed": report.smoothed, "accepted": report.accepted, "departure": report.departure,
                "blocking_before": report.blocking_before, "left": report.left.len(),
            }),
        );
        Ok(Smoothed { working, report })
    }

    /// Drive one camera's rig by hand.
    pub fn jog(&mut self, client: &str, camera: Option<&str>, jog: Jog) -> Res<()> {
        self.require_lease(client)?;
        let i = self.unit_index(camera)?;
        if self.transport.estop {
            return Err(Fail::refused("Reset E-Stop to continue"));
        }
        if self.transport.output != Output::Rehearse {
            return Err(Fail::refused("Disarm to drive the rig by hand"));
        }
        if self.transport.run != Run::Stopped && !self.transport.record {
            return Err(Fail::refused("Stop to position the rig by hand, or turn Record on to capture the move"));
        }
        // During a pass only the record-enabled axes are hand-driven; the rest play the timeline.
        let jog = if self.transport.run != Run::Stopped {
            let keep = |a: Axis| if self.record_axes.contains(&a) { jog.get(a) } else { 0.0 };
            Jog {
                pan: keep(Axis::Pan),
                tilt: keep(Axis::Tilt),
                zoom: keep(Axis::Zoom),
                focus: keep(Axis::Focus),
                elevation: keep(Axis::Elevation),
            }
        } else {
            jog
        };
        let unit = &mut self.units[i];
        unit.rig.jog(jog).map_err(Fail::refused)?;
        if !jog.is_zero() {
            // Moved during a pass: this camera's pass is one to keep.
            if let Some(capture) = self.captures.iter_mut().find(|c| c.camera == unit.id) {
                capture.mark_driven();
            }
        }
        if jog.is_zero() {
            unit.jog_deadline = None;
        } else {
            unit.manual = true;
            // The app repeats jog while a control is held; a dropped connection stops the rig.
            unit.jog_deadline = Some(Instant::now() + Duration::from_millis(250));
        }
        Ok(())
    }

    // ---- tick ----

    /// Where the timeline puts a camera at the playhead.
    fn timeline_pose(&self, camera: &str) -> Pose {
        let Some(e) = self.entry() else { return Pose::home() };
        let poses = e.compiled.poses(camera);
        if poses.is_empty() {
            return e.song.home_pose;
        }
        let f = self.frame_f.clamp(0.0, (poses.len() - 1) as f64);
        let i = f.floor() as usize;
        let j = (i + 1).min(poses.len() - 1);
        lerp_pose(&poses[i], &poses[j], f - i as f64)
    }

    /// The pose the monitor shows for a camera, and where it comes from.
    fn monitor_pose(&self, i: usize) -> (Pose, PoseSource) {
        let unit = &self.units[i];
        let passing = self.transport.record && self.transport.run != Run::Stopped;
        if self.transport.hardware_live() {
            (unit.rig.pose(), PoseSource::Rig)
        } else if passing {
            (unit.rig.pose(), PoseSource::Record)
        } else if !passing && unit.manual && unit.rig.is_simulator() {
            (unit.rig.pose(), PoseSource::Rig)
        } else {
            (self.timeline_pose(&unit.id), PoseSource::Timeline)
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().min(0.25);
        self.last_tick = now;

        if self.lease.as_ref().is_some_and(|l| now.duration_since(l.last_seen) > self.lease_ttl) {
            let name = self.lease.take().map(|l| l.name);
            self.log("lease", json!({ "expired": name }));
            self.poke();
        }
        if self.panel.as_ref().is_some_and(|(_, _, seen)| now.duration_since(*seen) > PANEL_TTL) {
            let station = self.panel.take().map(|(p, _, _)| p.station);
            self.log("panel", json!({ "station": station, "gone": true }));
            self.poke();
        }
        for u in &mut self.units {
            if u.jog_deadline.is_some_and(|d| now > d) {
                // A lost connection halts on the spot; letting go of a control ramps down instead.
                u.rig.stop();
                u.jog_deadline = None;
            }
        }
        self.collect_loaded_audio();

        if self.entry().is_some() {
            let length = self.length() as f64;
            if self.transport.time == TimeMode::LockedExternal {
                self.tick_external(now);
            } else {
                // One reading of the audio clock per tick: the playhead and the drift figure share it.
                let audio_pos = self.audio.position(self.rate());
                if self.transport.run == Run::Playing {
                    // Locked to a file, the show is the stretch that file covers.
                    let (first, end) = match self.transport.time {
                        TimeMode::LockedFile => self.locked_file_span().unwrap_or((0, 0)),
                        _ => (0, self.length()),
                    };
                    let f = self.clock.follow(audio_pos, now, self.rate().fps());
                    if f >= end as f64 {
                        self.frame_f = (end as f64).min(length);
                        self.event(Event::SongEnded);
                    } else if f < first as f64 - 0.5 {
                        self.frame_f = f.max(0.0);
                        self.event(Event::SongEnded);
                    } else {
                        self.frame_f = f;
                    }
                }
                self.audio_drift = if self.audio.is_playing() { audio_pos - self.frame_f } else { 0.0 };
            }

            if self.transport.hardware_live() {
                self.tick_cameras(now, dt);
            } else {
                // In Rehearse the virtual rigs show the timeline; hardware is sent nothing.
                let recording = !self.captures.is_empty();
                for i in 0..self.units.len() {
                    if !self.units[i].rig.is_simulator() {
                        continue;
                    }
                    let target = self.timeline_pose(&self.units[i].id);
                    // During a pass a rig in someone's hand keeps its record-enabled axes; every
                    // other rig, and every other axis, plays the timeline.
                    if recording || !self.units[i].manual {
                        let axes = self.playing_axes(i);
                        self.units[i].rig.sync_virtual(&target, &axes);
                    }
                }
            }
        }
        for (unit, camera) in self.units.iter_mut().zip(&self.project.cameras) {
            unit.rig.tick(dt, &camera.axes);
        }
        self.ticks += 1;
        self.tick_rig(now);
        let rate = self.rate();
        for capture in &mut self.captures {
            if let Some(unit) = self.units.iter().find(|u| u.id == capture.camera) {
                // The pose just read belongs to where the transport was `query_delay_s` ago.
                capture.feed(self.frame_f - rate.seconds_to_frames(unit.rig.query_delay_s()), unit.rig.pose());
            }
        }
    }

    // ---- status ----

    pub fn status(&self, client: &str) -> Status {
        let rate = self.rate();
        let entry = self.entry();
        let frame = self.frame_f.floor() as i64;
        let cameras: Vec<CameraStatus> = (0..self.units.len())
            .map(|i| {
                let unit = &self.units[i];
                let (pose, pose_source) = self.monitor_pose(i);
                CameraStatus {
                    id: unit.id.clone(),
                    pose,
                    pose_source,
                    rig_pose: unit.rig.pose(),
                    error: unit.live.error,
                    fault: unit.live.fault.clone(),
                    held: unit.held,
                    halting: unit.halting.is_some(),
                    preset_sync: unit.rig.preset_sync_progress(),
                    hardware: !unit.rig.is_simulator(),
                    adapters: unit.rig.status(),
                }
            })
            .collect();
        let telemetry = Telemetry {
            fault: (0..self.units.len())
                .find_map(|i| self.units[i].live.fault.as_ref().map(|why| format!("{}{why}", self.prefix(i)))),
            halting: cameras.iter().any(|c| c.halting),
            preset_sync: (0..self.units.len())
                .find_map(|i| cameras[i].preset_sync.as_ref().map(|at| format!("{}{at}", self.prefix(i)))),
            hardware: cameras.iter().any(|c| c.hardware),
        };
        let timecode = match entry {
            Some(e) => e.song.clock(rate).timecode_at(frame).to_string(),
            None => Timecode::from_frames(0, rate).to_string(),
        };
        let now = Instant::now();
        Status {
            server_time_ms: now.duration_since(self.started).as_millis() as u64,
            song: entry.map(|e| SongStatus {
                id: e.song.id.clone(),
                name: e.song.name.clone(),
                start_tc: e.song.start_tc.clone(),
                length_frames: e.song.length_frames,
                revision: e.revision(),
                dirty: e.dirty(),
                committed_clean: e.committed_clean(),
                edit_counter: e.counter,
                issues: e.compiled.issues.len(),
                blocking: e.compiled.blocking_count(&e.song),
            }),
            transport: self.transport,
            frame,
            frame_f: self.frame_f,
            timecode,
            rate,
            cameras,
            hardware_live: self.transport.hardware_live(),
            lease: self.lease.as_ref().map(|l| LeaseStatus {
                holder: l.name.clone(),
                client_id: l.client.clone(),
                expires_in_ms: self.lease_ttl.saturating_sub(now.duration_since(l.last_seen)).as_millis() as u64,
            }),
            you_hold_lease: self.holds_lease(client),
            guidance: transport::guidance(&self.transport, &self.context(client)),
            audio: self.audio_status(),
            lock: self.lock_status(),
            record: RecordStatus {
                camera: self.record_camera.clone(),
                axes: self.record_axes.clone(),
                active: !self.captures.is_empty(),
                frames: self.captures.iter().map(|c| c.frames() as i64).max().unwrap_or(0),
                moved: self.captures.iter().filter(|c| self.moved(c)).map(|c| c.camera.clone()).collect(),
            },
            telemetry,
            panel: self.panel.as_ref().map(|(p, _, _)| p.clone()),
            panel_host: self.panel_host.clone(),
        }
    }
}

fn pass_id_of(song: &Song, clip: &str) -> Option<String> {
    song.motion_clip(clip).and_then(|c| c.recorded_pass.clone())
}

/// The audio device says where it is once per callback, and callbacks land a millisecond
/// early or late. The playhead follows the device through a slow filter, so it advances
/// evenly and still cannot drift from the sound.
#[derive(Default)]
struct SteadyClock {
    last: Option<Followed>,
}

#[derive(Clone, Copy)]
struct Followed {
    /// The answer given, when, and the device position it was given for.
    at: f64,
    when: Instant,
    device: f64,
    /// The device had moved since the reading before.
    moving: bool,
}

impl SteadyClock {
    /// Share of the difference from the device taken up on each reading.
    const GAIN: f64 = 0.05;
    /// Further from the device than this is a start or a seek, not jitter.
    const SNAP_S: f64 = 0.05;

    fn follow(&mut self, device: f64, now: Instant, fps: f64) -> f64 {
        let moving = self.last.is_some_and(|l| l.device != device);
        let at = match self.last {
            // The device's first step is where it started from, at a moment between two
            // readings: there is nothing to coast from until it is already under way.
            Some(l) if moving && l.moving => {
                let coasted = l.at + now.duration_since(l.when).as_secs_f64() * fps;
                let off = device - coasted;
                if off.abs() > Self::SNAP_S * fps {
                    device
                } else {
                    coasted + off * Self::GAIN
                }
            }
            _ => device,
        };
        self.last = Some(Followed { at, when: now, device, moving });
        at
    }

    fn reset(&mut self) {
        self.last = None;
    }
}

/// Who the run log names for the E-Stop a stopping server presses. No client has this id.
const STOPPING: &str = "";

/// The server is about to exit. Stop every rig the way E-Stop does and wait, up to
/// `timeout`, until each reports standing still. A rig keeps its last speed command
/// when its controller goes away, so this has to finish before the process does.
/// Returns false when a rig had not confirmed in time.
pub fn halt_for_exit(show: &Shared, timeout: Duration) -> bool {
    {
        let mut s = show.lock().unwrap_or_else(|e| e.into_inner());
        let quiet = s.transport.output == Output::Rehearse && s.units.iter().all(|u| u.rig.is_simulator());
        if quiet {
            return true;
        }
        let was = s.transport;
        if was.estop || s.act(STOPPING, Action::Estop, &TransportRequest::default()).is_err() {
            for u in &mut s.units {
                u.rig.stop();
            }
        }
        s.log("server_stopping", json!({ "output": was.output, "run": was.run }));
    }
    let deadline = Instant::now() + timeout;
    loop {
        let moving = show.lock().unwrap_or_else(|e| e.into_inner()).units.iter().any(|u| u.rig.is_moving());
        if !moving {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Start the 60 Hz tick thread. It stops when the last other handle is dropped.
pub fn spawn_tick(show: &Shared) {
    let weak = Arc::downgrade(show);
    std::thread::Builder::new()
        .name("show-tick".into())
        .spawn(move || {
            let period = Duration::from_micros(16_667);
            let mut next = Instant::now() + period;
            loop {
                let Some(show) = weak.upgrade() else { break };
                if let Ok(mut s) = show.lock() {
                    s.tick();
                }
                drop(show);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
                next = (next + period).max(Instant::now());
            }
        })
        .expect("tick thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_project_names_the_output_unless_the_server_was_started_silent() {
        let named = Project { audio_device: " Desk ".into(), ..Project::default() };
        let unset = Project::default();
        let started = AudioChoice::Named("USB".into());
        assert_eq!(AudioChoice::Default.for_project(&named), AudioChoice::Named("Desk".into()));
        assert_eq!(started.for_project(&named), AudioChoice::Named("Desk".into()));
        assert_eq!(AudioChoice::Default.for_project(&unset), AudioChoice::Default);
        assert_eq!(started.for_project(&unset), started);
        assert_eq!(AudioChoice::Off.for_project(&named), AudioChoice::Off);
    }

    #[test]
    fn the_playhead_follows_the_device_without_its_callback_jitter() {
        let fps = 25.0;
        let start = Instant::now();
        let mut clock = SteadyClock::default();
        let mut seed = 11u32;
        let mut worst_step = 0.0f64;
        let mut last = 0.0;
        for tick in 0..600u64 {
            let now = start + Duration::from_micros(tick * 16_667);
            let true_pos = tick as f64 * 0.016_667 * fps;
            // Up to a millisecond either way, like a callback that ran early or late.
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            let jitter = ((seed >> 8) as f64 / (1 << 24) as f64 - 0.5) * 0.002 * fps;
            let at = clock.follow(true_pos + jitter, now, fps);
            if tick > 0 {
                worst_step = worst_step.max((at - last - 0.016_667 * fps).abs());
            }
            last = at;
            assert!((at - true_pos).abs() <= 0.001 * fps, "tick {tick}: {at} against {true_pos}");
        }
        // Raw readings step unevenly by up to 2 ms; the playhead by a tenth of that.
        assert!(worst_step < 0.0002 * fps, "{worst_step}");

        // A seek is taken at once, not eased into.
        let now = start + Duration::from_secs(11);
        assert_eq!(clock.follow(40.0, now, fps), 40.0);
        clock.reset();
        assert_eq!(clock.follow(7.0, now + Duration::from_millis(16), fps), 7.0);
    }
}
