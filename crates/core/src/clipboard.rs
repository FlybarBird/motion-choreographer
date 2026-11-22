//! Portable timeline snapshots. Copy never changes the source; paste validates a complete
//! candidate before returning it, so a refused paste cannot leave half a selection behind.

use crate::pose::{Axis, Project};
use crate::song::{ClipBody, FromPose, Invalid, Key, Marker, MediaClip, MotionClip, Song};
use crate::time::FrameRate;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fmt;

pub const VERSION: u32 = 1;
pub const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_ITEMS: usize = 4096;
pub const MAX_KEYS: usize = 100_000;
/// The same two-hour range offered by the song Length control. Compilation stores
/// every frame, so even an otherwise valid huge integer cannot be allowed to grow it.
pub const MAX_SECONDS: i64 = 7200;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
pub struct Clipboard {
    pub version: u32,
    pub rate: FrameRate,
    pub contents: Contents,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Contents {
    /// Starts and marker frames are relative to the earliest selected item. Key frames
    /// remain local to their motion clip; media trims remain local to their source file.
    Timeline { motion: Vec<MotionClip>, media: Vec<MediaClip>, markers: Vec<Marker> },
    /// Keys from one clip, with their earliest selected frame at zero.
    Keys { camera: String, curves: BTreeMap<Axis, Vec<Key>> },
}

#[derive(Clone, PartialEq, Debug)]
pub enum Error {
    Empty,
    NotFound,
    NotKeys,
    WrongKind,
    UnknownVersion(u32),
    BadFormat,
    TooLarge,
    FrameRateMismatch { copied: FrameRate, destination: FrameRate },
    Invalid(Invalid),
    InvalidData(&'static str),
    NoSuchCamera(String),
    OtherCamera,
    OutsideClip,
    KeyCollision { axis: Axis, frame: i64 },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "Nothing is selected to copy"),
            Self::NotFound => write!(f, "A selected item is no longer on the timeline"),
            Self::NotKeys => write!(f, "Only a keys clip can receive individual keys"),
            Self::WrongKind => write!(f, "The clipboard contains clips, not individual keys"),
            Self::UnknownVersion(version) => write!(f, "Clipboard version {version} is not supported"),
            Self::BadFormat => write!(f, "The clipboard does not contain a Motion Choreographer selection"),
            Self::TooLarge => write!(f, "The copied selection is too large; copy fewer clips or keys"),
            Self::FrameRateMismatch { copied, destination } => write!(
                f,
                "The selection was copied at {} fps; this show uses {} fps. Paste into a show with the same frame rate",
                copied.label(),
                destination.label()
            ),
            Self::Invalid(why) => write!(f, "{why}"),
            Self::InvalidData(why) => write!(f, "The clipboard cannot be pasted: {why}"),
            Self::NoSuchCamera(id) => write!(f, "The copied camera '{id}' is not in this project"),
            Self::OtherCamera => write!(f, "The copied keys belong to another camera"),
            Self::OutsideClip => write!(f, "The copied keys do not fit inside the selected clip"),
            Self::KeyCollision { axis, frame } => {
                write!(f, "A {} key already exists at clip frame {frame}", axis.label().to_lowercase())
            }
        }
    }
}

impl std::error::Error for Error {}

impl Clipboard {
    pub fn from_json(text: &str) -> Result<Self, Error> {
        if text.len() > MAX_JSON_BYTES {
            return Err(Error::TooLarge);
        }
        let snapshot: Self = serde_json::from_str(text).map_err(|_| Error::BadFormat)?;
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn to_json(&self) -> Result<String, Error> {
        self.validate()?;
        let mut output = BoundedJson { bytes: Vec::new(), too_large: false };
        if serde_json::to_writer(&mut output, self).is_err() {
            return Err(if output.too_large { Error::TooLarge } else { Error::BadFormat });
        }
        String::from_utf8(output.bytes).map_err(|_| Error::BadFormat)
    }

    pub fn is_keys(&self) -> bool {
        matches!(self.contents, Contents::Keys { .. })
    }

    pub fn item_count(&self) -> usize {
        match &self.contents {
            Contents::Timeline { motion, media, markers } => motion.len() + media.len() + markers.len(),
            Contents::Keys { curves, .. } => curves.values().map(Vec::len).sum(),
        }
    }

    /// References only: copying media does not copy or import its underlying file.
    pub fn media_ids(&self) -> Vec<&str> {
        let mut ids = Vec::new();
        if let Contents::Timeline { media, .. } = &self.contents {
            for clip in media {
                if !ids.contains(&clip.media_id.as_str()) {
                    ids.push(clip.media_id.as_str());
                }
            }
        }
        ids
    }

    /// Validate a decoded snapshot without serializing it again. A transport envelope
    /// should separately cap its encoded byte length before deserializing this value.
    pub fn validate(&self) -> Result<(), Error> {
        if self.version != VERSION {
            return Err(Error::UnknownVersion(self.version));
        }
        let (motion, media, markers) = match &self.contents {
            Contents::Timeline { motion, media, markers } => {
                check_sizes(motion, media.len(), markers.len())?;
                (motion.clone(), media.clone(), markers.clone())
            }
            Contents::Keys { camera, curves } => {
                if curves.values().map(Vec::len).sum::<usize>() > MAX_KEYS {
                    return Err(Error::TooLarge);
                }
                if curves.values().flatten().map(|key| key.frame).min() != Some(0) {
                    return Err(Error::InvalidData("the copied keys do not start at their origin"));
                }
                (vec![keys_clip("keys", camera, 0, curves.clone())?], vec![], vec![])
            }
        };
        let mut snapshot = Song::new("clipboard", "Clipboard", "00:00:00:00", 1);
        snapshot.motion = motion;
        snapshot.media = media;
        snapshot.markers = markers;
        if snapshot.motion.is_empty() && snapshot.media.is_empty() && snapshot.markers.is_empty() {
            return Err(Error::Empty);
        }
        let end = checked_end(&snapshot)?;
        if end > MAX_SECONDS * self.rate.nominal() {
            return Err(Error::InvalidData("the selection spans more than two hours"));
        }
        snapshot.length_frames = end.max(1);
        snapshot.validate(self.rate).map_err(Error::Invalid)?;
        let origin = snapshot
            .motion
            .iter()
            .map(|clip| clip.start_frame)
            .chain(snapshot.media.iter().map(|clip| clip.start_frame))
            .chain(snapshot.markers.iter().map(|marker| marker.frame))
            .min();
        if origin != Some(0) {
            return Err(Error::InvalidData("the selection has no item at its origin"));
        }
        Ok(())
    }
}

struct BoundedJson {
    bytes: Vec<u8>,
    too_large: bool,
}

impl std::io::Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.bytes.len().saturating_add(bytes.len()) > MAX_JSON_BYTES {
            self.too_large = true;
            return Err(std::io::Error::other("clipboard size limit"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn check_sizes(motion: &[MotionClip], media: usize, markers: usize) -> Result<(), Error> {
    if motion.len().saturating_add(media).saturating_add(markers) > MAX_ITEMS {
        return Err(Error::TooLarge);
    }
    let count = motion
        .iter()
        .filter_map(|clip| match &clip.body {
            ClipBody::Keys { curves } => Some(curves.values().map(Vec::len).sum::<usize>()),
            _ => None,
        })
        .fold(0usize, usize::saturating_add);
    if count > MAX_KEYS {
        Err(Error::TooLarge)
    } else {
        Ok(())
    }
}

/// Check arithmetic before calling Song::validate, whose ordinary timeline arithmetic
/// assumes bounded, nonnegative frames. An OS clipboard is not a trusted song file.
fn checked_end(song: &Song) -> Result<i64, Error> {
    if song.length_frames < 1 {
        return Err(Error::InvalidData("the song length is invalid"));
    }
    let mut end = 0;
    for clip in &song.motion {
        if clip.start_frame < 0 || clip.duration_frames < 1 {
            return Err(Error::InvalidData("motion frames must be nonnegative with a positive duration"));
        }
        let clip_end = clip.start_frame.checked_add(clip.duration_frames).ok_or(Error::InvalidData("frame numbers overflow"))?;
        end = end.max(clip_end);
        let finite = match &clip.body {
            ClipBody::PresetMove { from, to, .. } => {
                to.pose.is_finite() && !matches!(from, FromPose::Pinned(preset) if !preset.pose.is_finite())
            }
            ClipBody::AutoFollow { preset: Some(preset), .. } => preset.pose.is_finite(),
            _ => true,
        };
        if !finite {
            return Err(Error::InvalidData("a preset contains a non-finite value"));
        }
    }
    for clip in &song.media {
        if clip.start_frame < 0 || clip.in_frame < 0 || clip.out_frame <= clip.in_frame || clip.out_frame > clip.media_frames {
            return Err(Error::InvalidData("a media trim is outside its source"));
        }
        if clip.media_id.is_empty() {
            return Err(Error::InvalidData("a media clip names no source file"));
        }
        let duration = clip.out_frame.checked_sub(clip.in_frame).ok_or(Error::InvalidData("media frames overflow"))?;
        end = end.max(clip.start_frame.checked_add(duration).ok_or(Error::InvalidData("frame numbers overflow"))?);
    }
    for marker in &song.markers {
        if marker.frame < 0 {
            return Err(Error::InvalidData("a marker has a negative frame"));
        }
        end = end.max(marker.frame);
    }
    Ok(end)
}

fn valid_song(song: &Song, rate: FrameRate) -> Result<(), Error> {
    checked_end(song)?;
    song.validate(rate).map_err(Error::Invalid)
}

/// Capture whole selected clips and markers, including all keys inside selected motion clips.
/// Missing IDs are refused rather than silently copying a partial selection.
pub fn capture(song: &Song, rate: FrameRate, ids: &[String]) -> Result<Clipboard, Error> {
    if ids.is_empty() {
        return Err(Error::Empty);
    }
    if ids.len() > MAX_ITEMS {
        return Err(Error::TooLarge);
    }
    valid_song(song, rate)?;
    let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
    let mut motion: Vec<_> = song.motion.iter().filter(|clip| wanted.contains(clip.id.as_str())).cloned().collect();
    let mut media: Vec<_> = song.media.iter().filter(|clip| wanted.contains(clip.id.as_str())).cloned().collect();
    let mut markers: Vec<_> = song.markers.iter().filter(|marker| wanted.contains(marker.id.as_str())).cloned().collect();
    if motion.len() + media.len() + markers.len() != wanted.len() {
        return Err(Error::NotFound);
    }
    let origin = motion
        .iter()
        .map(|clip| clip.start_frame)
        .chain(media.iter().map(|clip| clip.start_frame))
        .chain(markers.iter().map(|marker| marker.frame))
        .min()
        .ok_or(Error::Empty)?;
    for clip in &mut motion {
        clip.start_frame -= origin;
    }
    for clip in &mut media {
        clip.start_frame -= origin;
    }
    for marker in &mut markers {
        marker.frame -= origin;
    }
    let snapshot = Clipboard { version: VERSION, rate, contents: Contents::Timeline { motion, media, markers } };
    snapshot.validate()?;
    Ok(snapshot)
}

pub fn capture_keys(song: &Song, rate: FrameRate, clip: &str, selection: &[(Axis, usize)]) -> Result<Clipboard, Error> {
    if selection.len() > MAX_KEYS {
        return Err(Error::TooLarge);
    }
    let keys: Vec<_> = selection.iter().map(|(axis, index)| (clip.to_owned(), *axis, *index)).collect();
    capture_key_selection(song, rate, &keys)
}

/// Capture keys across every selected clip. Multiple source clips become a timeline
/// selection of independent keys clips, retaining camera, axis and timing relationships.
pub fn capture_key_selection(song: &Song, rate: FrameRate, selection: &[(String, Axis, usize)]) -> Result<Clipboard, Error> {
    if selection.is_empty() {
        return Err(Error::Empty);
    }
    if selection.len() > MAX_KEYS {
        return Err(Error::TooLarge);
    }
    valid_song(song, rate)?;
    let unique: HashSet<_> = selection.iter().collect();
    let mut groups: BTreeMap<&str, BTreeMap<Axis, Vec<Key>>> = BTreeMap::new();
    for (id, axis, index) in unique {
        let source = song.motion_clip(id).ok_or(Error::NotFound)?;
        if !source.is_keys() {
            return Err(Error::NotKeys);
        }
        let key = *source.keys(*axis).get(*index).ok_or(Error::NotFound)?;
        groups.entry(id).or_default().entry(*axis).or_default().push(key);
    }
    if groups.len() > MAX_ITEMS {
        return Err(Error::TooLarge);
    }
    let mut clips = Vec::new();
    for (id, mut curves) in groups {
        let source = song.motion_clip(id).ok_or(Error::NotFound)?;
        let first = curves.values().flatten().map(|key| key.frame).min().ok_or(Error::Empty)?;
        for keys in curves.values_mut() {
            keys.sort_by_key(|key| key.frame);
            for key in keys {
                key.frame -= first;
            }
        }
        clips.push(keys_clip(
            id,
            &source.camera,
            source.start_frame.checked_add(first).ok_or(Error::InvalidData("frame numbers overflow"))?,
            curves,
        )?);
    }
    let origin = clips.iter().map(|clip| clip.start_frame).min().ok_or(Error::Empty)?;
    for clip in &mut clips {
        clip.start_frame -= origin;
    }
    let contents = if clips.len() == 1 {
        let only = clips.pop().unwrap();
        let ClipBody::Keys { curves } = only.body else { unreachable!() };
        Contents::Keys { camera: only.camera, curves }
    } else {
        Contents::Timeline { motion: clips, media: vec![], markers: vec![] }
    };
    let snapshot = Clipboard { version: VERSION, rate, contents };
    snapshot.validate()?;
    Ok(snapshot)
}

fn keys_clip(id: &str, camera: &str, start_frame: i64, curves: BTreeMap<Axis, Vec<Key>>) -> Result<MotionClip, Error> {
    if curves.values().any(Vec::is_empty) {
        return Err(Error::InvalidData("a copied curve has no keys"));
    }
    let duration_frames = curves.values().flatten().map(|key| key.frame).max().ok_or(Error::Empty)?.max(1);
    Ok(MotionClip {
        id: id.into(),
        camera: camera.into(),
        start_frame,
        duration_frames,
        axes: curves.keys().copied().collect(),
        body: ClipBody::Keys { curves },
        accepted: None,
        recorded_pass: None,
        recorded_reversed: false,
    })
}

fn check_destination(song: &Song, project: &Project, snapshot: &Clipboard, at: i64) -> Result<(), Error> {
    snapshot.validate()?;
    if snapshot.rate != project.rate {
        return Err(Error::FrameRateMismatch { copied: snapshot.rate, destination: project.rate });
    }
    if at < 0 {
        return Err(Error::InvalidData("paste must start at or after frame zero"));
    }
    if at > song.length_frames.max(MAX_SECONDS * project.rate.nominal()) {
        return Err(Error::InvalidData("paste would extend the song beyond two hours"));
    }
    valid_song(song, project.rate)
}

/// Paste at a song frame. A copied automatic preset move becomes custom-timed so the
/// snapshot keeps its exact duration. Other clips are not retimed by this operation.
pub fn paste(song: &Song, project: &Project, snapshot: &Clipboard, at: i64) -> Result<(Song, Vec<String>), Error> {
    check_destination(song, project, snapshot, at)?;
    let (motion, media, markers) = match &snapshot.contents {
        Contents::Timeline { motion, media, markers } => (motion.clone(), media.clone(), markers.clone()),
        Contents::Keys { camera, curves } => (vec![keys_clip("keys", camera, 0, curves.clone())?], vec![], vec![]),
    };
    for clip in &motion {
        if project.camera(&clip.camera).is_none() {
            return Err(Error::NoSuchCamera(clip.camera.clone()));
        }
    }
    let mut used: HashSet<String> = song
        .motion
        .iter()
        .map(|clip| clip.id.clone())
        .chain(song.media.iter().map(|clip| clip.id.clone()))
        .chain(song.markers.iter().map(|marker| marker.id.clone()))
        .chain(motion.iter().map(|clip| clip.id.clone()))
        .chain(media.iter().map(|clip| clip.id.clone()))
        .chain(markers.iter().map(|marker| marker.id.clone()))
        .collect();
    let mut serial = 0usize;
    let mut fresh = |prefix: &str| loop {
        serial += 1;
        let id = format!("{prefix}{serial}");
        if used.insert(id.clone()) {
            break id;
        }
    };
    let mut next = song.clone();
    let mut ids = Vec::new();
    for mut clip in motion {
        clip.id = fresh("c");
        clip.start_frame = at.checked_add(clip.start_frame).ok_or(Error::InvalidData("frame numbers overflow"))?;
        clip.accepted = None;
        clip.recorded_pass = None;
        clip.recorded_reversed = false;
        if let ClipBody::PresetMove { auto_duration, .. } = &mut clip.body {
            *auto_duration = false;
        }
        ids.push(clip.id.clone());
        next.motion.push(clip);
    }
    for mut clip in media {
        clip.id = fresh("m");
        clip.start_frame = at.checked_add(clip.start_frame).ok_or(Error::InvalidData("frame numbers overflow"))?;
        ids.push(clip.id.clone());
        next.media.push(clip);
    }
    for mut marker in markers {
        marker.id = fresh("k");
        marker.frame = at.checked_add(marker.frame).ok_or(Error::InvalidData("frame numbers overflow"))?;
        ids.push(marker.id.clone());
        next.markers.push(marker);
    }
    let end = checked_end(&next)?;
    if end > song.length_frames.max(MAX_SECONDS * project.rate.nominal()) {
        return Err(Error::InvalidData("paste would extend the song beyond two hours"));
    }
    next.length_frames = next.length_frames.max(end);
    next.validate(project.rate).map_err(Error::Invalid)?;
    Ok((next, ids))
}

/// Insert a Keys snapshot into one existing keys clip at a song frame. Keys retain
/// their handles and interpolation; existing keys are never silently replaced.
pub fn paste_keys(
    song: &Song,
    project: &Project,
    snapshot: &Clipboard,
    target: &str,
    at: i64,
) -> Result<(Song, Vec<(Axis, usize)>), Error> {
    check_destination(song, project, snapshot, at)?;
    let Contents::Keys { camera, curves } = &snapshot.contents else {
        return Err(Error::WrongKind);
    };
    if project.camera(camera).is_none() {
        return Err(Error::NoSuchCamera(camera.clone()));
    }
    let mut next = song.clone();
    let clip = next.motion_clip_mut(target).ok_or(Error::NotFound)?;
    if clip.camera != *camera {
        return Err(Error::OtherCamera);
    }
    let offset = at.checked_sub(clip.start_frame).ok_or(Error::OutsideClip)?;
    if offset < 0 || offset > clip.duration_frames {
        return Err(Error::OutsideClip);
    }
    let ClipBody::Keys { curves: destination } = &mut clip.body else {
        return Err(Error::NotKeys);
    };
    let mut inserted = Vec::new();
    for (&axis, keys) in curves {
        let target = destination.entry(axis).or_default();
        for key in keys {
            let frame = offset.checked_add(key.frame).filter(|frame| *frame <= clip.duration_frames).ok_or(Error::OutsideClip)?;
            if target.iter().any(|key| key.frame == frame) {
                return Err(Error::KeyCollision { axis, frame });
            }
            target.push(Key { frame, ..*key });
            inserted.push((axis, frame));
        }
        target.sort_by_key(|key| key.frame);
        if !clip.axes.contains(&axis) {
            clip.axes.push(axis);
        }
    }
    clip.accepted = None;
    clip.recorded_pass = None;
    clip.recorded_reversed = false;
    let selected = inserted
        .into_iter()
        .map(|(axis, frame)| (axis, destination[&axis].binary_search_by_key(&frame, |key| key.frame).unwrap()))
        .collect();
    next.validate(project.rate).map_err(Error::Invalid)?;
    Ok((next, selected))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::CameraConfig;
    use crate::pose::Pose;
    use crate::song::{Accepted, Ease, Interp, PresetRef, Track};

    fn project() -> Project {
        let mut project = Project::default();
        project.cameras.push(CameraConfig::new("cam2", "Second"));
        project
    }

    fn preset(id: &str, start: i64, duration: i64) -> MotionClip {
        MotionClip {
            id: id.into(),
            camera: "cam1".into(),
            start_frame: start,
            duration_frames: duration,
            axes: vec![Axis::Pan],
            body: ClipBody::PresetMove {
                auto_duration: true,
                from: FromPose::Pinned(PresetRef {
                    preset_id: "wide".into(),
                    name: "Wide".into(),
                    pose: Pose::home(),
                    ..Default::default()
                }),
                to: PresetRef {
                    preset_id: "close".into(),
                    name: "Close".into(),
                    pose: Pose { pan: 30.0, ..Pose::home() },
                    ..Default::default()
                },
                ease: Ease::EaseOut,
            },
            accepted: Some(Accepted { by: "operator".into(), at: "yesterday".into() }),
            recorded_pass: Some("pass-1".into()),
            recorded_reversed: false,
        }
    }

    fn source() -> Song {
        let mut song = Song::new("source", "Source", "01:00:00:00", 300);
        song.motion.push(preset("c1", 20, 40));
        let curves = BTreeMap::from([(
            Axis::Zoom,
            vec![
                Key { out_handle: [7.0, 0.1], ..Key::new(0, 0.2, Interp::Bezier) },
                Key { in_handle: [-8.0, -0.2], ..Key::new(40, 0.8, Interp::Linear) },
            ],
        )]);
        let mut keys = keys_clip("c2", "cam2", 80, curves).unwrap();
        keys.recorded_pass = Some("recorded-2".into());
        song.motion.push(keys);
        song.media = vec![
            MediaClip {
                id: "m1".into(),
                media_id: "audio-file".into(),
                track: Track::Audio,
                start_frame: 10,
                in_frame: 5,
                out_frame: 35,
                media_frames: 100,
                name: "Music".into(),
                muted: true,
            },
            MediaClip {
                id: "m2".into(),
                media_id: "video-file".into(),
                track: Track::Video,
                start_frame: 60,
                in_frame: 30,
                out_frame: 80,
                media_frames: 300,
                name: "Reference".into(),
                muted: false,
            },
        ];
        song.markers = vec![
            Marker { id: "k1".into(), frame: 5, name: "Entry".into() },
            Marker { id: "k2".into(), frame: 145, name: "End".into() },
        ];
        song
    }

    fn all(song: &Song) -> Vec<String> {
        song.motion
            .iter()
            .map(|clip| clip.id.clone())
            .chain(song.media.iter().map(|clip| clip.id.clone()))
            .chain(song.markers.iter().map(|marker| marker.id.clone()))
            .collect()
    }

    #[test]
    fn a_mixed_selection_round_trips_and_pastes_across_songs_with_exact_timing_and_fresh_ids() {
        let project = project();
        let source = source();
        let snapshot = capture(&source, project.rate, &all(&source)).unwrap();
        assert_eq!(snapshot.item_count(), 6);
        assert_eq!(snapshot.media_ids(), ["audio-file", "video-file"]);
        assert_eq!(Clipboard::from_json(&snapshot.to_json().unwrap()).unwrap(), snapshot);
        let mut destination = Song::new("destination", "Destination", "02:00:00:00", 30);
        destination.motion.push(preset("existing", 0, 4));
        let before = destination.clone();
        let (pasted, ids) = paste(&destination, &project, &snapshot, 200).unwrap();
        assert_eq!(destination, before, "paste is pure");
        assert_eq!(pasted.motion[0], before.motion[0], "existing automatic timing is not refreshed");
        assert_eq!(pasted.length_frames, 340);
        assert_eq!(pasted.id, "destination");
        assert_eq!(pasted.start_tc, "02:00:00:00");
        assert_eq!(ids.len(), 6);
        assert!(ids.iter().all(|id| !all(&source).contains(id) && !all(&destination).contains(id)));
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), ids.len());
        for (original, copy) in source.motion.iter().zip(&pasted.motion[1..]) {
            let mut expected = original.clone();
            expected.id = copy.id.clone();
            expected.start_frame = original.start_frame - 5 + 200;
            expected.accepted = None;
            expected.recorded_pass = None;
            if let ClipBody::PresetMove { auto_duration, .. } = &mut expected.body {
                *auto_duration = false;
            }
            assert_eq!(*copy, expected, "axes, key handles, camera, ease and poses are retained");
        }
        for (original, copy) in source.media.iter().zip(&pasted.media) {
            let mut expected = original.clone();
            expected.id = copy.id.clone();
            expected.start_frame = original.start_frame - 5 + 200;
            assert_eq!(*copy, expected, "source, trim, track and mute state are retained");
        }
        assert_eq!(
            pasted.markers.iter().map(|marker| (&*marker.name, marker.frame)).collect::<Vec<_>>(),
            [("Entry", 200), ("End", 340)]
        );
    }

    #[test]
    fn overlap_missing_references_and_rate_mismatch_refuse_the_whole_paste() {
        let project = project();
        let song = source();
        let before = song.clone();
        let snapshot = capture(&song, project.rate, &all(&song)).unwrap();
        assert!(matches!(paste(&song, &project, &snapshot, 5), Err(Error::Invalid(Invalid::MediaOverlap { .. }))));
        assert_eq!(song, before);
        let mut wrong_rate = project.clone();
        wrong_rate.rate = FrameRate::F30;
        assert!(matches!(paste(&song, &wrong_rate, &snapshot, 400), Err(Error::FrameRateMismatch { .. })));
        let mut missing_camera = project.clone();
        missing_camera.cameras.pop();
        assert_eq!(paste(&song, &missing_camera, &snapshot, 400), Err(Error::NoSuchCamera("cam2".into())));
        assert_eq!(capture(&song, project.rate, &["c1".into(), "gone".into()]), Err(Error::NotFound));
        assert_eq!(capture(&song, project.rate, &[]), Err(Error::Empty));
        assert!(paste(&song, &project, &snapshot, -1).is_err());
    }

    #[test]
    fn markers_alone_can_be_copied_and_their_spacing_grows_the_song() {
        let project = project();
        let song = source();
        let snapshot = capture(&song, project.rate, &["k2".into(), "k1".into()]).unwrap();
        let empty = Song::new("s", "S", "00:00:00:00", 1);
        let (pasted, ids) = paste(&empty, &project, &snapshot, 10).unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(pasted.length_frames, 150);
        assert_eq!(pasted.markers.iter().map(|marker| marker.frame).collect::<Vec<_>>(), [10, 150]);
        assert!(pasted.motion.is_empty() && pasted.media.is_empty());
    }

    fn key_source() -> Song {
        let mut song = Song::new("source", "Keys", "00:00:00:00", 400);
        let mut keys = vec![
            Key::new(0, 0.0, Interp::Hold),
            Key::new(10, 10.0, Interp::Bezier),
            Key::new(20, 20.0, Interp::Linear),
            Key::new(40, 40.0, Interp::Hold),
        ];
        keys[1].out_handle = [3.0, 2.0];
        keys[2].in_handle = [-4.0, -1.0];
        song.motion.push(keys_clip("keys", "cam1", 100, BTreeMap::from([(Axis::Pan, keys)])).unwrap());
        song.motion.push(
            keys_clip(
                "later",
                "cam1",
                200,
                BTreeMap::from([(Axis::Pan, vec![Key::new(0, 40.0, Interp::Linear), Key::new(20, 50.0, Interp::Bezier)])]),
            )
            .unwrap(),
        );
        song
    }

    #[test]
    fn individual_keys_keep_handles_and_insert_without_overwriting_existing_keys() {
        let project = project();
        let song = key_source();
        let snapshot = capture_keys(&song, project.rate, "keys", &[(Axis::Pan, 2), (Axis::Pan, 1)]).unwrap();
        assert!(snapshot.is_keys());
        assert_eq!(snapshot.item_count(), 2);
        let mut destination = Song::new("s", "S", "00:00:00:00", 100);
        let mut target = keys_clip(
            "target",
            "cam1",
            0,
            BTreeMap::from([(Axis::Pan, vec![Key::new(0, -1.0, Interp::Linear), Key::new(100, 99.0, Interp::Hold)])]),
        )
        .unwrap();
        target.accepted = Some(Accepted { by: "me".into(), at: "then".into() });
        target.recorded_pass = Some("raw".into());
        destination.motion.push(target);
        let (pasted, selected) = paste_keys(&destination, &project, &snapshot, "target", 30).unwrap();
        assert_eq!(selected, [(Axis::Pan, 1), (Axis::Pan, 2)]);
        let target = &pasted.motion[0];
        assert_eq!(target.keys(Axis::Pan).iter().map(|key| key.frame).collect::<Vec<_>>(), [0, 30, 40, 100]);
        assert_eq!(target.keys(Axis::Pan)[1].out_handle, [3.0, 2.0]);
        assert_eq!(target.keys(Axis::Pan)[2].in_handle, [-4.0, -1.0]);
        assert_eq!(target.keys(Axis::Pan)[1].interp, Interp::Bezier);
        assert!(target.accepted.is_none() && target.recorded_pass.is_none());
        assert_eq!(
            paste_keys(&destination, &project, &snapshot, "target", 0),
            Err(Error::KeyCollision { axis: Axis::Pan, frame: 0 })
        );
        assert_eq!(paste_keys(&destination, &project, &snapshot, "target", 95), Err(Error::OutsideClip));
        assert_eq!(destination.motion[0].keys(Axis::Pan).len(), 2, "refused insertions change nothing");
        let empty = Song::new("s", "S", "00:00:00:00", 1);
        let (pasted, _) = paste(&empty, &project, &snapshot, 25).unwrap();
        assert_eq!((pasted.motion[0].start_frame, pasted.motion[0].duration_frames), (25, 10));
        assert_eq!(pasted.motion[0].keys(Axis::Pan)[1].frame, 10);
    }

    #[test]
    fn keys_from_several_source_clips_keep_every_group_and_their_relative_times() {
        let project = project();
        let song = key_source();
        let snapshot = capture_key_selection(
            &song,
            project.rate,
            &[("later".into(), Axis::Pan, 1), ("keys".into(), Axis::Pan, 1), ("keys".into(), Axis::Pan, 2)],
        )
        .unwrap();
        assert!(!snapshot.is_keys());
        let empty = Song::new("s", "S", "00:00:00:00", 1);
        let (pasted, _) = paste(&empty, &project, &snapshot, 50).unwrap();
        assert_eq!(pasted.motion.len(), 2);
        assert_eq!(pasted.motion.iter().map(|clip| clip.start_frame).collect::<Vec<_>>(), [50, 160]);
        assert_eq!(pasted.motion[0].keys(Axis::Pan).iter().map(|key| key.value).collect::<Vec<_>>(), [10.0, 20.0]);
        assert_eq!(pasted.motion[1].keys(Axis::Pan)[0].value, 50.0);
        assert_eq!(pasted.length_frames, 161, "a standalone key gets a one-frame clip");
        assert_eq!(capture_key_selection(&song, project.rate, &[("keys".into(), Axis::Pan, 999)]), Err(Error::NotFound));
    }

    #[test]
    fn adding_an_axis_to_a_keys_clip_checks_overlap_atomically() {
        let project = project();
        let snapshot = Clipboard {
            version: VERSION,
            rate: project.rate,
            contents: Contents::Keys {
                camera: "cam1".into(),
                curves: BTreeMap::from([(Axis::Tilt, vec![Key::new(0, 1.0, Interp::Linear)])]),
            },
        };
        let mut song = key_source();
        let other = keys_clip(
            "tilt",
            "cam1",
            100,
            BTreeMap::from([(Axis::Tilt, vec![Key::new(0, 0.0, Interp::Linear), Key::new(40, 5.0, Interp::Linear)])]),
        )
        .unwrap();
        song.motion.push(other);
        let before = song.clone();
        assert!(matches!(
            paste_keys(&song, &project, &snapshot, "keys", 115),
            Err(Error::Invalid(Invalid::Overlap { axis: Axis::Tilt, .. }))
        ));
        assert_eq!(song, before);
    }

    #[test]
    fn malformed_or_huge_system_clipboards_are_refused_before_unsafe_arithmetic_or_compilation() {
        let project = project();
        let source = source();
        let snapshot = capture(&source, project.rate, &all(&source)).unwrap();
        for text in [
            "ordinary text",
            "{}",
            r#"{"version":999,"rate":"25","contents":{"kind":"timeline","motion":[],"media":[],"markers":[]}}"#,
        ] {
            assert!(Clipboard::from_json(text).is_err());
        }
        assert_eq!(Clipboard::from_json(&"x".repeat(MAX_JSON_BYTES + 1)), Err(Error::TooLarge));
        let mut unknown = snapshot.clone();
        unknown.version = 99;
        assert_eq!(paste(&source, &project, &unknown, 400), Err(Error::UnknownVersion(99)));
        for (start, duration) in [(0, i64::MAX), (i64::MAX, 2), (-1, 10), (0, 0)] {
            let mut value = serde_json::to_value(&snapshot).unwrap();
            value["contents"]["motion"][0]["start_frame"] = start.into();
            value["contents"]["motion"][0]["duration_frames"] = duration.into();
            assert!(Clipboard::from_json(&value.to_string()).is_err(), "{start}/{duration} must be refused");
        }
        let mut hostile = Clipboard {
            version: VERSION,
            rate: project.rate,
            contents: Contents::Timeline {
                motion: vec![],
                media: vec![],
                markers: vec![
                    Marker { id: "a".into(), frame: 0, name: "A".into() },
                    Marker { id: "b".into(), frame: i64::MAX, name: "B".into() },
                ],
            },
        };
        assert!(hostile.to_json().is_err());
        assert!(paste(&source, &project, &hostile, 0).is_err());
        if let Contents::Timeline { markers, .. } = &mut hostile.contents {
            markers[1].frame = 1;
            markers.resize(MAX_ITEMS + 1, Marker { id: "many".into(), frame: 0, name: "M".into() });
        }
        assert_eq!(hostile.to_json(), Err(Error::TooLarge));
        assert!(paste(&source, &project, &snapshot, i64::MAX).is_err());
        assert!(paste(&source, &project, &snapshot, MAX_SECONDS * project.rate.nominal()).is_err());
        let oversized_name = Clipboard {
            version: VERSION,
            rate: project.rate,
            contents: Contents::Timeline {
                motion: vec![],
                media: vec![],
                markers: vec![Marker { id: "a".into(), frame: 0, name: "x".repeat(MAX_JSON_BYTES) }],
            },
        };
        assert_eq!(oversized_name.to_json(), Err(Error::TooLarge));
        let mut bad_trim = serde_json::to_value(&snapshot).unwrap();
        bad_trim["contents"]["media"][0]["in_frame"] = i64::MIN.into();
        bad_trim["contents"]["media"][0]["out_frame"] = i64::MAX.into();
        assert!(Clipboard::from_json(&bad_trim.to_string()).is_err());
    }
}
