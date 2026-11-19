//! Wire types shared by the server, the app and the probe.

use crate::chase::ChaseState;
use crate::compile::Issue;
use crate::pose::{Axis, Pose, Preset, Routing, Units};
use crate::song::Song;
use crate::time::FrameRate;
use crate::transport::{Guidance, TransportState};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const API: &str = "/api/v1";
pub const DEFAULT_PORT: u16 = 8787;
pub const HEADER_CLIENT: &str = "x-client-id";
pub const HEADER_KEY: &str = "x-access-key";

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SongStatus {
    pub id: String,
    pub name: String,
    pub start_tc: String,
    pub length_frames: i64,
    /// Last committed revision, if any.
    pub revision: Option<u32>,
    /// Working copy differs from the last Save.
    pub dirty: bool,
    /// The working copy is exactly the last committed revision.
    pub committed_clean: bool,
    pub edit_counter: u64,
    pub issues: usize,
    pub blocking: usize,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct LeaseStatus {
    pub holder: String,
    pub client_id: String,
    pub expires_in_ms: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum PoseSource {
    /// The programmed pose at the playhead.
    #[default]
    Timeline,
    /// The rig's actual pose (hand-driven, or live).
    Rig,
    /// Recording: enabled axes from the rig, the rest from the timeline.
    Record,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct AudioStatus {
    /// Output device name, or `None` when running silent.
    pub output: Option<String>,
    pub silent_reason: Option<String>,
    pub device_rate: u32,
    /// The routing in use: the project's, with Program on both outputs while the song plays a
    /// file whose timecode channel is read.
    pub routing: Routing,
    /// Device callbacks so far. Rising proves the device is pulling audio.
    pub callbacks: u64,
    pub position_frame: f64,
    /// Audio position minus transport position, in frames.
    pub drift_frames: f64,
    pub peak_left: f32,
    pub peak_right: f32,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct LockStatus {
    /// `file:<clip id>`, `ltc`, `mtc` or `artnet`.
    pub source: Option<String>,
    pub chase: ChaseState,
    pub last_code: Option<String>,
    pub frames_since_code: Option<f64>,
    pub announced_rate: Option<String>,
    /// Problem opening the input, when there is one.
    pub problem: Option<String>,
}

/// What the server machine has to take timecode from and to play sound through.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct InputChoices {
    pub audio_inputs: Vec<String>,
    /// The audio outputs the server could play through.
    #[serde(default)]
    pub audio_outputs: Vec<String>,
    /// Why audio inputs cannot be listed, when they cannot.
    pub audio_problem: Option<String>,
    pub midi_inputs: Vec<String>,
    pub midi_problem: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct AdapterStatus {
    pub name: String,
    pub connected: bool,
    pub detail: String,
    /// AutoPod+ only.
    pub calibrated: Option<bool>,
    pub round_trip_ms: Option<f64>,
    /// AutoPod+ only: the raw position value, for building the height map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_position: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct RecordStatus {
    /// The camera that was selected when Record was turned on. A pass is kept for every camera
    /// that is moved, whichever was selected.
    #[serde(default = "crate::camera::first_camera")]
    pub camera: String,
    /// Axes that Record will write.
    pub axes: Vec<Axis>,
    /// A pass is being captured now.
    pub active: bool,
    pub frames: i64,
    /// The cameras moved so far in this pass: the ones it will be kept for.
    #[serde(default)]
    pub moved: Vec<String>,
}

impl Default for RecordStatus {
    fn default() -> Self {
        RecordStatus { camera: crate::camera::first_camera(), axes: Vec::new(), active: false, frames: 0, moved: Vec::new() }
    }
}

/// The show as a whole. Each camera's own state is in `Status::cameras`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct Telemetry {
    /// The first camera fault, named by camera when the project has several.
    pub fault: Option<String>,
    /// E-Stop was pressed and some rig has not yet reported that it stands still.
    pub halting: bool,
    /// Progress of a preset sync that is moving a camera.
    pub preset_sync: Option<String>,
    /// At least one camera is real hardware, not the simulator.
    pub hardware: bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct CameraStatus {
    pub id: String,
    /// What the monitor shows for this camera.
    pub pose: Pose,
    pub pose_source: PoseSource,
    /// Latest device feedback. Consult the adapters for connection state.
    pub rig_pose: Pose,
    /// The current show target, even when the rig is held, disarmed or in hand.
    pub intended_pose: Pose,
    /// This camera is enabled for automation when the show is live.
    #[serde(default = "camera_armed_by_default")]
    pub armed: bool,
    /// A current, nonzero jog owns the camera.
    pub manual: bool,
    /// Automation is reconciling the rig to the current show target.
    pub catching_up: bool,
    /// Rig position minus target per axis while live.
    pub error: Pose,
    pub fault: Option<String>,
    /// Faulted while live and left standing where it stopped; the others carry on.
    pub held: bool,
    /// E-Stop was pressed and this rig has not yet reported that it stands still.
    pub halting: bool,
    pub preset_sync: Option<String>,
    /// Real hardware, not the simulator.
    pub hardware: bool,
    pub adapters: Vec<AdapterStatus>,
}

fn camera_armed_by_default() -> bool {
    true
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct CameraArmRequest {
    pub armed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Status {
    pub server_time_ms: u64,
    pub song: Option<SongStatus>,
    pub transport: TransportState,
    pub frame: i64,
    /// Playhead with its fraction, for smooth drawing between frames.
    pub frame_f: f64,
    pub timecode: String,
    pub rate: FrameRate,
    /// One per camera of the project, in the project's order. Never empty.
    pub cameras: Vec<CameraStatus>,
    pub hardware_live: bool,
    pub lease: Option<LeaseStatus>,
    pub you_hold_lease: bool,
    /// This station has confirmed it is editing the loaded song while the show is live.
    #[serde(default)]
    pub live_edit: bool,
    pub guidance: Guidance,
    pub audio: AudioStatus,
    pub lock: LockStatus,
    pub record: RecordStatus,
    pub telemetry: Telemetry,
    /// The hardware panel controlling the show, while one is reporting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel: Option<PanelState>,
    /// The panel the server itself runs or is trying to run, when it was asked to run one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panel_host: Option<PanelHost>,
    /// Absent on servers predating configurable OSC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub osc: Option<OscStatus>,
}

/// OSC settings belong to the server's data folder and survive project edits and restarts.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OscConfig {
    pub enabled: bool,
    pub bind: std::net::SocketAddr,
}

impl Default for OscConfig {
    fn default() -> Self {
        Self { enabled: false, bind: ([127, 0, 0, 1], 9000).into() }
    }
}

impl OscConfig {
    pub fn validate(self, has_key: bool) -> Result<(), String> {
        if self.bind.port() == 0 {
            return Err("Choose a UDP port from 1 to 65535".into());
        }
        if self.enabled && !self.bind.ip().is_loopback() && !has_key {
            return Err(
                "LAN OSC needs a server access key. Set the key when starting the local server or use --access-key.".into()
            );
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct OscStatus {
    pub config: OscConfig,
    pub listening: Option<std::net::SocketAddr>,
    pub problem: Option<String>,
    pub from_command_line: bool,
    pub has_access_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_packet: Option<OscPacketStatus>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct OscPacketStatus {
    pub sender: std::net::SocketAddr,
    pub seconds_ago: u64,
}

/// The panel the server runs for itself: where it is, who asked for it, and what is in the way.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct PanelHost {
    /// The address the server connects to, with its port; or where it waits for a panel that dials in.
    pub address: String,
    /// It was asked for where the server was started, not in Show settings, and is changed there.
    pub from_command_line: bool,
    /// Why the server could not start running the panel, when it could not.
    pub problem: Option<String>,
}

/// What a hardware panel controller is doing, so the app can follow it. The
/// controller sends this about once a second; the server drops it when it stops.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct PanelState {
    /// The station name the controller takes the edit lease under.
    pub station: String,
    /// The controller has a connection to the panel itself.
    pub connected: bool,
    /// The panel's model id and the name its owner gave it, as the panel reports them.
    pub model: String,
    pub name: String,
    /// The mode the panel is in, as its display names it: Show, Transport, Camera, ...
    pub mode: String,
    /// The camera the panel selects and drives.
    pub camera: String,
    /// Ids of the motion and media clips selected on the panel.
    pub clips: Vec<String>,
    /// Ids of the markers selected on the panel.
    pub markers: Vec<String>,
    /// The message on the panel's main display, empty when there is none.
    pub message: String,
}

impl Status {
    pub fn camera(&self, id: &str) -> Option<&CameraStatus> {
        self.cameras.iter().find(|c| c.id == id)
    }

    /// The first camera: the whole rig of a one-camera project.
    pub fn first(&self) -> &CameraStatus {
        static NONE: std::sync::OnceLock<CameraStatus> = std::sync::OnceLock::new();
        self.cameras.first().unwrap_or_else(|| NONE.get_or_init(CameraStatus::default))
    }

    /// Every adapter of every camera.
    pub fn adapters(&self) -> impl Iterator<Item = &AdapterStatus> {
        self.cameras.iter().flat_map(|c| c.adapters.iter())
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct SongSummary {
    pub id: String,
    pub name: String,
    pub start_tc: String,
    pub length_frames: i64,
    pub revision: Option<u32>,
    pub dirty: bool,
    pub loaded: bool,
}

/// What Save wrote: the draft of every song that had unsaved changes.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct SavedAll {
    /// The names of the songs that were saved.
    pub songs: Vec<String>,
    /// The loaded song as it stands now, when there is one.
    pub working: Option<WorkingSong>,
}

impl SavedAll {
    /// What to tell the operator.
    pub fn text(&self) -> String {
        match &self.songs[..] {
            [] => "Nothing to save".into(),
            [one] => format!("Saved {one}"),
            many => format!("Saved {} songs", many.len()),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct WorkingSong {
    pub song: Song,
    pub edit_counter: u64,
    pub dirty: bool,
    pub revision: Option<u32>,
    pub issues: Vec<Issue>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct PutWorking {
    pub expected_counter: u64,
    pub song: Song,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct NewSong {
    pub name: String,
    #[serde(default)]
    pub start_tc: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct RevisionInfo {
    pub number: u32,
    pub sha256: String,
    pub committed_at: String,
    pub committed_by: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct LeaseRequest {
    pub name: String,
    /// Take the lease from the current holder.
    #[serde(default)]
    pub force: bool,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct TransportRequest {
    /// Seek target.
    #[serde(default)]
    pub frame: Option<i64>,
    /// Lock source: `file:<clip id>`, `ltc`, `mtc` or `artnet`.
    #[serde(default)]
    pub source: Option<String>,
    /// Axes for Record. Omitted keeps the current selection.
    #[serde(default)]
    pub axes: Option<Vec<Axis>>,
    /// Camera for Record. Omitted keeps the current one.
    #[serde(default)]
    pub camera: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub struct Jog {
    /// Each axis from -1 to 1, as a fraction of its top speed.
    pub pan: f64,
    pub tilt: f64,
    pub zoom: f64,
    pub focus: f64,
    pub elevation: f64,
}

impl Jog {
    pub fn get(&self, axis: Axis) -> f64 {
        match axis {
            Axis::Pan => self.pan,
            Axis::Tilt => self.tilt,
            Axis::Zoom => self.zoom,
            Axis::Focus => self.focus,
            Axis::Elevation => self.elevation,
        }
    }

    pub fn is_zero(&self) -> bool {
        Axis::ALL.iter().all(|a| self.get(*a) == 0.0)
    }
}

/// A jog and the camera it is for. Without a camera it drives the first.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct JogRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(flatten)]
    pub jog: Jog,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Audio,
    Video,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct MediaInfo {
    pub id: String,
    pub name: String,
    pub kind: MediaKind,
    pub channels: u32,
    pub sample_rate: u32,
    pub samples: u64,
    pub duration_s: f64,
    /// Length in timeline frames at the project rate.
    pub duration_frames: i64,
    /// Start timecode read from the file, if any.
    pub start_tc: Option<String>,
    /// `bext`, `quicktime`, `ltc channel N` or `typed`.
    pub start_tc_source: Option<String>,
    /// Proxy frames available for the reference monitor.
    pub proxy_frames: i64,
    /// Index of the channel carrying LTC (0 is the first), excluded from Program.
    pub ltc_channel: Option<u32>,
    /// Broadcast WAV time reference: samples since midnight at `sample_rate`.
    /// Kept so the start timecode can be restated when the project rate changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_samples: Option<u64>,
    /// The frame rate the proxy frames were made at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_rate: Option<FrameRate>,
}

impl MediaInfo {
    /// The song frame this file's first sample belongs on, when the file is stamped.
    pub fn frame_in(&self, song: &Song, rate: FrameRate) -> Option<i64> {
        song.clock(rate).frame_of_stamp(self.start_tc.as_deref()?)
    }
}

/// One level of a waveform overview: the lowest and highest sample in each
/// bucket, across channels, scaled to -127..=127. Sent as bytes, not JSON.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct PeakLevel {
    pub samples_per_bucket: u32,
    pub sample_rate: u32,
    pub pairs: Vec<(i8, i8)>,
}

impl PeakLevel {
    const MAGIC: &'static [u8; 4] = b"PTZW";

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(16 + self.pairs.len() * 2);
        out.extend_from_slice(Self::MAGIC);
        out.extend_from_slice(&self.samples_per_bucket.to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&(self.pairs.len() as u32).to_le_bytes());
        for (lo, hi) in &self.pairs {
            out.push(*lo as u8);
            out.push(*hi as u8);
        }
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<PeakLevel> {
        let word = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
        if bytes.get(..4)? != Self::MAGIC {
            return None;
        }
        let (samples_per_bucket, sample_rate, count) = (word(4)?, word(8)?, word(12)? as usize);
        let body = bytes.get(16..16 + count * 2)?;
        let pairs = body.as_chunks::<2>().0.iter().map(|c| (c[0] as i8, c[1] as i8)).collect();
        Some(PeakLevel { samples_per_bucket, sample_rate, pairs })
    }

    /// Seconds covered by one bucket.
    pub fn bucket_seconds(&self) -> f64 {
        self.samples_per_bucket as f64 / self.sample_rate.max(1) as f64
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct StoreTc {
    /// A typed start timecode; `ltc:<n>` to decode it from channel `n` (1 is the
    /// first channel); or empty to clear a typed or decoded stamp.
    pub start_tc: String,
}

/// Capture where a camera's rig stands as a preset to program a move to: a preset the camera
/// already has when the pose is within the project's range of one, otherwise a new one.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct CapturePreset {
    /// The camera. Omitted is the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// The name for a new preset. Omitted takes the camera's next free "Preset N".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// The preset a capture gave, and whether the camera already had it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Captured {
    pub preset: Preset,
    /// The pose was within range of this preset, which was there before. Nothing was stored.
    pub reused: bool,
    /// The pose the rig stood at.
    pub pose: Pose,
}

/// Store a camera's current position and zoom as the pose a new song starts from.
/// The whole pose is kept. Songs that already exist are not changed.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct StoreDefault {
    /// The camera. Omitted is the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
    /// Older clients send their last displayed pose. The server captures live hardware feedback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct StorePreset {
    pub name: String,
    /// Omitted stores the rig's current pose.
    #[serde(default)]
    pub pose: Option<Pose>,
    /// The camera the preset is for. Omitted is the first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
}

/// A change to a stored preset. With nothing given, the preset takes the pose its camera's rig stands at now.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct UpdatePreset {
    /// The whole pose to store.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pose: Option<Pose>,
    /// Only the column height, in millimetres; the rest of the pose stays as it is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elevation: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Refit {
    /// Multiplier on the default per-axis tolerance.
    pub scale: f64,
}

/// Smooth the loaded song's recorded moves to what each rig can do.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Smooth {
    /// Only this camera's recorded moves. Omitted is every camera's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
}

/// What smoothing did to the song.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct SmoothReport {
    /// Recorded moves that were looked at.
    pub moves: usize,
    /// Ids of the moves whose keys were smoothed.
    pub smoothed: Vec<String>,
    /// Ids of the moves still over a speed or acceleration limit, accepted as they are.
    pub accepted: Vec<String>,
    /// Issues that stopped Arm before.
    pub blocking_before: usize,
    /// Issues that still stop Arm. Range, jump and keep-out problems are never accepted here.
    pub left: Vec<Issue>,
    /// The furthest a smoothed move now is from what was recorded, on each axis that changed.
    pub departure: BTreeMap<Axis, f64>,
}

impl SmoothReport {
    /// The report in the operator's words.
    pub fn text(&self, units: Units) -> String {
        let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut said = Vec::new();
        if !self.smoothed.is_empty() {
            let far: Vec<String> =
                self.departure.iter().map(|(a, d)| format!("{} {}", a.label().to_lowercase(), a.format_in(*d, units))).collect();
            let moves = count(self.smoothed.len(), "recorded move", "recorded moves");
            said.push(format!("Smoothed {moves} to the rig's limits, by at most {}", far.join(" and ")));
        }
        if !self.accepted.is_empty() {
            said.push(format!("Accepted {} still over a limit", count(self.accepted.len(), "move", "moves")));
        }
        if said.is_empty() {
            said.push("No recorded move is over a speed or acceleration limit. Nothing was changed".into());
        }
        said.push(match self.left.len() {
            0 => "No issue stops Arm".into(),
            n => format!("{} left to fix", count(n, "issue is", "issues are")),
        });
        said.join(". ")
    }
}

/// The song after smoothing, and what was done to it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Smoothed {
    pub working: WorkingSong,
    pub report: SmoothReport,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ApiError {
    pub error: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct LogRecord {
    pub seq: u64,
    pub time: String,
    pub kind: String,
    pub data: serde_json::Value,
    pub prev: String,
    pub hash: String,
}

/// Messages pushed over the WebSocket.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsMsg {
    Status(Box<Status>),
    /// The working song changed on the server (another client, or a record pass).
    SongChanged {
        song_id: String,
        edit_counter: u64,
        reason: String,
    },
    SongListChanged,
    PresetsChanged,
    MediaChanged,
    ProjectChanged,
    Log {
        record: LogRecord,
    },
}
