//! The timeline's geometry and gestures. Everything here is a pure function of
//! the song and the view, so the same code runs in the window and in tests.

use egui::{pos2, Pos2, Rect};
use ptz_core::edit::{self, Refusal};
use ptz_core::pose::{Axis, Preset, Project};
use ptz_core::song::{ClipBody, Song, Track};
use std::collections::BTreeSet;

pub const HEADER_W: f32 = 132.0;
pub const RULER_H: f32 = 26.0;
pub const SNAP_PX: f32 = 6.0;
const EDGE_PX: f32 = 7.0;
const KEY_PX: f32 = 6.0;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Lane {
    Markers,
    Video,
    Audio,
    Motion,
    Axis(Axis),
}

impl Lane {
    pub fn label(self) -> &'static str {
        match self {
            Lane::Markers => "Markers",
            Lane::Video => "Reference video",
            Lane::Audio => "Audio",
            Lane::Motion => "Camera move",
            Lane::Axis(a) => a.label(),
        }
    }

    fn height(self) -> f32 {
        match self {
            Lane::Markers => 22.0,
            Lane::Video => 44.0,
            Lane::Audio => 56.0,
            Lane::Motion => 46.0,
            Lane::Axis(_) => 30.0,
        }
    }

    pub fn takes_motion(self) -> bool {
        matches!(self, Lane::Motion | Lane::Axis(_))
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct View {
    pub px_per_frame: f32,
    /// Frame at the left edge of the lanes.
    pub scroll: f64,
    /// Camera move twirled open to the five axis lanes.
    pub motion_open: bool,
}

impl Default for View {
    fn default() -> Self {
        View { px_per_frame: 1.0, scroll: 0.0, motion_open: false }
    }
}

impl View {
    pub const MIN_PPF: f32 = 0.02;
    pub const MAX_PPF: f32 = 40.0;

    /// Show the whole song with a little room at the end.
    pub fn fit(&mut self, length: i64, body_width: f32) {
        let frames = (length.max(25) as f32) * 1.04;
        self.px_per_frame = ((body_width - 8.0).max(50.0) / frames).clamp(Self::MIN_PPF, Self::MAX_PPF);
        self.scroll = 0.0;
    }

    /// Zoom by `factor`, keeping the frame under `x` (pixels from the left of the lanes) still.
    pub fn zoom_around(&mut self, factor: f32, x: f32) {
        let frame = self.scroll + (x / self.px_per_frame) as f64;
        self.px_per_frame = (self.px_per_frame * factor).clamp(Self::MIN_PPF, Self::MAX_PPF);
        self.scroll = frame - (x / self.px_per_frame) as f64;
    }

    pub fn clamp(&mut self, length: i64, body_width: f32) {
        let visible = (body_width / self.px_per_frame) as f64;
        let max = (length as f64 * 1.04 - visible * 0.5).max(0.0);
        self.scroll = self.scroll.clamp(0.0, max);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClipKind {
    Move,
    Keys,
    Audio,
    Video,
    /// A clip's presence on one axis lane.
    AxisPart,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ClipBox {
    pub id: String,
    pub lane: Lane,
    pub kind: ClipKind,
    pub rect: Rect,
    pub start: i64,
    pub end: i64,
}

#[derive(Clone, PartialEq, Debug)]
pub struct MarkerBox {
    pub id: String,
    pub frame: i64,
    pub x: f32,
}

#[derive(Clone, PartialEq, Debug)]
pub struct KeyBox {
    pub clip: String,
    pub axis: Axis,
    pub index: usize,
    pub pos: Pos2,
}

#[derive(Clone, PartialEq, Debug)]
pub struct LaneBox {
    pub lane: Lane,
    /// Full row including the header column.
    pub rect: Rect,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Layout {
    pub rect: Rect,
    pub ruler: Rect,
    /// The lane area right of the header column, below the ruler.
    pub body: Rect,
    pub twirl: Rect,
    pub lanes: Vec<LaneBox>,
    pub clips: Vec<ClipBox>,
    pub markers: Vec<MarkerBox>,
    pub keys: Vec<KeyBox>,
    pub px_per_frame: f32,
    pub scroll: f64,
    pub length: i64,
}

impl Layout {
    pub fn x_of(&self, frame: f64) -> f32 {
        self.body.left() + ((frame - self.scroll) * self.px_per_frame as f64) as f32
    }

    pub fn frame_at(&self, x: f32) -> f64 {
        self.scroll + ((x - self.body.left()) / self.px_per_frame) as f64
    }

    pub fn lane_rect(&self, lane: Lane) -> Option<Rect> {
        self.lanes.iter().find(|l| l.lane == lane).map(|l| l.rect)
    }

    pub fn lane_at(&self, y: f32) -> Option<Lane> {
        self.lanes.iter().find(|l| l.rect.top() <= y && y < l.rect.bottom()).map(|l| l.lane)
    }

    pub fn clip(&self, id: &str) -> Option<&ClipBox> {
        self.clips.iter().find(|c| c.id == id && c.kind != ClipKind::AxisPart)
    }

    pub fn content_height(view: &View) -> f32 {
        let mut h = RULER_H;
        for lane in lanes(view) {
            h += lane.height();
        }
        h
    }
}

fn lanes(view: &View) -> Vec<Lane> {
    let mut out = vec![Lane::Markers, Lane::Video, Lane::Audio, Lane::Motion];
    if view.motion_open {
        out.extend(Axis::ALL.iter().map(|a| Lane::Axis(*a)));
    }
    out
}

pub fn layout(song: &Song, view: &View, rect: Rect) -> Layout {
    let ruler = Rect::from_min_max(pos2(rect.left() + HEADER_W, rect.top()), pos2(rect.right(), rect.top() + RULER_H));
    let body = Rect::from_min_max(pos2(rect.left() + HEADER_W, ruler.bottom()), rect.right_bottom());
    let mut out = Layout {
        rect,
        ruler,
        body,
        twirl: Rect::NOTHING,
        lanes: Vec::new(),
        clips: Vec::new(),
        markers: Vec::new(),
        keys: Vec::new(),
        px_per_frame: view.px_per_frame,
        scroll: view.scroll,
        length: song.length_frames,
    };
    let mut y = ruler.bottom();
    for lane in lanes(view) {
        let r = Rect::from_min_max(pos2(rect.left(), y), pos2(rect.right(), y + lane.height()));
        if lane == Lane::Motion {
            out.twirl = Rect::from_min_size(pos2(rect.left() + 4.0, y + 4.0), egui::vec2(18.0, lane.height() - 8.0));
        }
        out.lanes.push(LaneBox { lane, rect: r });
        y += lane.height();
    }

    let span = |out: &Layout, lane_rect: Rect, start: i64, end: i64, row: usize, rows: usize| -> Rect {
        let h = (lane_rect.height() - 6.0) / rows as f32;
        let top = lane_rect.top() + 3.0 + h * row as f32;
        Rect::from_min_max(
            pos2(out.x_of(start as f64), top),
            pos2(out.x_of(end as f64), top + h - if rows > 1 { 1.0 } else { 0.0 }),
        )
    };

    for m in &song.markers {
        out.markers.push(MarkerBox { id: m.id.clone(), frame: m.frame, x: out.x_of(m.frame as f64) });
    }

    for c in &song.media {
        let (lane, kind) = match c.track {
            Track::Audio => (Lane::Audio, ClipKind::Audio),
            Track::Video => (Lane::Video, ClipKind::Video),
        };
        if let Some(lr) = out.lane_rect(lane) {
            let rect = span(&out, lr, c.start_frame, c.end_frame(), 0, 1);
            out.clips.push(ClipBox { id: c.id.clone(), lane, kind, rect, start: c.start_frame, end: c.end_frame() });
        }
    }

    // Moves on different axes may overlap in time; give each its own row.
    let mut order: Vec<usize> = (0..song.motion.len()).collect();
    order.sort_by_key(|i| (song.motion[*i].start_frame, song.motion[*i].id.clone()));
    let mut row_ends: Vec<i64> = Vec::new();
    let mut rows: Vec<usize> = vec![0; song.motion.len()];
    for i in &order {
        let c = &song.motion[*i];
        let row = row_ends.iter().position(|end| *end <= c.start_frame).unwrap_or(row_ends.len());
        if row == row_ends.len() {
            row_ends.push(c.end_frame());
        } else {
            row_ends[row] = c.end_frame();
        }
        rows[*i] = row;
    }
    let row_count = row_ends.len().max(1);
    if let Some(lr) = out.lane_rect(Lane::Motion) {
        for i in &order {
            let c = &song.motion[*i];
            let rect = span(&out, lr, c.start_frame, c.end_frame(), rows[*i], row_count);
            let kind = if c.is_keys() { ClipKind::Keys } else { ClipKind::Move };
            out.clips.push(ClipBox {
                id: c.id.clone(),
                lane: Lane::Motion,
                kind,
                rect,
                start: c.start_frame,
                end: c.end_frame(),
            });
        }
    }
    if view.motion_open {
        for i in &order {
            let c = &song.motion[*i];
            for axis in &c.axes {
                let Some(lr) = out.lane_rect(Lane::Axis(*axis)) else { continue };
                let rect = span(&out, lr, c.start_frame, c.end_frame(), 0, 1);
                out.clips.push(ClipBox {
                    id: c.id.clone(),
                    lane: Lane::Axis(*axis),
                    kind: ClipKind::AxisPart,
                    rect,
                    start: c.start_frame,
                    end: c.end_frame(),
                });
                if let ClipBody::Keys { curves } = &c.body {
                    for (index, k) in curves.get(axis).map(|v| v.as_slice()).unwrap_or(&[]).iter().enumerate() {
                        out.keys.push(KeyBox {
                            clip: c.id.clone(),
                            axis: *axis,
                            index,
                            pos: pos2(out.x_of((c.start_frame + k.frame) as f64), lr.center().y),
                        });
                    }
                }
            }
        }
    }
    out
}

#[derive(Clone, PartialEq, Debug)]
pub enum Hit {
    ClipBody { id: String },
    ClipStart { id: String },
    ClipEnd { id: String },
    Key { clip: String, axis: Axis, index: usize },
    Ruler { frame: i64 },
    Marker { id: String },
    Twirl,
    Header { lane: Lane },
    Empty { lane: Option<Lane>, frame: i64 },
    Outside,
}

pub fn hit(layout: &Layout, pos: Pos2) -> Hit {
    if !layout.rect.contains(pos) {
        return Hit::Outside;
    }
    let frame = layout.frame_at(pos.x).round() as i64;
    if pos.y < layout.ruler.bottom() {
        return if pos.x >= layout.body.left() { Hit::Ruler { frame: frame.clamp(0, layout.length) } } else { Hit::Outside };
    }
    let lane = layout.lane_at(pos.y);
    if pos.x < layout.body.left() {
        if layout.twirl.contains(pos) {
            return Hit::Twirl;
        }
        return match lane {
            Some(lane) => Hit::Header { lane },
            None => Hit::Outside,
        };
    }
    if lane == Some(Lane::Markers) {
        let nearest = layout.markers.iter().min_by(|a, b| (a.x - pos.x).abs().total_cmp(&(b.x - pos.x).abs()));
        if let Some(m) = nearest.filter(|m| (m.x - pos.x).abs() <= 7.0) {
            return Hit::Marker { id: m.id.clone() };
        }
    }
    if let Some(k) = layout
        .keys
        .iter()
        .filter(|k| (k.pos.x - pos.x).abs() <= KEY_PX && (k.pos.y - pos.y).abs() <= KEY_PX + 4.0)
        .min_by(|a, b| (a.pos.x - pos.x).abs().total_cmp(&(b.pos.x - pos.x).abs()))
    {
        return Hit::Key { clip: k.clip.clone(), axis: k.axis, index: k.index };
    }
    // Later boxes are painted on top, so test them first.
    for c in layout.clips.iter().rev() {
        if !c.rect.expand2(egui::vec2(0.5, 0.0)).contains(pos) {
            continue;
        }
        let zone = EDGE_PX.min(c.rect.width() / 3.0);
        let id = c.id.clone();
        return if pos.x >= c.rect.right() - zone {
            Hit::ClipEnd { id }
        } else if pos.x <= c.rect.left() + zone {
            Hit::ClipStart { id }
        } else {
            Hit::ClipBody { id }
        };
    }
    Hit::Empty { lane, frame: frame.max(0) }
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Mods {
    pub shift: bool,
    pub alt: bool,
    pub command: bool,
}

/// What the gesture code needs to know about the document.
pub struct Doc<'a> {
    pub song: &'a Song,
    pub project: &'a Project,
    pub playhead: f64,
}

#[derive(Clone, PartialEq, Debug)]
enum Drag {
    Move {
        ids: Vec<String>,
        primary: String,
        grab_frame: f64,
        copy: bool,
        moved: bool,
    },
    /// `grab` is the edge frame minus the frame under the pointer when the drag began.
    TrimEnd {
        id: String,
        grab: f64,
    },
    TrimStart {
        id: String,
        grab: f64,
    },
    Marquee {
        origin: Pos2,
        current: Pos2,
        base: BTreeSet<String>,
        base_keys: BTreeSet<KeyRef>,
    },
    Scrub,
    Keys {
        clip: String,
        grab_frame: f64,
    },
}

#[derive(Clone, PartialEq, Debug)]
pub enum Outcome {
    None,
    Seek(i64),
    /// A finished gesture: the new song, and what to select afterwards.
    Edit {
        song: Song,
        select: Option<Vec<String>>,
    },
    Refused(String),
}

pub type KeyRef = (String, Axis, usize);

/// A drop that would be refused, drawn in the warning colour.
#[derive(Clone, PartialEq, Debug)]
pub struct Attempt {
    pub ids: Vec<String>,
    pub delta: i64,
    pub reason: String,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Ghost {
    pub lane: Lane,
    pub start: i64,
    pub duration: i64,
    pub label: String,
    /// Why releasing here would do nothing.
    pub refusal: Option<String>,
}

#[derive(Default)]
pub struct Timeline {
    pub view: View,
    pub selection: BTreeSet<String>,
    pub key_selection: BTreeSet<KeyRef>,
    drag: Option<Drag>,
    /// The song as it will be if the drag is released now.
    pub preview: Option<Song>,
    preview_ids: Option<Vec<String>>,
    pub attempt: Option<Attempt>,
    /// Frame an edge snapped to, for the snap line.
    pub snap_frame: Option<i64>,
}

fn secs_text(frames: i64, project: &Project) -> String {
    format!("{:.1} s", project.rate.frames_to_seconds(frames as f64))
}

/// "Stage Wide to Lectern MCU, 2.4 s"
pub fn move_label(song: &Song, clip_id: &str, project: &Project) -> String {
    let Some(clip) = song.motion_clip(clip_id) else { return String::new() };
    match &clip.body {
        ClipBody::PresetMove { from, to, .. } => {
            let from_name = match from {
                ptz_core::song::FromPose::Pinned(p) => Some(p.name.clone()),
                ptz_core::song::FromPose::Linked(_) => {
                    song.motion.iter().filter(|c| c.end_frame() <= clip.start_frame).max_by_key(|c| c.end_frame()).and_then(|c| {
                        match &c.body {
                            ClipBody::PresetMove { to, .. } => Some(to.name.clone()),
                            _ => None,
                        }
                    })
                }
            };
            match from_name {
                Some(f) => format!("{f} to {}, {}", to.name, secs_text(clip.duration_frames, project)),
                None => format!("To {}, {}", to.name, secs_text(clip.duration_frames, project)),
            }
        }
        ClipBody::Keys { .. } => format!("{}, {}", clip.label(), secs_text(clip.duration_frames, project)),
    }
}

impl Timeline {
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn is_scrubbing(&self) -> bool {
        matches!(self.drag, Some(Drag::Scrub))
    }

    pub fn marquee(&self) -> Option<Rect> {
        match &self.drag {
            Some(Drag::Marquee { origin, current, .. }) => Some(Rect::from_two_pos(*origin, *current)),
            _ => None,
        }
    }

    pub fn cancel(&mut self) {
        self.drag = None;
        self.preview = None;
        self.preview_ids = None;
        self.attempt = None;
        self.snap_frame = None;
    }

    /// Drop selected ids that no longer exist.
    pub fn prune(&mut self, song: &Song) {
        self.selection.retain(|id| {
            song.motion_clip(id).is_some() || song.media_clip(id).is_some() || song.markers.iter().any(|m| &m.id == id)
        });
        self.key_selection.retain(|(clip, axis, index)| song.motion_clip(clip).is_some_and(|c| c.keys(*axis).len() > *index));
    }

    fn snap_targets(&self, doc: &Doc, moving: &[String]) -> Vec<i64> {
        let mut t = vec![doc.playhead.round() as i64, 0];
        t.extend(doc.song.markers.iter().filter(|m| !moving.contains(&m.id)).map(|m| m.frame));
        for c in doc.song.motion.iter().filter(|c| !moving.contains(&c.id)) {
            t.push(c.start_frame);
            t.push(c.end_frame());
        }
        for c in doc.song.media.iter().filter(|c| !moving.contains(&c.id)) {
            t.push(c.start_frame);
            t.push(c.end_frame());
        }
        t
    }

    fn snap(frame: i64, targets: &[i64], ppf: f32) -> Option<i64> {
        targets.iter().copied().filter(|t| ((*t - frame).abs() as f32) * ppf <= SNAP_PX).min_by_key(|t| (*t - frame).abs())
    }

    fn span_of(song: &Song, id: &str) -> Option<(i64, i64)> {
        if let Some(c) = song.motion_clip(id) {
            Some((c.start_frame, c.end_frame()))
        } else if let Some(c) = song.media_clip(id) {
            Some((c.start_frame, c.end_frame()))
        } else {
            song.markers.iter().find(|m| m.id == id).map(|m| (m.frame, m.frame))
        }
    }

    pub fn press(&mut self, layout: &Layout, _doc: &Doc, pos: Pos2, mods: Mods) -> Outcome {
        self.cancel();
        match hit(layout, pos) {
            Hit::Ruler { frame } => {
                self.drag = Some(Drag::Scrub);
                Outcome::Seek(frame)
            }
            Hit::Twirl => {
                self.view.motion_open = !self.view.motion_open;
                Outcome::None
            }
            Hit::ClipBody { id } | Hit::Marker { id } => {
                self.key_selection.clear();
                if mods.shift {
                    if !self.selection.remove(&id) {
                        self.selection.insert(id.clone());
                    }
                } else if !self.selection.contains(&id) {
                    self.selection.clear();
                    self.selection.insert(id.clone());
                }
                if self.selection.contains(&id) {
                    self.drag = Some(Drag::Move {
                        ids: self.selection.iter().cloned().collect(),
                        primary: id,
                        grab_frame: layout.frame_at(pos.x),
                        copy: mods.alt,
                        moved: false,
                    });
                }
                Outcome::None
            }
            Hit::ClipEnd { id } => {
                self.select_only(&id);
                let grab = layout.clip(&id).map(|c| c.end as f64 - layout.frame_at(pos.x)).unwrap_or(0.0);
                self.drag = Some(Drag::TrimEnd { id, grab });
                Outcome::None
            }
            Hit::ClipStart { id } => {
                self.select_only(&id);
                let grab = layout.clip(&id).map(|c| c.start as f64 - layout.frame_at(pos.x)).unwrap_or(0.0);
                self.drag = Some(Drag::TrimStart { id, grab });
                Outcome::None
            }
            Hit::Key { clip, axis, index } => {
                let key = (clip.clone(), axis, index);
                if mods.shift {
                    if !self.key_selection.remove(&key) {
                        self.key_selection.insert(key);
                    }
                } else if !self.key_selection.contains(&key) {
                    self.key_selection.clear();
                    self.key_selection.insert(key);
                }
                self.key_selection.retain(|k| k.0 == clip);
                self.select_only(&clip);
                self.drag = Some(Drag::Keys { clip, grab_frame: layout.frame_at(pos.x) });
                Outcome::None
            }
            Hit::Empty { .. } => {
                if !mods.shift {
                    self.selection.clear();
                    self.key_selection.clear();
                }
                self.drag = Some(Drag::Marquee {
                    origin: pos,
                    current: pos,
                    base: self.selection.clone(),
                    base_keys: self.key_selection.clone(),
                });
                Outcome::None
            }
            Hit::Header { .. } | Hit::Outside => Outcome::None,
        }
    }

    fn select_only(&mut self, id: &str) {
        self.selection.clear();
        self.selection.insert(id.to_string());
    }

    fn set_result(&mut self, result: Result<Song, Refusal>, ids: &[String], delta: i64) {
        match result {
            Ok(song) => {
                self.preview = Some(song);
                self.attempt = None;
            }
            Err(r) => {
                self.preview = None;
                self.attempt = Some(Attempt { ids: ids.to_vec(), delta, reason: r.to_string() });
            }
        }
    }

    pub fn drag_to(&mut self, layout: &Layout, doc: &Doc, pos: Pos2, mods: Mods) -> Outcome {
        let Some(drag) = self.drag.clone() else { return Outcome::None };
        let ppf = layout.px_per_frame;
        self.snap_frame = None;
        match drag {
            Drag::Scrub => Outcome::Seek((layout.frame_at(pos.x).round() as i64).clamp(0, layout.length)),
            Drag::Move { ids, primary, grab_frame, copy, .. } => {
                let mut delta = (layout.frame_at(pos.x) - grab_frame).round() as i64;
                let copy = copy || mods.alt;
                if let (Some((start, end)), false) = (Self::span_of(doc.song, &primary), mods.shift) {
                    let moving: Vec<String> = if copy { Vec::new() } else { ids.clone() };
                    let targets = self.snap_targets(doc, &moving);
                    let by_start = Self::snap(start + delta, &targets, ppf).map(|t| (t, t - start));
                    let by_end = Self::snap(end + delta, &targets, ppf).map(|t| (t, t - end));
                    let best = [by_start, by_end].into_iter().flatten().min_by_key(|(_, d)| (d - delta).abs());
                    if let Some((target, d)) = best {
                        delta = d;
                        self.snap_frame = Some(target);
                    }
                }
                self.drag = Some(Drag::Move { ids: ids.clone(), primary, grab_frame, copy, moved: delta != 0 });
                self.preview_ids = None;
                if delta == 0 {
                    self.preview = None;
                    self.attempt = None;
                } else if copy {
                    match edit::copy_clips(doc.song, doc.project, &ids, delta) {
                        Ok((song, new_ids)) => {
                            self.preview = Some(song);
                            self.preview_ids = Some(new_ids);
                            self.attempt = None;
                        }
                        Err(r) => self.set_result(Err(r), &ids, delta),
                    }
                } else {
                    self.set_result(edit::move_clips(doc.song, doc.project, &ids, delta), &ids, delta);
                }
                Outcome::None
            }
            Drag::TrimEnd { id, grab } => {
                let Some((start, end)) = Self::span_of(doc.song, &id) else { return Outcome::None };
                let mut new_end = ((layout.frame_at(pos.x) + grab).round() as i64).max(start + 1);
                if !mods.shift {
                    let targets = self.snap_targets(doc, std::slice::from_ref(&id));
                    if let Some(t) = Self::snap(new_end, &targets, ppf).filter(|t| *t > start) {
                        new_end = t;
                        self.snap_frame = Some(t);
                    }
                }
                if new_end == end {
                    self.preview = None;
                    self.attempt = None;
                } else {
                    let result = edit::trim_clip_end(doc.song, doc.project, &id, new_end);
                    self.set_result(result, std::slice::from_ref(&id), 0);
                }
                Outcome::None
            }
            Drag::TrimStart { id, grab } => {
                let Some((start, end)) = Self::span_of(doc.song, &id) else { return Outcome::None };
                let mut new_start = ((layout.frame_at(pos.x) + grab).round() as i64).clamp(0, end - 1);
                if !mods.shift {
                    let targets = self.snap_targets(doc, std::slice::from_ref(&id));
                    if let Some(t) = Self::snap(new_start, &targets, ppf).filter(|t| *t < end) {
                        new_start = t;
                        self.snap_frame = Some(t);
                    }
                }
                if new_start == start {
                    self.preview = None;
                    self.attempt = None;
                } else {
                    let result = edit::trim_clip_start(doc.song, doc.project, &id, new_start);
                    self.set_result(result, std::slice::from_ref(&id), 0);
                }
                Outcome::None
            }
            Drag::Marquee { origin, base, base_keys, .. } => {
                let area = Rect::from_two_pos(origin, pos);
                let mut sel = base.clone();
                for c in layout.clips.iter().filter(|c| c.kind != ClipKind::AxisPart) {
                    if c.rect.intersects(area) {
                        sel.insert(c.id.clone());
                    }
                }
                if let Some(lr) = layout.lane_rect(Lane::Markers) {
                    if lr.intersects(area) {
                        for m in layout.markers.iter().filter(|m| area.left() <= m.x && m.x <= area.right()) {
                            sel.insert(m.id.clone());
                        }
                    }
                }
                self.selection = sel;
                // Keys on the axis lanes: all from one clip, so they can be moved together.
                let mut keys = base_keys.clone();
                for k in layout.keys.iter().filter(|k| area.contains(k.pos)) {
                    if keys.iter().all(|other| other.0 == k.clip) {
                        keys.insert((k.clip.clone(), k.axis, k.index));
                    }
                }
                self.key_selection = keys;
                self.drag = Some(Drag::Marquee { origin, current: pos, base, base_keys });
                Outcome::None
            }
            Drag::Keys { clip, grab_frame } => {
                let delta = (layout.frame_at(pos.x) - grab_frame).round() as i64;
                if delta == 0 {
                    self.preview = None;
                    self.attempt = None;
                } else {
                    let sel: Vec<(Axis, usize)> = self.key_selection.iter().filter(|k| k.0 == clip).map(|k| (k.1, k.2)).collect();
                    let result = edit::shift_keys(doc.song, doc.project, &clip, &sel, delta, 0.0);
                    self.set_result(result, std::slice::from_ref(&clip), 0);
                }
                Outcome::None
            }
        }
    }

    pub fn release(&mut self, mods: Mods) -> Outcome {
        let drag = self.drag.take();
        let preview = self.preview.take();
        let preview_ids = self.preview_ids.take();
        let attempt = self.attempt.take();
        self.snap_frame = None;
        match drag {
            Some(Drag::Move { primary, moved, .. }) => {
                if let Some(song) = preview {
                    if let Some(ids) = &preview_ids {
                        self.selection = ids.iter().cloned().collect();
                    }
                    return Outcome::Edit { song, select: preview_ids };
                }
                if let Some(a) = attempt {
                    return Outcome::Refused(a.reason);
                }
                // A plain click on one of several selected clips narrows the selection to it.
                if !moved && !mods.shift && self.selection.len() > 1 {
                    self.select_only(&primary);
                }
                Outcome::None
            }
            Some(Drag::TrimEnd { .. }) | Some(Drag::TrimStart { .. }) | Some(Drag::Keys { .. }) => match (preview, attempt) {
                (Some(song), _) => Outcome::Edit { song, select: None },
                (None, Some(a)) => Outcome::Refused(a.reason),
                _ => Outcome::None,
            },
            _ => Outcome::None,
        }
    }

    /// Where a dragged preset would land.
    pub fn preset_ghost(&mut self, layout: &Layout, doc: &Doc, preset: &Preset, pos: Pos2, mods: Mods) -> Option<Ghost> {
        if !layout.body.contains(pos) || !layout.lane_at(pos.y).is_some_and(|l| l.takes_motion()) {
            self.snap_frame = None;
            return None;
        }
        let mut frame = (layout.frame_at(pos.x).round() as i64).max(0);
        self.snap_frame = None;
        if !mods.shift {
            if let Some(t) = Self::snap(frame, &self.snap_targets(doc, &[]), layout.px_per_frame) {
                frame = t;
                self.snap_frame = Some(t);
            }
        }
        Some(match edit::drop_preview(doc.song, doc.project, preset, frame) {
            Ok(p) => {
                let from = p.from_name.clone().map(|f| format!("{f} to ")).unwrap_or_else(|| "To ".into());
                let mut label = format!("{from}{}, {}", preset.name, secs_text(p.duration_frames, doc.project));
                if p.duration_frames < p.wanted_frames {
                    label.push_str(&format!(" (needs {})", secs_text(p.wanted_frames, doc.project)));
                }
                Ghost { lane: Lane::Motion, start: p.start_frame, duration: p.duration_frames, label, refusal: None }
            }
            Err(r) => Ghost {
                lane: Lane::Motion,
                start: frame,
                duration: doc.project.rate.seconds_to_frames(2.0).round() as i64,
                label: preset.name.clone(),
                refusal: Some(r.to_string()),
            },
        })
    }

    pub fn drop_preset(&mut self, layout: &Layout, doc: &Doc, preset: &Preset, pos: Pos2, mods: Mods) -> Outcome {
        let Some(ghost) = self.preset_ghost(layout, doc, preset, pos, mods) else { return Outcome::None };
        self.snap_frame = None;
        if let Some(reason) = ghost.refusal {
            return Outcome::Refused(reason);
        }
        match edit::insert_preset_move(doc.song, doc.project, preset, ghost.start) {
            Ok((song, id)) => {
                self.select_only(&id);
                self.key_selection.clear();
                Outcome::Edit { song, select: Some(vec![id]) }
            }
            Err(r) => Outcome::Refused(r.to_string()),
        }
    }

    /// Where dragged media would land: on its own track, at the pointer frame.
    pub fn media_ghost(
        &mut self,
        layout: &Layout,
        doc: &Doc,
        name: &str,
        track: Track,
        media_frames: i64,
        pos: Pos2,
        mods: Mods,
    ) -> Option<Ghost> {
        if !layout.body.contains(pos) {
            return None;
        }
        let lane = if track == Track::Video { Lane::Video } else { Lane::Audio };
        let mut frame = (layout.frame_at(pos.x).round() as i64).max(0);
        self.snap_frame = None;
        if !mods.shift {
            if let Some(t) = Self::snap(frame, &self.snap_targets(doc, &[]), layout.px_per_frame) {
                frame = t;
                self.snap_frame = Some(t);
            }
        }
        let refusal = edit::add_media_clip(doc.song, doc.project, "probe", name, track, media_frames, frame).err();
        Some(Ghost {
            lane,
            start: frame,
            duration: media_frames.max(1),
            label: name.to_string(),
            refusal: refusal.map(|r| r.to_string()),
        })
    }

    pub fn drop_media(
        &mut self,
        layout: &Layout,
        doc: &Doc,
        media_id: &str,
        name: &str,
        track: Track,
        media_frames: i64,
        pos: Pos2,
        mods: Mods,
    ) -> Outcome {
        let Some(ghost) = self.media_ghost(layout, doc, name, track, media_frames, pos, mods) else { return Outcome::None };
        self.snap_frame = None;
        match edit::add_media_clip(doc.song, doc.project, media_id, name, track, media_frames, ghost.start) {
            Ok((song, id)) => {
                self.select_only(&id);
                Outcome::Edit { song, select: Some(vec![id]) }
            }
            Err(r) => Outcome::Refused(r.to_string()),
        }
    }
}

/// Ruler tick spacing in frames: (major, minor), adapted to zoom.
pub fn tick_step(px_per_frame: f32, fps: f64) -> (i64, i64) {
    let fps_i = fps.round() as i64;
    let steps: [(i64, i64); 14] = [
        (1, 1),
        (2, 1),
        (5, 1),
        (10, 5),
        (fps_i, (fps_i / 5).max(1)),
        (2 * fps_i, fps_i),
        (5 * fps_i, fps_i),
        (10 * fps_i, 2 * fps_i),
        (15 * fps_i, 5 * fps_i),
        (30 * fps_i, 10 * fps_i),
        (60 * fps_i, 15 * fps_i),
        (120 * fps_i, 30 * fps_i),
        (300 * fps_i, 60 * fps_i),
        (600 * fps_i, 120 * fps_i),
    ];
    steps.into_iter().find(|(major, _)| *major as f32 * px_per_frame >= 96.0).unwrap_or((1800 * fps_i, 600 * fps_i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;
    use ptz_core::pose::Pose;

    fn presets() -> Vec<Preset> {
        let p = |id: &str, name: &str, pan: f64, elevation: f64| Preset {
            id: id.into(),
            name: name.into(),
            pose: Pose { pan, tilt: 0.0, zoom: 0.2, focus: 0.5, elevation },
            camera_number: None,
            has_thumbnail: false,
        };
        vec![
            p("wide", "Stage Wide", 0.0, 120.0),
            p("lectern", "Lectern MCU", -24.0, 310.0),
            p("awards", "Awards Two-Shot", 18.0, 220.0),
        ]
    }

    fn song_with_moves() -> (Song, Project, Vec<String>) {
        let project = Project::default();
        let mut song = Song::new("s", "S", "01:00:00:00", 1000);
        let mut ids = Vec::new();
        for (i, at) in [100, 400, 700].into_iter().enumerate() {
            let (next, id) = edit::insert_preset_move(&song, &project, &presets()[i], at).unwrap();
            song = next;
            ids.push(id);
        }
        song = edit::add_marker(&song, &project, 250, "Doors").unwrap();
        (song, project, ids)
    }

    fn rect() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(HEADER_W + 1000.0, 300.0))
    }

    fn view(ppf: f32) -> View {
        View { px_per_frame: ppf, scroll: 0.0, motion_open: false }
    }

    fn motion_y(l: &Layout) -> f32 {
        l.lane_rect(Lane::Motion).unwrap().center().y
    }

    #[test]
    fn hits_at_edges_bodies_and_gaps_at_several_zooms() {
        let (song, _, ids) = song_with_moves();
        for ppf in [0.5f32, 1.0, 2.0, 3.0] {
            let v = view(ppf);
            let l = layout(&song, &v, rect());
            let y = motion_y(&l);
            let c = song.motion_clip(&ids[0]).unwrap();
            let (x0, x1) = (l.x_of(c.start_frame as f64), l.x_of(c.end_frame() as f64));
            assert_eq!(hit(&l, pos2((x0 + x1) / 2.0, y)), Hit::ClipBody { id: ids[0].clone() }, "body at {ppf}");
            assert_eq!(hit(&l, pos2(x0 + 1.0, y)), Hit::ClipStart { id: ids[0].clone() }, "start at {ppf}");
            assert_eq!(hit(&l, pos2(x1 - 1.0, y)), Hit::ClipEnd { id: ids[0].clone() }, "end at {ppf}");
            // The gap before the first clip.
            let gap = hit(&l, pos2(l.x_of(40.0), y));
            assert_eq!(gap, Hit::Empty { lane: Some(Lane::Motion), frame: 40 }, "gap at {ppf}");
            // Ruler and marker.
            assert_eq!(hit(&l, pos2(l.x_of(60.0), 10.0)), Hit::Ruler { frame: 60 });
            let my = l.lane_rect(Lane::Markers).unwrap().center().y;
            assert!(matches!(hit(&l, pos2(l.x_of(250.0) + 2.0, my)), Hit::Marker { .. }), "marker at {ppf}");
            // Header column and outside.
            assert_eq!(hit(&l, pos2(60.0, y)), Hit::Header { lane: Lane::Motion });
            assert_eq!(hit(&l, pos2(10.0, y)), Hit::Twirl);
            assert_eq!(hit(&l, pos2(-5.0, y)), Hit::Outside);
        }
    }

    #[test]
    fn narrow_clips_keep_a_body_and_scroll_shifts_everything() {
        let (song, _, ids) = song_with_moves();
        let v = View { px_per_frame: 0.1, scroll: 0.0, motion_open: false };
        let l = layout(&song, &v, rect());
        let c = l.clip(&ids[1]).unwrap().clone();
        assert!(c.rect.width() < 21.0);
        assert_eq!(hit(&l, c.rect.center()), Hit::ClipBody { id: ids[1].clone() });

        let v2 = View { px_per_frame: 2.0, scroll: 300.0, motion_open: false };
        let l2 = layout(&song, &v2, rect());
        assert_eq!(l2.x_of(300.0), l2.body.left());
        assert!((l2.frame_at(l2.body.left() + 200.0) - 400.0).abs() < 1e-6);
        let c2 = l2.clip(&ids[1]).unwrap();
        assert!((c2.rect.left() - (HEADER_W + 200.0)).abs() < 1e-3);
    }

    #[test]
    fn open_twirl_adds_axis_lanes_with_keys() {
        let (mut song, project, ids) = song_with_moves();
        song = edit::convert_to_keys(&song, &project, &ids[0]).unwrap();
        let v = View { px_per_frame: 1.0, scroll: 0.0, motion_open: true };
        let l = layout(&song, &v, rect());
        assert_eq!(l.lanes.len(), 9);
        assert_eq!(l.keys.len(), 10, "two keys on each of five axes");
        let k = l.keys.iter().find(|k| k.axis == Axis::Pan && k.index == 0).unwrap();
        assert_eq!(hit(&l, k.pos), Hit::Key { clip: ids[0].clone(), axis: Axis::Pan, index: 0 });
        assert!((Layout::content_height(&v) - (RULER_H + 22.0 + 44.0 + 56.0 + 46.0 + 150.0)).abs() < 1e-3);
    }

    /// The headless driver: the same press, drag and release the window sends.
    struct Driver {
        song: Song,
        project: Project,
        t: Timeline,
        playhead: f64,
        undo: Vec<Song>,
        refusals: Vec<String>,
    }

    impl Driver {
        fn new(song: Song, project: Project) -> Driver {
            let t = Timeline { view: view(1.0), ..Timeline::default() };
            Driver { song, project, t, playhead: 0.0, undo: Vec::new(), refusals: Vec::new() }
        }

        fn layout(&self) -> Layout {
            layout(&self.song, &self.t.view, rect())
        }

        fn take(&mut self, o: Outcome) {
            match o {
                Outcome::Edit { song, .. } => {
                    self.undo.push(std::mem::replace(&mut self.song, song));
                }
                Outcome::Seek(f) => self.playhead = f as f64,
                Outcome::Refused(r) => self.refusals.push(r),
                Outcome::None => {}
            }
        }

        fn drag(&mut self, from: Pos2, to: Pos2, mods: Mods) {
            let l = self.layout();
            let doc = Doc { song: &self.song, project: &self.project, playhead: self.playhead };
            // Shift is pressed after the drag begins, as an operator would: on the press it toggles selection.
            let o = self.t.press(&l, &doc, from, Mods { shift: false, ..mods });
            let mut outs = vec![o];
            for i in 1..=4 {
                let p = from + (to - from) * (i as f32 / 4.0);
                outs.push(self.t.drag_to(&l, &doc, p, mods));
            }
            outs.push(self.t.release(mods));
            for o in outs {
                self.take(o);
            }
        }

        fn click(&mut self, at: Pos2, mods: Mods) {
            let l = self.layout();
            let doc = Doc { song: &self.song, project: &self.project, playhead: self.playhead };
            let o = self.t.press(&l, &doc, at, mods);
            self.take(o);
            let o = self.t.release(mods);
            self.take(o);
        }

        fn drop_preset(&mut self, preset: &Preset, at: Pos2) {
            let l = self.layout();
            let doc = Doc { song: &self.song, project: &self.project, playhead: self.playhead };
            let o = self.t.drop_preset(&l, &doc, preset, at, Mods::default());
            self.take(o);
        }
    }

    #[test]
    fn build_a_song_by_dragging_only() {
        let project = Project::default();
        let mut d = Driver::new(Song::new("s", "S", "01:00:00:00", 1500), project);
        let p = presets();
        let y = motion_y(&d.layout());
        let x = |d: &Driver, f: f64| d.layout().x_of(f);

        // Three drops. The ghost names both poses and the time.
        let l = d.layout();
        let doc = Doc { song: &d.song, project: &d.project, playhead: 0.0 };
        let g = d.t.preset_ghost(&l, &doc, &p[0], pos2(l.x_of(50.0), y), Mods::default()).unwrap();
        assert!(g.refusal.is_none());
        assert!(g.label.starts_with("To Stage Wide, "), "{}", g.label);
        d.drop_preset(&p[0], pos2(x(&d, 50.0), y));
        let l = d.layout();
        let doc = Doc { song: &d.song, project: &d.project, playhead: 0.0 };
        let g = d.t.preset_ghost(&l, &doc, &p[1], pos2(l.x_of(400.0), y), Mods::default()).unwrap();
        assert!(g.label.starts_with("Stage Wide to Lectern MCU, "), "{}", g.label);
        assert_eq!(g.duration, d.project.rate.seconds_to_frames(7.9).round() as i64, "auto time from the elevation travel");
        d.drop_preset(&p[1], pos2(x(&d, 400.0), y));
        d.drop_preset(&p[2], pos2(x(&d, 800.0), y));
        assert_eq!(d.song.motion.len(), 3);
        assert_eq!(d.undo.len(), 3, "each drop is one undo step");
        let ids: Vec<String> = d.song.motion.iter().map(|c| c.id.clone()).collect();
        assert_eq!(d.t.selection.iter().cloned().collect::<Vec<_>>(), vec![ids[2].clone()]);

        // A drop inside an existing move is refused and changes nothing.
        let before = d.song.clone();
        let l = d.layout();
        let doc = Doc { song: &d.song, project: &d.project, playhead: 0.0 };
        let g = d.t.preset_ghost(&l, &doc, &p[2], pos2(l.x_of(450.0), y), Mods { shift: true, ..Mods::default() }).unwrap();
        assert!(g.refusal.as_deref().unwrap().contains("Overlaps"));
        d.drop_preset(&p[2], pos2(x(&d, 450.0), y));
        assert_eq!(d.song, before);
        assert_eq!(d.refusals.len(), 1);

        // Move the second clip later by its body. Linked From keeps the same auto time here.
        let c = d.song.motion_clip(&ids[1]).unwrap().clone();
        let mid = (c.start_frame + c.end_frame()) as f64 / 2.0;
        d.drag(pos2(x(&d, mid), y), pos2(x(&d, mid + 60.0), y), Mods { shift: true, ..Mods::default() });
        assert_eq!(d.song.motion_clip(&ids[1]).unwrap().start_frame, c.start_frame + 60);

        // Dragging it onto the third clip is refused; the song is unchanged.
        let before = d.song.clone();
        let c = d.song.motion_clip(&ids[1]).unwrap().clone();
        let mid = (c.start_frame + c.end_frame()) as f64 / 2.0;
        d.drag(pos2(x(&d, mid), y), pos2(x(&d, mid + 250.0), y), Mods { shift: true, ..Mods::default() });
        assert_eq!(d.song, before);
        assert!(d.refusals.last().unwrap().contains("Overlaps"));

        // Stretch the first clip by its right edge: it becomes custom time.
        let c = d.song.motion_clip(&ids[0]).unwrap().clone();
        d.drag(
            pos2(x(&d, c.end_frame() as f64) - 2.0, y),
            pos2(x(&d, c.end_frame() as f64 + 80.0) - 2.0, y),
            Mods { shift: true, ..Mods::default() },
        );
        let stretched = d.song.motion_clip(&ids[0]).unwrap();
        assert_eq!(stretched.duration_frames, c.duration_frames + 80);
        assert!(matches!(stretched.body, ClipBody::PresetMove { auto_duration: false, .. }));

        // Squeeze it far too short: allowed, and the compiler flags it.
        let c = d.song.motion_clip(&ids[0]).unwrap().clone();
        d.drag(
            pos2(x(&d, c.end_frame() as f64) - 2.0, y),
            pos2(x(&d, c.start_frame as f64 + 20.0) - 2.0, y),
            Mods { shift: true, ..Mods::default() },
        );
        assert_eq!(d.song.motion_clip(&ids[0]).unwrap().duration_frames, 20);
        let issues = ptz_core::compile::compile(&d.song, &d.project).issues;
        assert!(issues.iter().any(|i| i.clip_id() == Some(ids[0].as_str())), "too fast is reported");

        // Trim the start of the third clip; its end stays put.
        let c = d.song.motion_clip(&ids[2]).unwrap().clone();
        d.drag(
            pos2(x(&d, c.start_frame as f64) + 2.0, y),
            pos2(x(&d, c.start_frame as f64 + 30.0) + 2.0, y),
            Mods { shift: true, ..Mods::default() },
        );
        let t = d.song.motion_clip(&ids[2]).unwrap();
        assert_eq!((t.start_frame, t.end_frame()), (c.start_frame + 30, c.end_frame()));

        // Undo walks all the way back to the empty song.
        let steps = d.undo.len();
        assert_eq!(steps, 7);
        assert!(d.undo[0].motion.is_empty());
    }

    #[test]
    fn snapping_to_playhead_markers_and_edges_and_shift_turns_it_off() {
        let (song, project, ids) = song_with_moves();
        let mut d = Driver::new(song, project);
        d.playhead = 300.0;
        let y = motion_y(&d.layout());
        let c = d.song.motion_clip(&ids[0]).unwrap().clone();
        let dur = c.duration_frames;
        let mid = d.layout().x_of((c.start_frame + c.end_frame()) as f64 / 2.0);

        // Marker at 250: dragging the start to within 6 px snaps onto it.
        d.drag(pos2(mid, y), pos2(mid + 146.0, y), Mods::default());
        assert_eq!(d.song.motion_clip(&ids[0]).unwrap().start_frame, 250);

        // The end snaps to the next clip's start (400).
        let c = d.song.motion_clip(&ids[0]).unwrap().clone();
        let mid = d.layout().x_of((c.start_frame + c.end_frame()) as f64 / 2.0);
        let want = 400 - dur - 250;
        d.drag(pos2(mid, y), pos2(mid + want as f32 + 4.0, y), Mods::default());
        assert_eq!(d.song.motion_clip(&ids[0]).unwrap().end_frame(), 400);

        // With Shift the same drag lands exactly where the pointer says.
        let c = d.song.motion_clip(&ids[0]).unwrap().clone();
        let mid = d.layout().x_of((c.start_frame + c.end_frame()) as f64 / 2.0);
        d.drag(pos2(mid, y), pos2(mid - 3.0, y), Mods { shift: true, ..Mods::default() });
        assert_eq!(d.song.motion_clip(&ids[0]).unwrap().start_frame, c.start_frame - 3);

        // The playhead (300) attracts a trimmed end.
        let c = d.song.motion_clip(&ids[0]).unwrap().clone();
        let x_end = d.layout().x_of(c.end_frame() as f64);
        d.drag(pos2(x_end - 2.0, y), pos2(d.layout().x_of(303.0) - 2.0, y), Mods::default());
        assert_eq!(d.song.motion_clip(&ids[0]).unwrap().end_frame(), 300);
    }

    #[test]
    fn selection_marquee_copy_and_scrub() {
        let (song, project, ids) = song_with_moves();
        let mut d = Driver::new(song, project);
        let l = d.layout();
        let y = motion_y(&l);
        let center = |d: &Driver, id: &str| d.layout().clip(id).unwrap().rect.center();

        d.click(center(&d, &ids[0]), Mods::default());
        assert_eq!(d.t.selection.len(), 1);
        d.click(center(&d, &ids[1]), Mods { shift: true, ..Mods::default() });
        assert_eq!(d.t.selection.len(), 2);
        d.click(center(&d, &ids[1]), Mods { shift: true, ..Mods::default() });
        assert_eq!(d.t.selection.iter().cloned().collect::<Vec<_>>(), vec![ids[0].clone()]);

        // Marquee over empty lane space selects what it touches.
        let below = l.lane_rect(Lane::Motion).unwrap();
        d.drag(pos2(l.x_of(380.0), below.top() + 2.0), pos2(l.x_of(900.0), below.bottom() - 2.0), Mods::default());
        assert_eq!(d.t.selection, [ids[1].clone(), ids[2].clone()].into_iter().collect());
        assert!(d.undo.is_empty(), "selecting is not an edit");

        // Dragging one of two selected clips moves both.
        let before: Vec<i64> = ids.iter().map(|id| d.song.motion_clip(id).unwrap().start_frame).collect();
        let c = center(&d, &ids[1]);
        d.drag(c, c + vec2(-40.0, 0.0), Mods { shift: false, ..Mods::default() });
        let after: Vec<i64> = ids.iter().map(|id| d.song.motion_clip(id).unwrap().start_frame).collect();
        assert_eq!(after[0], before[0]);
        assert_eq!(after[1] - before[1], after[2] - before[2]);
        assert!(after[1] < before[1]);

        // Alt-drag copies the clip instead of moving it.
        d.click(pos2(l.x_of(20.0), y), Mods::default());
        assert!(d.t.selection.is_empty());
        let c = center(&d, &ids[0]);
        let count = d.song.motion.len();
        let last_end = d.song.motion.iter().map(|c| c.end_frame()).max().unwrap();
        d.drag(c, c + vec2(last_end as f32 + 20.0 - 100.0, 0.0), Mods { alt: true, shift: true, ..Mods::default() });
        assert_eq!(d.song.motion.len(), count + 1);
        assert_eq!(d.song.motion_clip(&ids[0]).unwrap().start_frame, 100, "the original stays");

        // Dragging in the ruler scrubs.
        d.drag(pos2(l.x_of(10.0), 8.0), pos2(l.x_of(480.0), 8.0), Mods::default());
        assert_eq!(d.playhead, 480.0);

        // Dragging a marker moves it.
        let my = l.lane_rect(Lane::Markers).unwrap().center().y;
        d.drag(pos2(l.x_of(250.0), my), pos2(l.x_of(275.0), my), Mods { shift: true, ..Mods::default() });
        assert_eq!(d.song.markers[0].frame, 275);
    }

    #[test]
    fn zoom_keeps_the_frame_under_the_pointer_and_ticks_adapt() {
        let mut v = View { px_per_frame: 1.0, scroll: 100.0, motion_open: false };
        let before = v.scroll + 300.0 / v.px_per_frame as f64;
        v.zoom_around(2.5, 300.0);
        let after = v.scroll + 300.0 / v.px_per_frame as f64;
        assert!((before - after).abs() < 1e-3);
        assert_eq!(v.px_per_frame, 2.5);

        v.fit(1000, 1040.0);
        assert!((v.px_per_frame - 1032.0 / 1040.0).abs() < 1e-3);
        assert_eq!(v.scroll, 0.0);

        assert_eq!(tick_step(40.0, 25.0), (5, 1));
        assert_eq!(tick_step(4.0, 25.0), (25, 5));
        assert_eq!(tick_step(1.0, 25.0), (125, 25));
        assert_eq!(tick_step(0.05, 25.0), (3000, 750));
    }
}
