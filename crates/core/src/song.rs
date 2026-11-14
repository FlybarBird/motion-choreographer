//! The song file: one frame timeline holding its SMPTE start, markers, media
//! clips and motion clips. Clip-local key frames, so moving a clip moves its keys.

use crate::pose::{Axis, Pose, Preset};
use crate::time::{FrameRate, SongClock, Timecode};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Song {
    pub id: String,
    pub name: String,
    pub start_tc: String,
    pub length_frames: i64,
    pub home_pose: Pose,
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub media: Vec<MediaClip>,
    #[serde(default)]
    pub motion: Vec<MotionClip>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Marker {
    pub id: String,
    pub frame: i64,
    pub name: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    Audio,
    Video,
}

/// A piece of imported media on the timeline. `in_frame..out_frame` is the
/// part of the source that plays; trimming never alters the file.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct MediaClip {
    pub id: String,
    pub media_id: String,
    pub track: Track,
    pub start_frame: i64,
    pub in_frame: i64,
    pub out_frame: i64,
    /// Length of the source in timeline frames, so trims can be clamped.
    pub media_frames: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub muted: bool,
}

impl MediaClip {
    pub fn duration(&self) -> i64 {
        self.out_frame - self.in_frame
    }

    pub fn end_frame(&self) -> i64 {
        self.start_frame + self.duration()
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum Ease {
    Linear,
    EaseIn,
    EaseOut,
    #[default]
    EaseInOut,
}

impl Ease {
    pub const ALL: [Ease; 4] = [Ease::Linear, Ease::EaseIn, Ease::EaseOut, Ease::EaseInOut];

    pub fn label(self) -> &'static str {
        match self {
            Ease::Linear => "Linear",
            Ease::EaseIn => "Ease in",
            Ease::EaseOut => "Ease out",
            Ease::EaseInOut => "Ease in-out",
        }
    }

    /// Cubic bezier control points `(x1, y1, x2, y2)` in normalised time and value.
    pub fn bezier(self) -> (f64, f64, f64, f64) {
        match self {
            Ease::Linear => (0.0, 0.0, 1.0, 1.0),
            Ease::EaseIn => (0.42, 0.0, 1.0, 1.0),
            Ease::EaseOut => (0.0, 0.0, 0.58, 1.0),
            Ease::EaseInOut => (0.42, 0.0, 0.58, 1.0),
        }
    }
}

/// A preset as it was when it was dropped. Clips copy the pose so committed
/// revisions never change when the preset cache does.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct PresetRef {
    pub preset_id: String,
    #[serde(default)]
    pub name: String,
    pub pose: Pose,
}

impl PresetRef {
    pub fn of(p: &Preset) -> PresetRef {
        PresetRef { preset_id: p.id.clone(), name: p.name.clone(), pose: p.pose }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum LinkedTag {
    Linked,
}

/// Where a preset move starts: wherever the timeline already is, or a pinned preset.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(untagged)]
pub enum FromPose {
    Linked(LinkedTag),
    Pinned(PresetRef),
}

impl FromPose {
    pub fn linked() -> FromPose {
        FromPose::Linked(LinkedTag::Linked)
    }

    pub fn is_linked(&self) -> bool {
        matches!(self, FromPose::Linked(_))
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum Interp {
    Hold,
    Linear,
    #[default]
    Bezier,
}

impl Interp {
    pub const ALL: [Interp; 3] = [Interp::Hold, Interp::Linear, Interp::Bezier];

    pub fn label(self) -> &'static str {
        match self {
            Interp::Hold => "Hold",
            Interp::Linear => "Linear",
            Interp::Bezier => "Bezier",
        }
    }
}

fn is_zero(h: &[f64; 2]) -> bool {
    h[0] == 0.0 && h[1] == 0.0
}

/// One key on one axis. `interp` shapes the segment that follows the key.
/// Handles are offsets `[frames, value]` from the key.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct Key {
    pub frame: i64,
    pub value: f64,
    #[serde(default)]
    pub interp: Interp,
    #[serde(rename = "in", default, skip_serializing_if = "is_zero")]
    pub in_handle: [f64; 2],
    #[serde(rename = "out", default, skip_serializing_if = "is_zero")]
    pub out_handle: [f64; 2],
}

impl Key {
    pub fn new(frame: i64, value: f64, interp: Interp) -> Key {
        Key { frame, value, interp, in_handle: [0.0, 0.0], out_handle: [0.0, 0.0] }
    }
}

/// Who accepted a too-fast move, and when. Stored in the song so it is part of
/// the committed revision.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Accepted {
    pub by: String,
    pub at: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ClipBody {
    PresetMove {
        auto_duration: bool,
        from: FromPose,
        to: PresetRef,
        #[serde(default)]
        ease: Ease,
    },
    Keys {
        #[serde(default)]
        curves: BTreeMap<Axis, Vec<Key>>,
    },
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct MotionClip {
    pub id: String,
    pub start_frame: i64,
    pub duration_frames: i64,
    pub axes: Vec<Axis>,
    #[serde(flatten)]
    pub body: ClipBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accepted: Option<Accepted>,
    /// Set on clips written by a record pass; names the raw sample file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_pass: Option<String>,
}

impl MotionClip {
    pub fn end_frame(&self) -> i64 {
        self.start_frame + self.duration_frames
    }

    pub fn has_axis(&self, axis: Axis) -> bool {
        self.axes.contains(&axis)
    }

    pub fn is_preset_move(&self) -> bool {
        matches!(self.body, ClipBody::PresetMove { .. })
    }

    pub fn is_keys(&self) -> bool {
        matches!(self.body, ClipBody::Keys { .. })
    }

    pub fn keys(&self, axis: Axis) -> &[Key] {
        match &self.body {
            ClipBody::Keys { curves } => curves.get(&axis).map(|v| v.as_slice()).unwrap_or(&[]),
            _ => &[],
        }
    }

    /// Short text for the clip bar.
    pub fn label(&self) -> String {
        match &self.body {
            ClipBody::PresetMove { from, to, .. } => match from {
                FromPose::Linked(_) => format!("to {}", to.name),
                FromPose::Pinned(p) => format!("{} to {}", p.name, to.name),
            },
            ClipBody::Keys { .. } => {
                if self.recorded_pass.is_some() {
                    "Recorded keys".into()
                } else {
                    "Keys".into()
                }
            }
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Invalid {
    DuplicateId(String),
    TooShort(String),
    OutsideSong(String),
    NoAxes(String),
    Overlap { a: String, b: String, axis: Axis },
    BadKeys { clip: String, axis: Axis },
    BadStart(String),
    BadMedia(String),
    MediaOverlap { a: String, b: String },
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Invalid::DuplicateId(id) => write!(f, "Two items share the id '{id}'"),
            Invalid::TooShort(_) => write!(f, "A clip must be at least one frame long"),
            Invalid::OutsideSong(_) => write!(f, "The clip would fall outside the song"),
            Invalid::NoAxes(_) => write!(f, "A move must drive at least one axis"),
            Invalid::Overlap { axis, .. } => {
                write!(f, "Overlaps another move on {}", axis.label().to_lowercase())
            }
            Invalid::BadKeys { axis, .. } => {
                write!(f, "Keys on {} are out of order or outside the clip", axis.label().to_lowercase())
            }
            Invalid::BadStart(s) => write!(f, "'{s}' is not a valid start timecode"),
            Invalid::BadMedia(_) => write!(f, "The media clip trim is outside its source"),
            Invalid::MediaOverlap { .. } => write!(f, "Overlaps another clip on that track"),
        }
    }
}

impl Song {
    pub fn new(id: &str, name: &str, start_tc: &str, length_frames: i64) -> Song {
        Song {
            id: id.into(),
            name: name.into(),
            start_tc: start_tc.into(),
            length_frames,
            home_pose: Pose::home(),
            markers: Vec::new(),
            media: Vec::new(),
            motion: Vec::new(),
        }
    }

    pub fn start(&self, rate: FrameRate) -> Timecode {
        Timecode::parse_at(&self.start_tc, rate).unwrap_or(Timecode::from_frames(0, rate))
    }

    pub fn clock(&self, rate: FrameRate) -> SongClock {
        SongClock::new(rate, self.start(rate), self.length_frames)
    }

    pub fn motion_clip(&self, id: &str) -> Option<&MotionClip> {
        self.motion.iter().find(|c| c.id == id)
    }

    pub fn motion_clip_mut(&mut self, id: &str) -> Option<&mut MotionClip> {
        self.motion.iter_mut().find(|c| c.id == id)
    }

    pub fn media_clip(&self, id: &str) -> Option<&MediaClip> {
        self.media.iter().find(|c| c.id == id)
    }

    pub fn media_clip_mut(&mut self, id: &str) -> Option<&mut MediaClip> {
        self.media.iter_mut().find(|c| c.id == id)
    }

    /// A fresh id of the form `<prefix><n>` unused by any clip or marker.
    pub fn next_id(&self, prefix: &str) -> String {
        let used: HashSet<&str> = self
            .motion
            .iter()
            .map(|c| c.id.as_str())
            .chain(self.media.iter().map(|c| c.id.as_str()))
            .chain(self.markers.iter().map(|m| m.id.as_str()))
            .collect();
        let mut n = self.motion.len() + self.media.len() + self.markers.len() + 1;
        loop {
            let id = format!("{prefix}{n}");
            if !used.contains(id.as_str()) {
                return id;
            }
            n += 1;
        }
    }

    /// The motion clip on `axis` that covers `frame`, if any.
    pub fn clip_on_axis_at(&self, axis: Axis, frame: i64) -> Option<&MotionClip> {
        self.motion
            .iter()
            .find(|c| c.has_axis(axis) && c.start_frame <= frame && frame < c.end_frame())
    }

    /// End of the last clip or marker; the song can never be shorter than this.
    pub fn content_end(&self) -> i64 {
        let m = self.motion.iter().map(|c| c.end_frame()).max().unwrap_or(0);
        let a = self.media.iter().map(|c| c.end_frame()).max().unwrap_or(0);
        let k = self.markers.iter().map(|c| c.frame).max().unwrap_or(0);
        m.max(a).max(k)
    }

    pub fn validate(&self, rate: FrameRate) -> Result<(), Invalid> {
        if Timecode::parse_at(&self.start_tc, rate).is_none() {
            return Err(Invalid::BadStart(self.start_tc.clone()));
        }
        let mut ids = HashSet::new();
        for id in self
            .motion
            .iter()
            .map(|c| &c.id)
            .chain(self.media.iter().map(|c| &c.id))
            .chain(self.markers.iter().map(|m| &m.id))
        {
            if !ids.insert(id.as_str()) {
                return Err(Invalid::DuplicateId(id.clone()));
            }
        }
        for m in &self.markers {
            if m.frame < 0 || m.frame > self.length_frames {
                return Err(Invalid::OutsideSong(m.id.clone()));
            }
        }
        for c in &self.media {
            if c.duration() < 1 {
                return Err(Invalid::TooShort(c.id.clone()));
            }
            if c.in_frame < 0 || c.out_frame > c.media_frames {
                return Err(Invalid::BadMedia(c.id.clone()));
            }
            if c.start_frame < 0 || c.end_frame() > self.length_frames {
                return Err(Invalid::OutsideSong(c.id.clone()));
            }
        }
        for (i, a) in self.media.iter().enumerate() {
            for b in &self.media[i + 1..] {
                if a.track == b.track && a.start_frame < b.end_frame() && b.start_frame < a.end_frame() {
                    return Err(Invalid::MediaOverlap { a: a.id.clone(), b: b.id.clone() });
                }
            }
        }
        for c in &self.motion {
            if c.duration_frames < 1 {
                return Err(Invalid::TooShort(c.id.clone()));
            }
            if c.start_frame < 0 || c.end_frame() > self.length_frames {
                return Err(Invalid::OutsideSong(c.id.clone()));
            }
            if c.axes.is_empty() {
                return Err(Invalid::NoAxes(c.id.clone()));
            }
            let mut seen = HashSet::new();
            if !c.axes.iter().all(|a| seen.insert(*a)) {
                return Err(Invalid::NoAxes(c.id.clone()));
            }
            if let ClipBody::Keys { curves } = &c.body {
                for (axis, keys) in curves {
                    let ordered = keys.windows(2).all(|w| w[0].frame < w[1].frame);
                    let inside = keys.iter().all(|k| k.frame >= 0 && k.frame <= c.duration_frames);
                    let finite = keys.iter().all(|k| {
                        k.value.is_finite()
                            && k.in_handle.iter().chain(k.out_handle.iter()).all(|v| v.is_finite())
                    });
                    if !ordered || !inside || !finite || !c.has_axis(*axis) {
                        return Err(Invalid::BadKeys { clip: c.id.clone(), axis: *axis });
                    }
                }
            }
        }
        for (i, a) in self.motion.iter().enumerate() {
            for b in &self.motion[i + 1..] {
                if a.start_frame < b.end_frame() && b.start_frame < a.end_frame() {
                    if let Some(axis) = a.axes.iter().find(|x| b.has_axis(**x)) {
                        return Err(Invalid::Overlap { a: a.id.clone(), b: b.id.clone(), axis: *axis });
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "id": "walk-in",
      "name": "Walk-In",
      "start_tc": "01:00:00:00",
      "length_frames": 4500,
      "home_pose": { "pan": 0, "tilt": 0, "zoom": 0, "focus": 0.5, "elevation": 0 },
      "markers": [{ "id": "k1", "frame": 250, "name": "Doors" }],
      "media": [
        { "id": "c1", "media_id": "m-click", "track": "audio", "start_frame": 0, "in_frame": 0, "out_frame": 310, "media_frames": 310 }
      ],
      "motion": [
        { "id": "c2", "kind": "preset_move", "start_frame": 100, "duration_frames": 96,
          "auto_duration": true, "from": "linked",
          "to": { "preset_id": "lectern-mcu", "pose": { "pan": -12, "tilt": 3, "zoom": 0.4, "focus": 0.5, "elevation": 300 } },
          "ease": "ease_in_out", "axes": ["pan", "tilt", "zoom", "focus", "elevation"] },
        { "id": "c3", "kind": "keys", "start_frame": 400, "duration_frames": 200, "axes": ["pan"],
          "curves": { "pan": [
            { "frame": 0, "value": -12, "interp": "bezier", "out": [8, 0] },
            { "frame": 200, "value": 20, "interp": "bezier", "in": [-8, 0] }
          ] } }
      ]
    }"#;

    #[test]
    fn plan_document_parses_and_round_trips() {
        let song: Song = serde_json::from_str(SAMPLE).unwrap();
        assert!(song.validate(FrameRate::F25).is_ok());
        assert!(song.motion[0].is_preset_move());
        assert_eq!(song.motion[1].keys(Axis::Pan).len(), 2);
        assert_eq!(song.motion[1].keys(Axis::Pan)[0].out_handle, [8.0, 0.0]);
        let text = serde_json::to_string(&song).unwrap();
        let again: Song = serde_json::from_str(&text).unwrap();
        assert_eq!(song, again);
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["motion"][0]["from"], "linked");
        assert_eq!(v["motion"][0]["kind"], "preset_move");
    }

    #[test]
    fn overlap_on_a_shared_axis_is_invalid() {
        let mut song: Song = serde_json::from_str(SAMPLE).unwrap();
        song.motion[1].start_frame = 150;
        assert!(matches!(song.validate(FrameRate::F25), Err(Invalid::Overlap { axis: Axis::Pan, .. })));
        song.motion[0].axes = vec![Axis::Zoom];
        assert!(song.validate(FrameRate::F25).is_ok());
    }

    #[test]
    fn clips_must_be_inside_the_song_and_one_frame_long() {
        let mut song: Song = serde_json::from_str(SAMPLE).unwrap();
        song.motion[0].duration_frames = 0;
        assert!(matches!(song.validate(FrameRate::F25), Err(Invalid::TooShort(_))));
        song.motion[0].duration_frames = 96;
        song.motion[1].start_frame = 4400;
        assert!(matches!(song.validate(FrameRate::F25), Err(Invalid::OutsideSong(_))));
    }
}
