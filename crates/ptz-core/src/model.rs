//! Project / song / move domain models and interpolation.

use crate::easing::{ease, EaseKind};
use crate::timecode::{FrameRate, Timecode};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockSource {
    Internal,
    Ltc,
    Mtc,
    ArtNet,
}

impl ClockSource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Internal => "Internal",
            Self::Ltc => "LTC",
            Self::Mtc => "MTC (quarter frames)",
            Self::ArtNet => "Art-Net Timecode",
        }
    }

    pub fn all() -> &'static [ClockSource] {
        &[Self::Internal, Self::Ltc, Self::Mtc, Self::ArtNet]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioChannelRoute {
    Silent,
    Program,
    LtcGenerator,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioRouting {
    pub left: AudioChannelRoute,
    pub right: AudioChannelRoute,
    /// LTC generator initially off.
    pub ltc_generator_enabled: bool,
}

impl Default for AudioRouting {
    fn default() -> Self {
        Self {
            left: AudioChannelRoute::Program,
            right: AudioChannelRoute::LtcGenerator,
            ltc_generator_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSettings {
    pub frame_rate: FrameRate,
    pub clock_source: ClockSource,
    pub audio_routing: AudioRouting,
    pub backend_url: String,
}

impl Default for ProjectSettings {
    fn default() -> Self {
        Self {
            frame_rate: FrameRate::Fps25,
            clock_source: ClockSource::Internal,
            audio_routing: AudioRouting::default(),
            backend_url: crate::DEFAULT_BACKEND_URL.to_string(),
        }
    }
}

/// Absolute rig pose: FR7 PTZ + AutoPod pedestal.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RigPose {
    /// Pan degrees, -170..170 typical.
    pub pan_deg: f64,
    /// Tilt degrees, -30..90 typical.
    pub tilt_deg: f64,
    /// Zoom normalized 0..1 (wide→tele).
    pub zoom: f64,
    /// Focus normalized 0..1.
    pub focus: f64,
    /// Pedestal elevation meters.
    pub elevation_m: f64,
}

impl Default for RigPose {
    fn default() -> Self {
        Self {
            pan_deg: 0.0,
            tilt_deg: 0.0,
            zoom: 0.35,
            focus: 0.5,
            elevation_m: 1.4,
        }
    }
}

impl RigPose {
    pub fn lerp(self, other: Self, t: f64) -> Self {
        let t = t.clamp(0.0, 1.0);
        Self {
            pan_deg: self.pan_deg + (other.pan_deg - self.pan_deg) * t,
            tilt_deg: self.tilt_deg + (other.tilt_deg - self.tilt_deg) * t,
            zoom: self.zoom + (other.zoom - self.zoom) * t,
            focus: self.focus + (other.focus - self.focus) * t,
            elevation_m: self.elevation_m + (other.elevation_m - self.elevation_m) * t,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Pan,
    Tilt,
    Zoom,
    Focus,
    Elevation,
}

impl Axis {
    pub fn all() -> &'static [Axis] {
        &[
            Self::Pan,
            Self::Tilt,
            Self::Zoom,
            Self::Focus,
            Self::Elevation,
        ]
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pan => "Pan",
            Self::Tilt => "Tilt",
            Self::Zoom => "Zoom",
            Self::Focus => "Focus",
            Self::Elevation => "Elevation",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisSelection {
    pub pan: bool,
    pub tilt: bool,
    pub zoom: bool,
    pub focus: bool,
    pub elevation: bool,
}

impl Default for AxisSelection {
    fn default() -> Self {
        Self {
            pan: true,
            tilt: true,
            zoom: true,
            focus: false,
            elevation: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Normalized 0..1 within the move.
    pub t: f64,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisCurve {
    pub axis: Axis,
    pub keyframes: Vec<Keyframe>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RigPreset {
    pub id: String,
    pub name: String,
    pub pose: RigPose,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveClip {
    pub id: String,
    pub name: String,
    pub preset_id: Option<String>,
    /// Start frame relative to song timeline (0 = song start).
    pub start_frame: i64,
    pub duration_frames: i64,
    pub easing: EaseKind,
    pub axes: AxisSelection,
    /// Target pose (usually from preset).
    pub target: RigPose,
    /// Optional per-axis curves; if empty, ease from previous pose → target.
    pub curves: Vec<AxisCurve>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub id: String,
    pub name: String,
    pub frame: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioClipMeta {
    pub asset_id: String,
    pub filename: String,
    pub duration_secs: f64,
    /// Peak samples for timeline drawing, normalized −1..1 abs peaks.
    pub waveform: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Song {
    pub id: String,
    pub name: String,
    pub revision: u64,
    /// Signed SMPTE start offset for the song.
    pub smpte_start: Timecode,
    pub duration_frames: i64,
    pub markers: Vec<Marker>,
    pub moves: Vec<MoveClip>,
    pub audio: Option<AudioClipMeta>,
    pub committed: bool,
    pub dirty: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveState {
    pub pose: RigPose,
    pub playhead_frames: i64,
    pub playhead_tc: String,
    pub song_seconds: f64,
}

/// Evaluate rig pose at an absolute song frame.
pub fn interpolate_pose(
    moves: &[MoveClip],
    frame: i64,
    default_pose: RigPose,
) -> RigPose {
    let mut pose = default_pose;
    let mut sorted = moves.to_vec();
    sorted.sort_by_key(|m| m.start_frame);

    for mv in &sorted {
        let end = mv.start_frame + mv.duration_frames;
        if frame < mv.start_frame {
            break;
        }
        if frame >= end {
            // Apply completed move as new baseline for selected axes.
            pose = apply_axes(pose, mv.target, &mv.axes, 1.0);
            continue;
        }
        let dur = mv.duration_frames.max(1) as f64;
        let local_t = (frame - mv.start_frame) as f64 / dur;
        let eased = ease(mv.easing, local_t);
        if mv.curves.is_empty() {
            pose = apply_axes(pose, mv.target, &mv.axes, eased);
        } else {
            pose = apply_curves(pose, mv, eased);
        }
    }
    pose
}

fn apply_axes(from: RigPose, to: RigPose, axes: &AxisSelection, t: f64) -> RigPose {
    let mut out = from;
    if axes.pan {
        out.pan_deg = from.pan_deg + (to.pan_deg - from.pan_deg) * t;
    }
    if axes.tilt {
        out.tilt_deg = from.tilt_deg + (to.tilt_deg - from.tilt_deg) * t;
    }
    if axes.zoom {
        out.zoom = from.zoom + (to.zoom - from.zoom) * t;
    }
    if axes.focus {
        out.focus = from.focus + (to.focus - from.focus) * t;
    }
    if axes.elevation {
        out.elevation_m = from.elevation_m + (to.elevation_m - from.elevation_m) * t;
    }
    out
}

fn apply_curves(from: RigPose, mv: &MoveClip, _eased: f64) -> RigPose {
    let mut out = from;
    for curve in &mv.curves {
        let v = sample_curve(curve, /* use raw local via keyframes */ {
            // Recompute local t from first/last — caller passes eased already baked into
            // keyframe sampling by using linear keyframe t then ease externally.
            // Here we sample by linear local t recovered from eased? Prefer raw.
            // For simplicity sample using keyframe t against eased progress.
            _eased
        });
        match curve.axis {
            Axis::Pan if mv.axes.pan => out.pan_deg = v,
            Axis::Tilt if mv.axes.tilt => out.tilt_deg = v,
            Axis::Zoom if mv.axes.zoom => out.zoom = v,
            Axis::Focus if mv.axes.focus => out.focus = v,
            Axis::Elevation if mv.axes.elevation => out.elevation_m = v,
            _ => {}
        }
    }
    // Axes without curves fall back to target lerp
    let with_target = apply_axes(from, mv.target, &mv.axes, _eased);
    if !mv.curves.iter().any(|c| c.axis == Axis::Pan) && mv.axes.pan {
        out.pan_deg = with_target.pan_deg;
    }
    if !mv.curves.iter().any(|c| c.axis == Axis::Tilt) && mv.axes.tilt {
        out.tilt_deg = with_target.tilt_deg;
    }
    if !mv.curves.iter().any(|c| c.axis == Axis::Zoom) && mv.axes.zoom {
        out.zoom = with_target.zoom;
    }
    if !mv.curves.iter().any(|c| c.axis == Axis::Focus) && mv.axes.focus {
        out.focus = with_target.focus;
    }
    if !mv.curves.iter().any(|c| c.axis == Axis::Elevation) && mv.axes.elevation {
        out.elevation_m = with_target.elevation_m;
    }
    out
}

fn sample_curve(curve: &AxisCurve, t: f64) -> f64 {
    if curve.keyframes.is_empty() {
        return 0.0;
    }
    let mut kfs = curve.keyframes.clone();
    kfs.sort_by(|a, b| a.t.partial_cmp(&b.t).unwrap());
    if t <= kfs[0].t {
        return kfs[0].value;
    }
    if t >= kfs[kfs.len() - 1].t {
        return kfs[kfs.len() - 1].value;
    }
    for w in kfs.windows(2) {
        if t >= w[0].t && t <= w[1].t {
            let span = (w[1].t - w[0].t).max(1e-9);
            let u = (t - w[0].t) / span;
            return w[0].value + (w[1].value - w[0].value) * u;
        }
    }
    kfs[kfs.len() - 1].value
}

pub fn demo_presets() -> Vec<RigPreset> {
    vec![
        RigPreset {
            id: "preset-stage-wide".into(),
            name: "Stage Wide".into(),
            pose: RigPose {
                pan_deg: 0.0,
                tilt_deg: -8.0,
                zoom: 0.18,
                focus: 0.45,
                elevation_m: 1.6,
            },
        },
        RigPreset {
            id: "preset-lectern-mcu".into(),
            name: "Lectern MCU".into(),
            pose: RigPose {
                pan_deg: -12.0,
                tilt_deg: -2.0,
                zoom: 0.62,
                focus: 0.55,
                elevation_m: 1.45,
            },
        },
        RigPreset {
            id: "preset-awards-two-shot".into(),
            name: "Awards Two-Shot".into(),
            pose: RigPose {
                pan_deg: 18.0,
                tilt_deg: -5.0,
                zoom: 0.4,
                focus: 0.5,
                elevation_m: 1.5,
            },
        },
    ]
}

pub fn demo_songs(rate: FrameRate) -> Vec<Song> {
    let presets = demo_presets();
    let walk_in_start = Timecode::parse("01:00:00:00", rate).expect("tc");
    let walk_in = Song {
        id: "song-walk-in".into(),
        name: "Walk-In".into(),
        revision: 1,
        smpte_start: walk_in_start,
        duration_frames: rate.seconds_to_frames(90.0),
        markers: vec![
            Marker {
                id: Uuid::new_v4().to_string(),
                name: "Doors".into(),
                frame: 0,
            },
            Marker {
                id: Uuid::new_v4().to_string(),
                name: "House to half".into(),
                frame: rate.seconds_to_frames(45.0),
            },
        ],
        moves: vec![
            MoveClip {
                id: Uuid::new_v4().to_string(),
                name: "Open wide".into(),
                preset_id: Some(presets[0].id.clone()),
                start_frame: 0,
                duration_frames: rate.seconds_to_frames(8.0),
                easing: EaseKind::EaseInOut,
                axes: AxisSelection::default(),
                target: presets[0].pose,
                curves: vec![],
            },
            MoveClip {
                id: Uuid::new_v4().to_string(),
                name: "Push lectern".into(),
                preset_id: Some(presets[1].id.clone()),
                start_frame: rate.seconds_to_frames(20.0),
                duration_frames: rate.seconds_to_frames(12.0),
                easing: EaseKind::Smooth,
                axes: AxisSelection::default(),
                target: presets[1].pose,
                curves: vec![],
            },
        ],
        audio: None,
        committed: true,
        dirty: false,
    };

    let opening = Song {
        id: "song-opening".into(),
        name: "Opening Number".into(),
        revision: 1,
        smpte_start: Timecode::parse("01:05:00:00", rate).unwrap(),
        duration_frames: rate.seconds_to_frames(180.0),
        markers: vec![Marker {
            id: Uuid::new_v4().to_string(),
            name: "Downbeat".into(),
            frame: 0,
        }],
        moves: vec![MoveClip {
            id: Uuid::new_v4().to_string(),
            name: "Sweep awards".into(),
            preset_id: Some(presets[2].id.clone()),
            start_frame: rate.seconds_to_frames(5.0),
            duration_frames: rate.seconds_to_frames(16.0),
            easing: EaseKind::EaseInOut,
            axes: AxisSelection::default(),
            target: presets[2].pose,
            curves: vec![],
        }],
        audio: None,
        committed: true,
        dirty: false,
    };

    let awards = Song {
        id: "song-awards-bed".into(),
        name: "Awards Bed".into(),
        revision: 1,
        smpte_start: Timecode::parse("01:20:00:00", rate).unwrap(),
        duration_frames: rate.seconds_to_frames(240.0),
        markers: vec![],
        moves: vec![
            MoveClip {
                id: Uuid::new_v4().to_string(),
                name: "Two-shot hold".into(),
                preset_id: Some(presets[2].id.clone()),
                start_frame: 0,
                duration_frames: rate.seconds_to_frames(6.0),
                easing: EaseKind::Linear,
                axes: AxisSelection::default(),
                target: presets[2].pose,
                curves: vec![],
            },
            MoveClip {
                id: Uuid::new_v4().to_string(),
                name: "Back to wide".into(),
                preset_id: Some(presets[0].id.clone()),
                start_frame: rate.seconds_to_frames(30.0),
                duration_frames: rate.seconds_to_frames(10.0),
                easing: EaseKind::EaseOut,
                axes: AxisSelection::default(),
                target: presets[0].pose,
                curves: vec![],
            },
        ],
        audio: None,
        committed: true,
        dirty: false,
    };

    vec![walk_in, opening, awards]
}

/// Audio output / sync snapshot for headless Mac verification (`GET /api/v1/status`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioOutputStatus {
    /// Device name, or `"silent"`.
    pub device: String,
    /// Present when silent / failed to open (or informational).
    pub reason: Option<String>,
    /// Program (clip) sample rate, or output rate when no clip loaded.
    pub sample_rate: u32,
    /// Loaded clip duration in milliseconds, if any.
    pub duration_ms: Option<u64>,
    /// Current audio engine position in milliseconds.
    pub position_ms: u64,
    /// Transport / show-clock position in milliseconds.
    pub transport_position_ms: u64,
    /// `position_ms - transport_position_ms`.
    pub drift_ms: i64,
    /// Mirrors `transport.hardware_live` for a one-shot curl check.
    pub hardware_live: bool,
}

/// API / websocket envelope pieces shared by client & server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShowStatus {
    pub api_version: String,
    pub transport: crate::transport::TransportSnapshot,
    pub guidance: crate::transport::TransportGuidance,
    pub settings: ProjectSettings,
    pub active_song_id: Option<String>,
    pub live: LiveState,
    pub audio_output: AudioOutputStatus,
    pub server_time_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Status(ShowStatus),
    SongUpdated { song: Song },
    Log { level: String, message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::easing::EaseKind;

    #[test]
    fn interpolate_reaches_target() {
        let mv = MoveClip {
            id: "m".into(),
            name: "m".into(),
            preset_id: None,
            start_frame: 0,
            duration_frames: 100,
            easing: EaseKind::Linear,
            axes: AxisSelection::default(),
            target: RigPose {
                pan_deg: 40.0,
                tilt_deg: 10.0,
                zoom: 0.8,
                focus: 0.5,
                elevation_m: 2.0,
            },
            curves: vec![],
        };
        let start = interpolate_pose(&[mv.clone()], 0, RigPose::default());
        assert!((start.pan_deg - 0.0).abs() < 1e-9);
        let end = interpolate_pose(&[mv], 100, RigPose::default());
        assert!((end.pan_deg - 40.0).abs() < 1e-9);
        assert!((end.elevation_m - 2.0).abs() < 1e-9);
    }

    #[test]
    fn demo_songs_seeded() {
        let songs = demo_songs(FrameRate::Fps25);
        assert_eq!(songs.len(), 3);
        assert_eq!(songs[0].name, "Walk-In");
        assert_eq!(songs[0].smpte_start.display(), "01:00:00:00");
    }
}
