//! Every timeline gesture maps to one function here, so editing is tested
//! without a window. Each returns the new song or a typed refusal that the app
//! shows as the reason a drag will not land.

use crate::compile::{refresh_auto, suggested_duration};
use crate::eval::{ease_keys, split_keys, Evaluator};
use crate::pose::{Axis, Preset, Project};
use crate::song::{
    Accepted, ClipBody, Ease, FromPose, Interp, Invalid, Key, Marker, MediaClip, MotionClip, PresetRef, Song, Track,
};
use crate::time::Timecode;
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, PartialEq, Debug)]
pub enum Refusal {
    NotFound,
    Invalid(Invalid),
    NotAPresetMove,
    NotKeys,
    /// A preset move is under the playhead on that axis.
    ConvertFirst(Axis),
    KeysBeyondTrim,
    NoRoom,
    Empty,
    /// A preset holds one camera's pose; it cannot be the target of another camera's move.
    OtherCamera,
    NoSuchCamera,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::NotFound => write!(f, "That item is no longer on the timeline"),
            Refusal::Invalid(i) => write!(f, "{i}"),
            Refusal::NotAPresetMove => write!(f, "Only a preset move has this setting"),
            Refusal::NotKeys => write!(f, "Only a keys clip has keys"),
            Refusal::ConvertFirst(a) => {
                write!(f, "A preset move drives {} here. Convert it to keys to add a key", a.label().to_lowercase())
            }
            Refusal::KeysBeyondTrim => write!(f, "The clip cannot be trimmed past its keys"),
            Refusal::NoRoom => write!(f, "There is no room here"),
            Refusal::Empty => write!(f, "Nothing is selected"),
            Refusal::OtherCamera => write!(f, "That preset belongs to another camera"),
            Refusal::NoSuchCamera => write!(f, "That camera is not in the project"),
        }
    }
}

pub type Edit = Result<Song, Refusal>;

fn finish(mut song: Song, project: &Project) -> Edit {
    // Checked before auto-timed moves are refreshed, so an edit that lands on
    // an existing move is refused instead of squeezing that move.
    song.validate(project.rate).map_err(Refusal::Invalid)?;
    refresh_auto(&mut song, project);
    song.validate(project.rate).map_err(Refusal::Invalid)?;
    Ok(song)
}

fn motion_mut<'a>(song: &'a mut Song, id: &str) -> Result<&'a mut MotionClip, Refusal> {
    let clip = song.motion_clip_mut(id).ok_or(Refusal::NotFound)?;
    clip.accepted = None;
    Ok(clip)
}

fn grow(song: &mut Song, end: i64) {
    if song.length_frames < end {
        song.length_frames = end;
    }
}

/// Drop a preset on a frame. The move is for the preset's camera: it ends on the
/// preset and starts from the pose already programmed for that camera there.
/// Returns the new clip's id.
pub fn insert_preset_move(song: &Song, project: &Project, preset: &Preset, frame: i64) -> Result<(Song, String), Refusal> {
    let mut next = song.clone();
    let frame = frame.max(0);
    let id = next.next_id("c");
    let clip = MotionClip {
        id: id.clone(),
        camera: preset.camera.clone(),
        start_frame: frame,
        duration_frames: 1,
        axes: Axis::ALL.to_vec(),
        body: ClipBody::PresetMove {
            auto_duration: true,
            from: FromPose::linked(),
            to: PresetRef::of(preset),
            ease: Ease::EaseInOut,
        },
        accepted: None,
        recorded_pass: None,
    };
    next.motion.push(clip);
    grow(&mut next, frame + 1);
    Ok((finish(next, project)?, id))
}

/// Room a new auto-timed move would get at `frame` and the time it wants.
/// `None` for room means nothing follows it.
pub fn drop_preview(song: &Song, project: &Project, preset: &Preset, frame: i64) -> Result<DropPreview, Refusal> {
    let (next, id) = insert_preset_move(song, project, preset, frame)?;
    let clip = next.motion_clip(&id).ok_or(Refusal::NotFound)?;
    let wanted = suggested_duration(&next, clip, project).unwrap_or(clip.duration_frames);
    let from_name = song
        .motion
        .iter()
        .filter(|c| c.camera == preset.camera && c.end_frame() <= frame.max(0))
        .max_by_key(|c| c.start_frame)
        .and_then(|c| match &c.body {
            ClipBody::PresetMove { to, .. } => Some(to.name.clone()),
            _ => None,
        });
    Ok(DropPreview { start_frame: clip.start_frame, duration_frames: clip.duration_frames, wanted_frames: wanted, from_name })
}

#[derive(Clone, PartialEq, Debug)]
pub struct DropPreview {
    pub start_frame: i64,
    pub duration_frames: i64,
    pub wanted_frames: i64,
    /// Name of the preset the move starts from, when the previous move ended on one.
    pub from_name: Option<String>,
}

/// Move one clip (motion or media) or marker to a new start frame.
pub fn move_clip(song: &Song, project: &Project, id: &str, new_start: i64) -> Edit {
    let start = if let Some(c) = song.motion_clip(id) {
        c.start_frame
    } else if let Some(c) = song.media_clip(id) {
        c.start_frame
    } else if let Some(m) = song.markers.iter().find(|m| m.id == id) {
        m.frame
    } else {
        return Err(Refusal::NotFound);
    };
    move_clips(song, project, &[id.to_string()], new_start.max(0) - start)
}

/// Move several clips and markers by the same number of frames.
pub fn move_clips(song: &Song, project: &Project, ids: &[String], delta: i64) -> Edit {
    if ids.is_empty() {
        return Err(Refusal::Empty);
    }
    let mut next = song.clone();
    let has = |id: &String| ids.contains(id);
    let earliest = next
        .motion
        .iter()
        .filter(|c| has(&c.id))
        .map(|c| c.start_frame)
        .chain(next.media.iter().filter(|c| has(&c.id)).map(|c| c.start_frame))
        .chain(next.markers.iter().filter(|m| has(&m.id)).map(|m| m.frame))
        .min()
        .ok_or(Refusal::NotFound)?;
    let delta = delta.max(-earliest);
    let mut end = 0;
    for c in next.motion.iter_mut().filter(|c| has(&c.id)) {
        c.start_frame += delta;
        c.accepted = None;
        end = end.max(c.end_frame());
    }
    for c in next.media.iter_mut().filter(|c| has(&c.id)) {
        c.start_frame += delta;
        end = end.max(c.end_frame());
    }
    for m in next.markers.iter_mut().filter(|m| has(&m.id)) {
        m.frame += delta;
        end = end.max(m.frame);
    }
    grow(&mut next, end);
    finish(next, project)
}

/// Drag the right edge. A preset move switches to custom time.
pub fn trim_clip_end(song: &Song, project: &Project, id: &str, new_end: i64) -> Edit {
    let mut next = song.clone();
    if next.motion_clip(id).is_some() {
        let c = motion_mut(&mut next, id)?;
        let dur = (new_end - c.start_frame).max(1);
        match &mut c.body {
            ClipBody::PresetMove { auto_duration, .. } => *auto_duration = false,
            ClipBody::Keys { curves } => {
                let last = curves.values().filter_map(|k| k.last()).map(|k| k.frame).max().unwrap_or(0);
                if dur < last {
                    return Err(Refusal::KeysBeyondTrim);
                }
            }
        }
        c.duration_frames = dur;
        let end = c.end_frame();
        grow(&mut next, end);
    } else {
        let c = next.media_clip_mut(id).ok_or(Refusal::NotFound)?;
        let dur = (new_end - c.start_frame).max(1);
        c.out_frame = (c.in_frame + dur).min(c.media_frames);
        let end = c.end_frame();
        grow(&mut next, end);
    }
    finish(next, project)
}

/// Drag the left edge. The end stays put.
pub fn trim_clip_start(song: &Song, project: &Project, id: &str, new_start: i64) -> Edit {
    let mut next = song.clone();
    if next.motion_clip(id).is_some() {
        let c = motion_mut(&mut next, id)?;
        let end = c.end_frame();
        let start = new_start.clamp(0, end - 1);
        let shift = start - c.start_frame;
        match &mut c.body {
            ClipBody::PresetMove { auto_duration, .. } => *auto_duration = false,
            ClipBody::Keys { curves } => {
                let first = curves.values().filter_map(|k| k.first()).map(|k| k.frame).min().unwrap_or(i64::MAX);
                if shift > first {
                    return Err(Refusal::KeysBeyondTrim);
                }
                for keys in curves.values_mut() {
                    for k in keys.iter_mut() {
                        k.frame -= shift;
                    }
                }
            }
        }
        c.start_frame = start;
        c.duration_frames = end - start;
    } else {
        let c = next.media_clip_mut(id).ok_or(Refusal::NotFound)?;
        let end = c.end_frame();
        let start = new_start.clamp(0, end - 1);
        let mut shift = start - c.start_frame;
        if c.in_frame + shift < 0 {
            shift = -c.in_frame;
        }
        c.in_frame += shift;
        c.start_frame += shift;
    }
    finish(next, project)
}

fn preset_body<'a>(
    song: &'a mut Song,
    id: &str,
) -> Result<(&'a mut bool, &'a mut FromPose, &'a mut PresetRef, &'a mut Ease), Refusal> {
    match &mut motion_mut(song, id)?.body {
        ClipBody::PresetMove { auto_duration, from, to, ease } => Ok((auto_duration, from, to, ease)),
        _ => Err(Refusal::NotAPresetMove),
    }
}

pub fn set_custom_duration(song: &Song, project: &Project, id: &str, frames: i64) -> Edit {
    let mut next = song.clone();
    *preset_body(&mut next, id)?.0 = false;
    let c = motion_mut(&mut next, id)?;
    c.duration_frames = frames.max(1);
    let end = c.end_frame();
    grow(&mut next, end);
    finish(next, project)
}

pub fn set_auto_duration(song: &Song, project: &Project, id: &str) -> Edit {
    let mut next = song.clone();
    *preset_body(&mut next, id)?.0 = true;
    finish(next, project)
}

pub fn set_ease(song: &Song, project: &Project, id: &str, ease: Ease) -> Edit {
    let mut next = song.clone();
    *preset_body(&mut next, id)?.3 = ease;
    finish(next, project)
}

/// Linked, or pinned to a preset for "copy X to Y".
pub fn set_from(song: &Song, project: &Project, id: &str, from: FromPose) -> Edit {
    let mut next = song.clone();
    *preset_body(&mut next, id)?.1 = from;
    finish(next, project)
}

pub fn set_to(song: &Song, project: &Project, id: &str, preset: &Preset) -> Edit {
    let mut next = song.clone();
    if next.motion_clip(id).ok_or(Refusal::NotFound)?.camera != preset.camera {
        return Err(Refusal::OtherCamera);
    }
    *preset_body(&mut next, id)?.2 = PresetRef::of(preset);
    finish(next, project)
}

/// Hand clips to another camera. Refused where that camera already moves.
pub fn set_camera(song: &Song, project: &Project, ids: &[String], camera: &str) -> Edit {
    if project.camera(camera).is_none() {
        return Err(Refusal::NoSuchCamera);
    }
    let mut next = song.clone();
    let mut any = false;
    for c in next.motion.iter_mut().filter(|c| ids.contains(&c.id)) {
        any = true;
        if c.camera != camera {
            c.camera = camera.to_string();
            c.accepted = None;
        }
    }
    if !any {
        return Err(Refusal::Empty);
    }
    finish(next, project)
}

pub fn set_axes(song: &Song, project: &Project, id: &str, axes: &[Axis]) -> Edit {
    let mut next = song.clone();
    let c = motion_mut(&mut next, id)?;
    c.axes = Axis::ALL.into_iter().filter(|a| axes.contains(a)).collect();
    if let ClipBody::Keys { curves } = &mut c.body {
        curves.retain(|a, _| axes.contains(a));
    }
    finish(next, project)
}

fn to_keys_in_place(song: &mut Song, id: &str) -> Result<(), Refusal> {
    let clip = song.motion_clip(id).ok_or(Refusal::NotFound)?.clone();
    let ClipBody::PresetMove { to, ease, .. } = &clip.body else {
        return Ok(());
    };
    let ev = Evaluator::new(song, &clip.camera);
    let mut curves = BTreeMap::new();
    for &axis in &clip.axes {
        let from = ev.value_at_start(&clip, axis);
        curves.insert(axis, ease_keys(from, to.pose.get(axis), clip.duration_frames, *ease).to_vec());
    }
    let c = song.motion_clip_mut(id).ok_or(Refusal::NotFound)?;
    c.body = ClipBody::Keys { curves };
    Ok(())
}

/// Turn a preset move into a keys clip with two keys per axis. The motion is
/// unchanged frame for frame.
pub fn convert_to_keys(song: &Song, project: &Project, id: &str) -> Edit {
    let mut next = song.clone();
    if !next.motion_clip(id).ok_or(Refusal::NotFound)?.is_preset_move() {
        return Err(Refusal::NotAPresetMove);
    }
    to_keys_in_place(&mut next, id)?;
    finish(next, project)
}

fn insert_key_sorted(keys: &mut Vec<Key>, at: i64, fallback: f64) {
    if keys.iter().any(|k| k.frame == at) {
        return;
    }
    if keys.is_empty() {
        keys.push(Key::new(at, fallback, Interp::Bezier));
        return;
    }
    let inside = at > keys[0].frame && at < keys[keys.len() - 1].frame;
    if inside {
        let (mut left, right) = split_keys(keys, at);
        // Rejoin: the split key carries both handles.
        let mid_out = right[0].out_handle;
        if let Some(l) = left.last_mut() {
            l.out_handle = mid_out;
            l.interp = right[0].interp;
        }
        left.extend_from_slice(&right[1..]);
        *keys = left;
    } else if at < keys[0].frame {
        let v = keys[0].value;
        keys.insert(0, Key::new(at, v, Interp::Bezier));
    } else {
        let v = keys[keys.len() - 1].value;
        keys.push(Key::new(at, v, Interp::Bezier));
    }
}

/// `K`: add a key on each of a camera's axes at a song frame, holding the current
/// value. Extends the keys clip that ends before the frame when it can; otherwise
/// starts a new one.
pub fn add_key(song: &Song, project: &Project, camera: &str, axes: &[Axis], frame: i64) -> Edit {
    if axes.is_empty() {
        return Err(Refusal::Empty);
    }
    let frame = frame.max(0);
    let mut next = song.clone();
    let mut fresh: Vec<Axis> = Vec::new();
    for &axis in axes {
        let value = Evaluator::new(&next, camera).axis_at(axis, frame as f64);
        // A clip covering the frame, or a keys clip ending exactly on it.
        let covering = next
            .clip_on_axis_at(camera, axis, frame)
            .or_else(|| {
                next.motion.iter().find(|c| c.camera == camera && c.has_axis(axis) && c.is_keys() && frame == c.end_frame())
            })
            .map(|c| (c.id.clone(), c.is_keys(), frame < c.end_frame()));
        if let Some((id, is_keys, strictly)) = covering {
            if is_keys {
                let c = motion_mut(&mut next, &id)?;
                let local = frame - c.start_frame;
                if let ClipBody::Keys { curves } = &mut c.body {
                    insert_key_sorted(curves.entry(axis).or_default(), local, value);
                }
                continue;
            } else if strictly {
                return Err(Refusal::ConvertFirst(axis));
            }
        }
        // Extend the previous keys clip if nothing is in the way on any of its axes.
        let prev = next
            .motion
            .iter()
            .filter(|c| c.camera == camera && c.has_axis(axis) && c.end_frame() <= frame)
            .max_by_key(|c| c.start_frame)
            .filter(|c| c.is_keys())
            .cloned();
        let extended = prev.is_some_and(|p| {
            let blocked = next.motion.iter().any(|o| {
                o.id != p.id
                    && o.camera == camera
                    && o.axes.iter().any(|a| p.has_axis(*a))
                    && o.start_frame < frame
                    && o.end_frame() > p.start_frame
            });
            if blocked {
                return false;
            }
            if let Some(c) = next.motion_clip_mut(&p.id) {
                c.accepted = None;
                c.duration_frames = frame - c.start_frame;
                let local = c.duration_frames;
                if let ClipBody::Keys { curves } = &mut c.body {
                    insert_key_sorted(curves.entry(axis).or_default(), local, value);
                }
            }
            true
        });
        if !extended {
            fresh.push(axis);
        }
    }
    if !fresh.is_empty() {
        let room = next
            .motion
            .iter()
            .filter(|o| o.camera == camera && o.start_frame > frame && o.axes.iter().any(|a| fresh.contains(a)))
            .map(|o| o.start_frame - frame)
            .min()
            .unwrap_or(i64::MAX);
        let duration = project.rate.nominal().min(room).max(1);
        let ev = Evaluator::new(&next, camera);
        let mut curves = BTreeMap::new();
        for &axis in &fresh {
            curves.insert(axis, vec![Key::new(0, ev.axis_at(axis, frame as f64), Interp::Bezier)]);
        }
        let id = next.next_id("c");
        next.motion.push(MotionClip {
            id,
            camera: camera.to_string(),
            start_frame: frame,
            duration_frames: duration,
            axes: Axis::ALL.into_iter().filter(|a| fresh.contains(a)).collect(),
            body: ClipBody::Keys { curves },
            accepted: None,
            recorded_pass: None,
        });
        grow(&mut next, frame + duration);
    }
    finish(next, project)
}

fn keys_mut<'a>(song: &'a mut Song, clip: &str, axis: Axis) -> Result<&'a mut Vec<Key>, Refusal> {
    match &mut motion_mut(song, clip)?.body {
        ClipBody::Keys { curves } => Ok(curves.entry(axis).or_default()),
        _ => Err(Refusal::NotKeys),
    }
}

/// Graph editor double-click: add a key with an explicit value at a clip-local frame.
pub fn add_key_at(song: &Song, project: &Project, clip: &str, axis: Axis, local: i64, value: Option<f64>) -> Edit {
    let mut next = song.clone();
    let c = next.motion_clip(clip).ok_or(Refusal::NotFound)?;
    if !c.has_axis(axis) {
        return Err(Refusal::NotFound);
    }
    let local = local.clamp(0, c.duration_frames);
    let current = Evaluator::new(&next, &c.camera).axis_at(axis, (c.start_frame + local) as f64);
    let keys = keys_mut(&mut next, clip, axis)?;
    insert_key_sorted(keys, local, current);
    if let (Some(v), Some(k)) = (value, keys.iter_mut().find(|k| k.frame == local)) {
        k.value = v;
    }
    finish(next, project)
}

/// Move selected keys by a frame and value offset. Keys keep their order: each
/// is clamped between its unselected neighbours and inside the clip.
pub fn shift_keys(song: &Song, project: &Project, clip: &str, selection: &[(Axis, usize)], dframe: i64, dvalue: f64) -> Edit {
    if selection.is_empty() {
        return Err(Refusal::Empty);
    }
    let mut next = song.clone();
    let duration = next.motion_clip(clip).ok_or(Refusal::NotFound)?.duration_frames;
    // Largest shift every selected key can make without crossing a neighbour.
    let mut lo = i64::MIN;
    let mut hi = i64::MAX;
    for &(axis, idx) in selection {
        let keys = keys_mut(&mut next, clip, axis)?;
        let k = keys.get(idx).ok_or(Refusal::NotFound)?;
        let sel = |i: usize| selection.contains(&(axis, i));
        let left = (0..idx).rev().find(|i| !sel(*i)).map(|i| keys[i].frame + 1).unwrap_or(0);
        let right = (idx + 1..keys.len()).find(|i| !sel(*i)).map(|i| keys[i].frame - 1).unwrap_or(i64::MAX);
        lo = lo.max(left - k.frame);
        hi = hi.min(right.saturating_sub(k.frame));
    }
    let dframe = dframe.clamp(lo.min(0), hi.max(0));
    let mut max_frame = duration;
    for &(axis, idx) in selection {
        let keys = keys_mut(&mut next, clip, axis)?;
        keys[idx].frame += dframe;
        keys[idx].value += dvalue;
        max_frame = max_frame.max(keys[idx].frame);
    }
    if max_frame > duration {
        let c = motion_mut(&mut next, clip)?;
        c.duration_frames = max_frame;
        let end = c.end_frame();
        grow(&mut next, end);
    }
    finish(next, project)
}

pub fn move_key(song: &Song, project: &Project, clip: &str, axis: Axis, index: usize, new_local: i64, new_value: f64) -> Edit {
    let k = *song.motion_clip(clip).ok_or(Refusal::NotFound)?.keys(axis).get(index).ok_or(Refusal::NotFound)?;
    shift_keys(song, project, clip, &[(axis, index)], new_local - k.frame, new_value - k.value)
}

pub fn set_key_handles(
    song: &Song,
    project: &Project,
    clip: &str,
    axis: Axis,
    index: usize,
    in_handle: [f64; 2],
    out_handle: [f64; 2],
) -> Edit {
    let mut next = song.clone();
    let k = keys_mut(&mut next, clip, axis)?.get_mut(index).ok_or(Refusal::NotFound)?;
    k.in_handle = [in_handle[0].min(0.0), in_handle[1]];
    k.out_handle = [out_handle[0].max(0.0), out_handle[1]];
    finish(next, project)
}

pub fn set_key_interp(song: &Song, project: &Project, clip: &str, axis: Axis, index: usize, interp: Interp) -> Edit {
    let mut next = song.clone();
    keys_mut(&mut next, clip, axis)?.get_mut(index).ok_or(Refusal::NotFound)?.interp = interp;
    finish(next, project)
}

/// Replace the curves of a keys clip on the given axes, for example after
/// refitting a recorded pass. Axes the clip does not drive are ignored.
pub fn replace_curves(song: &Song, project: &Project, clip: &str, new_curves: BTreeMap<Axis, Vec<Key>>) -> Edit {
    let mut next = song.clone();
    let c = motion_mut(&mut next, clip)?;
    let axes = c.axes.clone();
    let ClipBody::Keys { curves } = &mut c.body else {
        return Err(Refusal::NotKeys);
    };
    for (axis, keys) in new_curves {
        if axes.contains(&axis) {
            curves.insert(axis, keys);
        }
    }
    finish(next, project)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EasyEase {
    Both,
    In,
    Out,
}

/// Flatten a key's handles the way the clip ease presets do: 0.42 of each
/// neighbouring segment's length, with no value offset.
pub fn easy_ease(song: &Song, project: &Project, clip: &str, axis: Axis, index: usize, mode: EasyEase) -> Edit {
    let mut next = song.clone();
    let keys = keys_mut(&mut next, clip, axis)?;
    let k = *keys.get(index).ok_or(Refusal::NotFound)?;
    let prev = index.checked_sub(1).and_then(|i| keys.get(i)).map(|p| (k.frame - p.frame) as f64);
    let after = keys.get(index + 1).map(|n| (n.frame - k.frame) as f64);
    let key = &mut keys[index];
    if mode != EasyEase::Out {
        key.in_handle = [-0.42 * prev.unwrap_or(0.0), 0.0];
    }
    if mode != EasyEase::In {
        key.out_handle = [0.42 * after.unwrap_or(0.0), 0.0];
        key.interp = Interp::Bezier;
    }
    if mode != EasyEase::Out && index > 0 {
        keys[index - 1].interp = Interp::Bezier;
    }
    finish(next, project)
}

/// Delete keys. A clip left with no keys at all is removed.
pub fn delete_keys(song: &Song, project: &Project, clip: &str, selection: &[(Axis, usize)]) -> Edit {
    if selection.is_empty() {
        return Err(Refusal::Empty);
    }
    let mut next = song.clone();
    let mut sorted = selection.to_vec();
    sorted.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (axis, idx) in sorted {
        let keys = keys_mut(&mut next, clip, axis)?;
        if idx < keys.len() {
            keys.remove(idx);
        }
    }
    let empty = match &next.motion_clip(clip).ok_or(Refusal::NotFound)?.body {
        ClipBody::Keys { curves } => curves.values().all(|k| k.is_empty()),
        _ => false,
    };
    if empty {
        next.motion.retain(|c| c.id != clip);
    }
    finish(next, project)
}

pub fn add_marker(song: &Song, project: &Project, frame: i64, name: &str) -> Edit {
    let mut next = song.clone();
    let id = next.next_id("k");
    let frame = frame.max(0);
    next.markers.push(Marker { id, frame, name: name.into() });
    grow(&mut next, frame);
    finish(next, project)
}

pub fn rename_marker(song: &Song, project: &Project, id: &str, name: &str) -> Edit {
    let mut next = song.clone();
    next.markers.iter_mut().find(|m| m.id == id).ok_or(Refusal::NotFound)?.name = name.into();
    finish(next, project)
}

/// Place imported media on the timeline. Returns the new clip's id.
pub fn add_media_clip(
    song: &Song,
    project: &Project,
    media_id: &str,
    name: &str,
    track: Track,
    media_frames: i64,
    start_frame: i64,
) -> Result<(Song, String), Refusal> {
    let mut next = song.clone();
    let id = next.next_id("m");
    let start_frame = start_frame.max(0);
    let media_frames = media_frames.max(1);
    next.media.push(MediaClip {
        id: id.clone(),
        media_id: media_id.into(),
        track,
        start_frame,
        in_frame: 0,
        out_frame: media_frames,
        media_frames,
        name: name.into(),
        muted: false,
    });
    grow(&mut next, start_frame + media_frames);
    Ok((finish(next, project)?, id))
}

pub fn set_media_muted(song: &Song, project: &Project, id: &str, muted: bool) -> Edit {
    let mut next = song.clone();
    next.media_clip_mut(id).ok_or(Refusal::NotFound)?.muted = muted;
    finish(next, project)
}

/// Remove clips and markers by id.
pub fn delete(song: &Song, project: &Project, ids: &[String]) -> Edit {
    if ids.is_empty() {
        return Err(Refusal::Empty);
    }
    let mut next = song.clone();
    next.motion.retain(|c| !ids.contains(&c.id));
    next.media.retain(|c| !ids.contains(&c.id));
    next.markers.retain(|m| !ids.contains(&m.id));
    finish(next, project)
}

/// Copy the selected clips so the copies start `delta` frames later.
/// Returns the new ids.
pub fn copy_clips(song: &Song, project: &Project, ids: &[String], delta: i64) -> Result<(Song, Vec<String>), Refusal> {
    let mut next = song.clone();
    let mut new_ids = Vec::new();
    let mut end = 0;
    for c in song.motion.iter().filter(|c| ids.contains(&c.id)) {
        let mut copy = c.clone();
        copy.id = next.next_id("c");
        copy.start_frame = (copy.start_frame + delta).max(0);
        copy.accepted = None;
        end = end.max(copy.end_frame());
        new_ids.push(copy.id.clone());
        next.motion.push(copy);
    }
    for c in song.media.iter().filter(|c| ids.contains(&c.id)) {
        let mut copy = c.clone();
        copy.id = next.next_id("m");
        copy.start_frame = (copy.start_frame + delta).max(0);
        end = end.max(copy.end_frame());
        new_ids.push(copy.id.clone());
        next.media.push(copy);
    }
    if new_ids.is_empty() {
        return Err(Refusal::Empty);
    }
    grow(&mut next, end);
    Ok((finish(next, project)?, new_ids))
}

/// Cmd-D: copies placed directly after the selection.
pub fn duplicate(song: &Song, project: &Project, ids: &[String]) -> Result<(Song, Vec<String>), Refusal> {
    let spans = song
        .motion
        .iter()
        .filter(|c| ids.contains(&c.id))
        .map(|c| (c.start_frame, c.end_frame()))
        .chain(song.media.iter().filter(|c| ids.contains(&c.id)).map(|c| (c.start_frame, c.end_frame())));
    let (mut lo, mut hi) = (i64::MAX, i64::MIN);
    for (s, e) in spans {
        lo = lo.min(s);
        hi = hi.max(e);
    }
    if lo > hi {
        return Err(Refusal::Empty);
    }
    copy_clips(song, project, ids, hi - lo)
}

pub fn set_song_name(song: &Song, project: &Project, name: &str) -> Edit {
    let mut next = song.clone();
    next.name = name.trim().to_string();
    finish(next, project)
}

pub fn set_song_start(song: &Song, project: &Project, start_tc: &str) -> Edit {
    let tc =
        Timecode::parse_at(start_tc, project.rate).ok_or_else(|| Refusal::Invalid(Invalid::BadStart(start_tc.to_string())))?;
    let mut next = song.clone();
    next.start_tc = tc.to_string();
    finish(next, project)
}

/// Set the song length. It cannot be shorter than its content.
pub fn set_song_length(song: &Song, project: &Project, frames: i64) -> Edit {
    let mut next = song.clone();
    next.length_frames = frames.max(next.content_end()).max(1);
    finish(next, project)
}

/// Record who accepted a too-fast move. Any later change to the clip clears it.
pub fn accept_issues(song: &Song, project: &Project, clip: &str, by: &str, at: &str) -> Edit {
    let mut next = song.clone();
    next.motion_clip_mut(clip).ok_or(Refusal::NotFound)?.accepted = Some(Accepted { by: by.into(), at: at.into() });
    finish(next, project)
}

pub fn clear_acceptance(song: &Song, project: &Project, clip: &str) -> Edit {
    let mut next = song.clone();
    motion_mut(&mut next, clip)?;
    finish(next, project)
}

/// Write a camera's recorded pass onto the timeline. The new keys clip replaces
/// whatever that camera had on its axes over `[start, start + duration)`. A clip that is partly
/// covered is trimmed or split; a preset move becomes keys first so the part
/// that remains is unchanged. Returns the new clip's id.
pub fn punch_keys(
    song: &Song,
    project: &Project,
    camera: &str,
    start: i64,
    duration: i64,
    curves: BTreeMap<Axis, Vec<Key>>,
    pass: Option<String>,
) -> Result<(Song, String), Refusal> {
    if curves.is_empty() || duration < 1 {
        return Err(Refusal::Empty);
    }
    let end = start + duration;
    let rec_axes: Vec<Axis> = curves.keys().copied().collect();
    let mut next = song.clone();

    let hit: Vec<String> = next
        .motion
        .iter()
        .filter(|c| {
            c.camera == camera && c.start_frame < end && start < c.end_frame() && c.axes.iter().any(|a| rec_axes.contains(a))
        })
        .map(|c| c.id.clone())
        .collect();
    for id in &hit {
        to_keys_in_place(&mut next, id)?;
    }
    // Give every shared axis an explicit curve, so a split piece does not
    // start holding the recorded clip's end value instead of its own.
    let ev_values: Vec<(String, Axis, f64)> = {
        let ev = Evaluator::new(&next, camera);
        hit.iter()
            .filter_map(|id| next.motion_clip(id))
            .flat_map(|c| {
                c.axes
                    .iter()
                    .filter(|a| rec_axes.contains(a) && c.keys(**a).is_empty())
                    .map(|a| (c.id.clone(), *a, ev.value_before(c, *a)))
                    .collect::<Vec<_>>()
            })
            .collect()
    };
    for (id, axis, value) in ev_values {
        if let Some(MotionClip { body: ClipBody::Keys { curves }, .. }) = next.motion_clip_mut(&id) {
            curves.insert(axis, vec![Key::new(0, value, Interp::Hold)]);
        }
    }

    for id in &hit {
        let Some(pos) = next.motion.iter().position(|c| &c.id == id) else { continue };
        let old = next.motion[pos].clone();
        let shared: Vec<Axis> = old.axes.iter().copied().filter(|a| rec_axes.contains(a)).collect();
        let ClipBody::Keys { curves: old_curves } = &old.body else { continue };

        let mut left_curves = BTreeMap::new();
        let mut right_curves = BTreeMap::new();
        for &axis in &shared {
            let keys = old_curves.get(&axis).map(|v| v.as_slice()).unwrap_or(&[]);
            if old.start_frame < start {
                let (l, _) = split_keys(keys, start - old.start_frame);
                left_curves.insert(axis, l);
            }
            if old.end_frame() > end {
                let cut = end - old.start_frame;
                let (_, mut r) = split_keys(keys, cut);
                for k in r.iter_mut() {
                    k.frame -= cut;
                }
                right_curves.insert(axis, r);
            }
        }

        // The original clip keeps only the axes that were not recorded.
        {
            let c = &mut next.motion[pos];
            c.accepted = None;
            c.axes.retain(|a| !shared.contains(a));
            if let ClipBody::Keys { curves } = &mut c.body {
                curves.retain(|a, _| !shared.contains(a));
            }
        }
        if next.motion[pos].axes.is_empty() {
            next.motion.remove(pos);
        }
        if !left_curves.is_empty() {
            let id = next.next_id("c");
            next.motion.push(MotionClip {
                id,
                camera: camera.to_string(),
                start_frame: old.start_frame,
                duration_frames: start - old.start_frame,
                axes: shared.clone(),
                body: ClipBody::Keys { curves: left_curves },
                accepted: None,
                recorded_pass: old.recorded_pass.clone(),
            });
        }
        if !right_curves.is_empty() {
            let id = next.next_id("c");
            next.motion.push(MotionClip {
                id,
                camera: camera.to_string(),
                start_frame: end,
                duration_frames: old.end_frame() - end,
                axes: shared.clone(),
                body: ClipBody::Keys { curves: right_curves },
                accepted: None,
                recorded_pass: old.recorded_pass.clone(),
            });
        }
    }

    let id = next.next_id("c");
    next.motion.push(MotionClip {
        id: id.clone(),
        camera: camera.to_string(),
        start_frame: start,
        duration_frames: duration,
        axes: rec_axes,
        body: ClipBody::Keys { curves },
        accepted: None,
        recorded_pass: pass,
    });
    grow(&mut next, end);
    Ok((finish(next, project)?, id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::compile;
    use crate::eval::pose_at;
    use crate::pose::Pose;

    const CAM: &str = crate::camera::FIRST_CAMERA;

    fn preset(name: &str, pan: f64, elevation: f64) -> Preset {
        Preset::new(&name.to_lowercase().replace(' ', "-"), name, Pose { pan, tilt: 2.0, zoom: 0.3, focus: 0.5, elevation })
    }

    fn preset_on(camera: &str, name: &str, pan: f64) -> Preset {
        Preset { camera: camera.into(), ..preset(name, pan, 100.0) }
    }

    fn two_cameras() -> (Song, Project) {
        let (song, mut p) = base();
        p.cameras.push(crate::camera::CameraConfig::new("cam2", "Stage left"));
        (song, p)
    }

    fn base() -> (Song, Project) {
        (Song::new("s", "S", "01:00:00:00", 1500), Project::default())
    }

    #[test]
    fn drop_drag_trim_and_auto() {
        let (song, p) = base();
        let wide = preset("Stage Wide", 20.0, 100.0);
        let (song, a) = insert_preset_move(&song, &p, &wide, 100).unwrap();
        let auto = song.motion_clip(&a).unwrap().duration_frames;
        assert!(auto > 25);
        assert!(compile(&song, &p).issues.is_empty());

        // Dropping inside the move is refused as an overlap.
        let err = insert_preset_move(&song, &p, &wide, 110).unwrap_err();
        assert!(matches!(err, Refusal::Invalid(Invalid::Overlap { .. })));

        let song = move_clip(&song, &p, &a, 300).unwrap();
        assert_eq!(song.motion_clip(&a).unwrap().start_frame, 300);
        assert_eq!(song.motion_clip(&a).unwrap().duration_frames, auto);

        let song = trim_clip_end(&song, &p, &a, 310).unwrap();
        assert_eq!(song.motion_clip(&a).unwrap().duration_frames, 10);
        assert!(!compile(&song, &p).issues.is_empty());

        let song = set_auto_duration(&song, &p, &a).unwrap();
        assert_eq!(song.motion_clip(&a).unwrap().duration_frames, auto);

        let song = trim_clip_start(&song, &p, &a, 290).unwrap();
        let c = song.motion_clip(&a).unwrap();
        assert_eq!((c.start_frame, c.end_frame()), (290, 300 + auto));
    }

    #[test]
    fn linked_move_retimes_when_the_earlier_move_changes() {
        let (song, p) = base();
        let (song, a) = insert_preset_move(&song, &p, &preset("A", 10.0, 0.0), 0).unwrap();
        let (song, b) = insert_preset_move(&song, &p, &preset("B", 20.0, 0.0), 400).unwrap();
        let before = song.motion_clip(&b).unwrap().duration_frames;
        let song = set_to(&song, &p, &a, &preset("A2", -150.0, 0.0)).unwrap();
        let after = song.motion_clip(&b).unwrap().duration_frames;
        assert!(after > before * 2, "{before} -> {after}");
        // Copy X to Y: pin From.
        let song = set_from(&song, &p, &b, FromPose::Pinned(PresetRef::of(&preset("X", 15.0, 0.0)))).unwrap();
        assert!(song.motion_clip(&b).unwrap().duration_frames < after);
    }

    #[test]
    fn convert_to_keys_is_identical_on_every_frame_for_every_ease() {
        let (song, p) = base();
        let (song, _a) = insert_preset_move(&song, &p, &preset("A", 35.0, 250.0), 20).unwrap();
        let (song, b) = insert_preset_move(&song, &p, &preset("B", -60.0, 80.0), 500).unwrap();
        for ease in Ease::ALL {
            let song = set_ease(&song, &p, &b, ease).unwrap();
            let keyed = convert_to_keys(&song, &p, &b).unwrap();
            assert!(keyed.motion_clip(&b).unwrap().is_keys());
            for f in 0..=song.length_frames {
                assert_eq!(pose_at(&song, CAM, f), pose_at(&keyed, CAM, f), "frame {f} ease {ease:?}");
            }
        }
    }

    #[test]
    fn keys_add_move_delete() {
        let (song, p) = base();
        let song = add_key(&song, &p, CAM, &[Axis::Pan, Axis::Tilt], 100).unwrap();
        assert_eq!(song.motion.len(), 1);
        let id = song.motion[0].id.clone();
        // A second K later extends the same clip.
        let song = add_key(&song, &p, CAM, &[Axis::Pan], 220).unwrap();
        assert_eq!(song.motion.len(), 1);
        assert_eq!(song.motion[0].duration_frames, 120);
        assert_eq!(song.motion[0].keys(Axis::Pan).len(), 2);

        let song = move_key(&song, &p, &id, Axis::Pan, 1, 120, 40.0).unwrap();
        assert_eq!(pose_at(&song, CAM, 220).pan, 40.0);
        assert!(pose_at(&song, CAM, 160).pan > 0.0 && pose_at(&song, CAM, 160).pan < 40.0);

        // A key cannot cross its neighbour.
        let song2 = move_key(&song, &p, &id, Axis::Pan, 1, -50, 40.0).unwrap();
        assert_eq!(song2.motion[0].keys(Axis::Pan)[1].frame, 1);

        // Adding a key in the middle of a bezier keeps the curve.
        let mid = add_key(&song, &p, CAM, &[Axis::Pan], 150).unwrap();
        assert_eq!(mid.motion[0].keys(Axis::Pan).len(), 3);
        for f in 100..=220 {
            assert!((pose_at(&song, CAM, f).pan - pose_at(&mid, CAM, f).pan).abs() < 1e-7);
        }

        let song = delete_keys(&song, &p, &id, &[(Axis::Pan, 0), (Axis::Pan, 1), (Axis::Tilt, 0)]).unwrap();
        assert!(song.motion.is_empty());
    }

    #[test]
    fn key_under_a_preset_move_asks_for_conversion() {
        let (song, p) = base();
        let (song, _) = insert_preset_move(&song, &p, &preset("A", 35.0, 250.0), 20).unwrap();
        assert_eq!(add_key(&song, &p, CAM, &[Axis::Pan], 30).unwrap_err(), Refusal::ConvertFirst(Axis::Pan));
    }

    #[test]
    fn punch_replaces_only_the_recorded_axis_and_range() {
        let (song, p) = base();
        let (song, a) = insert_preset_move(&song, &p, &preset("A", 60.0, 400.0), 100).unwrap();
        let song = set_custom_duration(&song, &p, &a, 400).unwrap();
        let (song, _) = insert_preset_move(&song, &p, &preset("B", -20.0, 50.0), 700).unwrap();

        let mut curves = BTreeMap::new();
        curves.insert(Axis::Pan, vec![Key::new(0, 5.0, Interp::Linear), Key::new(100, 9.0, Interp::Linear)]);
        let (punched, id) = punch_keys(&song, &p, CAM, 200, 100, curves, Some("pass1".into())).unwrap();
        assert!(punched.validate(p.rate).is_ok());
        assert_eq!(punched.motion_clip(&id).unwrap().axes, vec![Axis::Pan]);

        for f in 0..=song.length_frames {
            let (o, n) = (pose_at(&song, CAM, f), pose_at(&punched, CAM, f));
            for axis in [Axis::Tilt, Axis::Zoom, Axis::Focus, Axis::Elevation] {
                assert!((o.get(axis) - n.get(axis)).abs() < 1e-7, "{axis:?} changed at {f}");
            }
            if !(200..300).contains(&f) {
                assert!((o.pan - n.pan).abs() < 1e-6, "pan changed outside the pass at {f}: {} vs {}", o.pan, n.pan);
            }
        }
        assert_eq!(pose_at(&punched, CAM, 200).pan, 5.0);
        assert!((pose_at(&punched, CAM, 250).pan - 7.0).abs() < 1e-9);
    }

    #[test]
    fn media_clip_trim_is_clamped_to_its_source() {
        let (song, p) = base();
        let (song, id) = add_media_clip(&song, &p, "m1", "click", Track::Audio, 310, 50).unwrap();
        let song = trim_clip_end(&song, &p, &id, 5000).unwrap();
        assert_eq!(song.media_clip(&id).unwrap().out_frame, 310);
        let song = trim_clip_start(&song, &p, &id, 80).unwrap();
        let c = song.media_clip(&id).unwrap();
        assert_eq!((c.start_frame, c.in_frame, c.end_frame()), (80, 30, 360));
        let song = trim_clip_start(&song, &p, &id, 0).unwrap();
        let c = song.media_clip(&id).unwrap();
        assert_eq!((c.start_frame, c.in_frame), (50, 0));
    }

    #[test]
    fn duplicate_and_delete_and_markers() {
        let (song, p) = base();
        let (song, a) = insert_preset_move(&song, &p, &preset("A", 35.0, 250.0), 20).unwrap();
        let (song, ids) = duplicate(&song, &p, std::slice::from_ref(&a)).unwrap();
        assert_eq!(song.motion.len(), 2);
        assert_eq!(song.motion_clip(&ids[0]).unwrap().start_frame, song.motion_clip(&a).unwrap().end_frame());
        let song = add_marker(&song, &p, 250, "Doors").unwrap();
        let mid = song.markers[0].id.clone();
        let song = move_clip(&song, &p, &mid, 300).unwrap();
        assert_eq!(song.markers[0].frame, 300);
        let song = delete(&song, &p, &[a, mid]).unwrap();
        assert_eq!((song.motion.len(), song.markers.len()), (1, 0));
    }

    #[test]
    fn random_edit_sequences_never_overlap_an_axis() {
        let (mut song, p) = base();
        let presets = [preset("A", 35.0, 250.0), preset("B", -60.0, 80.0), preset("C", 120.0, 600.0)];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut rnd = |n: i64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n as u64) as i64
        };
        let mut applied = 0;
        for _ in 0..3000 {
            let pick =
                if song.motion.is_empty() { None } else { Some(song.motion[rnd(song.motion.len() as i64) as usize].id.clone()) };
            let result = match (rnd(9), pick) {
                (0, _) | (_, None) => insert_preset_move(&song, &p, &presets[rnd(3) as usize], rnd(1400)).map(|r| r.0),
                (1, Some(id)) => move_clip(&song, &p, &id, rnd(1400)),
                (2, Some(id)) => trim_clip_end(&song, &p, &id, rnd(1500)),
                (3, Some(id)) => trim_clip_start(&song, &p, &id, rnd(1400)),
                (4, Some(id)) => convert_to_keys(&song, &p, &id),
                (5, Some(id)) => set_axes(&song, &p, &id, &[Axis::ALL[rnd(5) as usize], Axis::ALL[rnd(5) as usize]]),
                (6, Some(id)) => duplicate(&song, &p, &[id]).map(|r| r.0),
                (7, Some(_)) => add_key(&song, &p, CAM, &[Axis::ALL[rnd(5) as usize]], rnd(1400)),
                (_, Some(id)) => delete(&song, &p, &[id]),
            };
            if let Ok(next) = result {
                assert!(next.validate(p.rate).is_ok());
                song = next;
                applied += 1;
            }
            // The song the refusal left behind is still valid.
            assert!(song.validate(p.rate).is_ok());
        }
        assert!(applied > 500, "only {applied} edits applied");
    }

    #[test]
    fn a_dropped_preset_makes_a_move_for_its_own_camera() {
        let (song, p) = two_cameras();
        let (song, a) = insert_preset_move(&song, &p, &preset("Wide", 20.0, 100.0), 100).unwrap();
        // The second camera may move over the same frames.
        let (song, b) = insert_preset_move(&song, &p, &preset_on("cam2", "Keys", -30.0), 110).unwrap();
        assert_eq!(song.motion_clip(&a).unwrap().camera, "cam1");
        assert_eq!(song.motion_clip(&b).unwrap().camera, "cam2");
        assert_eq!(pose_at(&song, CAM, 1000).pan, 20.0);
        assert_eq!(pose_at(&song, "cam2", 1000).pan, -30.0);
        // But not twice itself.
        let err = insert_preset_move(&song, &p, &preset_on("cam2", "Drums", 5.0), 115).unwrap_err();
        assert!(matches!(err, Refusal::Invalid(Invalid::Overlap { .. })));

        // The drop preview names the preset this camera's last move ended on.
        let preview = drop_preview(&song, &p, &preset_on("cam2", "Drums", 5.0), 900).unwrap();
        assert_eq!(preview.from_name.as_deref(), Some("Keys"));
        let preview = drop_preview(&song, &p, &preset("Close", 5.0, 100.0), 900).unwrap();
        assert_eq!(preview.from_name.as_deref(), Some("Wide"));

        // A move's target stays a preset of its camera.
        assert_eq!(set_to(&song, &p, &a, &preset_on("cam2", "Keys", -30.0)).unwrap_err(), Refusal::OtherCamera);
        assert_eq!(Refusal::OtherCamera.to_string(), "That preset belongs to another camera");
        assert!(set_to(&song, &p, &b, &preset_on("cam2", "Drums", 5.0)).is_ok());
    }

    #[test]
    fn keys_and_recorded_passes_touch_one_camera() {
        let (song, p) = two_cameras();
        let (song, a) = insert_preset_move(&song, &p, &preset("A", 60.0, 400.0), 100).unwrap();
        let song = set_custom_duration(&song, &p, &a, 400).unwrap();

        // K on the second camera, in the middle of the first camera's move, starts a keys clip there.
        let keyed = add_key(&song, &p, "cam2", &[Axis::Pan], 200).unwrap();
        assert_eq!(keyed.motion.len(), 2);
        assert_eq!(keyed.motion[1].camera, "cam2");
        assert_eq!(add_key(&song, &p, CAM, &[Axis::Pan], 200).unwrap_err(), Refusal::ConvertFirst(Axis::Pan));
        let keyed = add_key(&keyed, &p, "cam2", &[Axis::Pan], 300).unwrap();
        assert_eq!(keyed.motion.len(), 2, "the second K extends that camera's clip");
        assert_eq!(keyed.motion[1].keys(Axis::Pan).len(), 2);

        // A pass recorded on the second camera leaves the first camera's move whole.
        let mut curves = BTreeMap::new();
        curves.insert(Axis::Pan, vec![Key::new(0, 5.0, Interp::Linear), Key::new(100, 9.0, Interp::Linear)]);
        let (punched, id) = punch_keys(&song, &p, "cam2", 200, 100, curves.clone(), Some("pass1".into())).unwrap();
        assert_eq!(punched.motion_clip(&id).unwrap().camera, "cam2");
        assert!(punched.motion_clip(&a).unwrap().is_preset_move());
        for f in 0..=song.length_frames {
            assert_eq!(pose_at(&song, CAM, f), pose_at(&punched, CAM, f));
        }
        assert_eq!(pose_at(&punched, "cam2", 250).pan, 7.0);

        // The same pass on the first camera splits its move, and the pieces stay on that camera.
        let (punched, _) = punch_keys(&song, &p, CAM, 200, 100, curves, None).unwrap();
        assert!(punched.motion.len() >= 3);
        assert!(punched.motion.iter().all(|c| c.camera == CAM));
    }

    #[test]
    fn clips_can_be_handed_to_another_camera_where_it_is_free() {
        let (song, p) = two_cameras();
        let (song, a) = insert_preset_move(&song, &p, &preset("A", 60.0, 400.0), 100).unwrap();
        let (song, b) = insert_preset_move(&song, &p, &preset_on("cam2", "B", 10.0), 120).unwrap();
        assert!(matches!(
            set_camera(&song, &p, std::slice::from_ref(&a), "cam2"),
            Err(Refusal::Invalid(Invalid::Overlap { .. }))
        ));
        let song = move_clip(&song, &p, &b, 900).unwrap();
        let moved = set_camera(&song, &p, std::slice::from_ref(&a), "cam2").unwrap();
        assert_eq!(moved.motion_clip(&a).unwrap().camera, "cam2");
        assert_eq!(pose_at(&moved, CAM, 800), moved.home_pose);
        assert_eq!(set_camera(&song, &p, &[a], "cam5").unwrap_err(), Refusal::NoSuchCamera);
        assert_eq!(set_camera(&song, &p, &["nothing".into()], "cam2").unwrap_err(), Refusal::Empty);

        // Copies stay on their camera.
        let (copied, ids) = duplicate(&song, &p, std::slice::from_ref(&b)).unwrap();
        assert_eq!(copied.motion_clip(&ids[0]).unwrap().camera, "cam2");
    }
}
