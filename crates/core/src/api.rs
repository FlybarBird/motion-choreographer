//! Wire types shared by the server, the app and the probe.

use crate::chase::ChaseState;
use crate::compile::Issue;
use crate::pose::{Axis, Pose, Routing};
use crate::song::Song;
use crate::time::FrameRate;
use crate::transport::{Guidance, TransportState};
use serde::{Deserialize, Serialize};

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

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct AdapterStatus {
    pub name: String,
    pub connected: bool,
    pub detail: String,
    /// AutoPod only.
    pub calibrated: Option<bool>,
    pub round_trip_ms: Option<f64>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct RecordStatus {
    /// Axes that Record will write.
    pub axes: Vec<Axis>,
    /// A pass is being captured now.
    pub active: bool,
    pub frames: i64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
pub struct Telemetry {
    /// Rig position minus target per axis while live.
    pub error: Pose,
    pub fault: Option<String>,
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
    pub pose: Pose,
    pub pose_source: PoseSource,
    pub rig_pose: Pose,
    pub hardware_live: bool,
    pub lease: Option<LeaseStatus>,
    pub you_hold_lease: bool,
    pub guidance: Guidance,
    pub audio: AudioStatus,
    pub lock: LockStatus,
    pub adapters: Vec<AdapterStatus>,
    pub record: RecordStatus,
    pub telemetry: Telemetry,
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
    /// Channel carrying LTC, excluded from Program.
    pub ltc_channel: Option<u32>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct StoreTc {
    /// Typed start timecode, or `ltc:<channel>` to decode it from a channel.
    pub start_tc: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct StorePreset {
    pub name: String,
    /// Omitted stores the rig's current pose.
    #[serde(default)]
    pub pose: Option<Pose>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Refit {
    /// Multiplier on the default per-axis tolerance.
    pub scale: f64,
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
