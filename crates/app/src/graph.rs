//! Graph editor: value against time for the selected clip, with bezier
//! handles, and a speed view against each axis's limit. It shares the
//! timeline's horizontal zoom, scroll and playhead.

use crate::panels::note;
use crate::session::Session;
use crate::theme::*;
use crate::timeline::{self, KeyRef, Mods, View, HEADER_W};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui};
use ptz_core::compile::{speed_profile, SLACK};
use ptz_core::edit::{self, EasyEase, Refusal};
use ptz_core::eval::Evaluator;
use ptz_core::pose::{Axis, Project};
use ptz_core::song::{Interp, Key, MotionClip, Song};
use std::collections::BTreeSet;

pub const RULER_H: f32 = 20.0;
/// How close the pointer must be to a key or handle to grab it.
const GRAB_PX: f32 = 8.0;
/// How far above or below a curve a click still counts as on it.
const CURVE_PX: f32 = 8.0;
const PAD: f32 = 10.0;
/// Where the max-speed line sits in the speed view, as a fraction of the plot height.
const LIMIT_AT: f32 = 0.66;

#[derive(Clone, PartialEq, Debug)]
enum Drag {
    Keys { clip: String, axis: Axis, grab_frame: f64, grab_value: f64, origin: Pos2 },
    Handle { clip: String, axis: Axis, index: usize, out: bool },
    Marquee { origin: Pos2, current: Pos2, base: BTreeSet<KeyRef> },
    Scrub,
}

pub struct Graph {
    pub visible: [bool; 5],
    /// The axis whose units the value ruler shows.
    pub active: Axis,
    pub speed: bool,
    /// Value at the bottom and top of the plot, per axis: each curve has its own scale.
    pub ranges: [(f64, f64); 5],
    /// The camera whose curves are drawn: the shown clip's, else the selected one.
    pub camera: String,
    /// What the ranges were last fitted to.
    fitted: Option<String>,
    drag: Option<Drag>,
    /// Where the plot was drawn last frame, for the UI tests.
    pub probe: Option<Rect>,
}

impl Default for Graph {
    fn default() -> Self {
        Graph {
            visible: [true; 5],
            active: Axis::Pan,
            speed: false,
            ranges: [(0.0, 1.0); 5],
            camera: ptz_core::camera::first_camera(),
            fitted: None,
            drag: None,
            probe: None,
        }
    }
}

/// The plot's geometry: the same time axis as the timeline, one value scale per axis.
#[derive(Clone, Debug)]
pub struct Plot {
    pub rect: Rect,
    pub ruler: Rect,
    pub body: Rect,
    pub px_per_frame: f32,
    pub scroll: f64,
    pub ranges: [(f64, f64); 5],
}

impl Plot {
    pub fn new(rect: Rect, view: &View, ranges: [(f64, f64); 5]) -> Plot {
        let ruler = Rect::from_min_max(pos2(rect.left() + HEADER_W, rect.top()), pos2(rect.right(), rect.top() + RULER_H));
        let body = Rect::from_min_max(pos2(rect.left() + HEADER_W, ruler.bottom()), rect.right_bottom());
        Plot { rect, ruler, body, px_per_frame: view.px_per_frame, scroll: view.scroll, ranges }
    }

    pub fn x_of(&self, frame: f64) -> f32 {
        self.body.left() + ((frame - self.scroll) * self.px_per_frame as f64) as f32
    }

    pub fn frame_at(&self, x: f32) -> f64 {
        self.scroll + ((x - self.body.left()) / self.px_per_frame) as f64
    }

    fn span(&self) -> f32 {
        (self.body.height() - 2.0 * PAD).max(1.0)
    }

    pub fn y_of(&self, axis: Axis, value: f64) -> f32 {
        let (lo, hi) = self.ranges[axis.index()];
        self.body.bottom() - PAD - ((value - lo) / (hi - lo).max(1e-12)) as f32 * self.span()
    }

    pub fn value_at(&self, axis: Axis, y: f32) -> f64 {
        let (lo, hi) = self.ranges[axis.index()];
        lo + ((self.body.bottom() - PAD - y) / self.span()) as f64 * (hi - lo)
    }

    /// Screen pixels per unit of value.
    fn px_per_value(&self, axis: Axis) -> f64 {
        let (lo, hi) = self.ranges[axis.index()];
        self.span() as f64 / (hi - lo).max(1e-12)
    }

    pub fn key_pos(&self, clip: &MotionClip, axis: Axis, key: &Key) -> Pos2 {
        pos2(self.x_of((clip.start_frame + key.frame) as f64), self.y_of(axis, key.value))
    }

    pub fn handle_pos(&self, clip: &MotionClip, axis: Axis, key: &Key, handle: [f64; 2]) -> Pos2 {
        pos2(self.x_of(clip.start_frame as f64 + key.frame as f64 + handle[0]), self.y_of(axis, key.value + handle[1]))
    }

    /// Height of a speed, as a fraction of the axis's limit, in the speed view.
    pub fn speed_y(&self, fraction: f64) -> f32 {
        (self.body.bottom() - PAD - fraction as f32 * LIMIT_AT * self.span()).max(self.body.top() + 2.0)
    }
}

/// A handle as it is drawn. A handle that was never set sits a third of the
/// way along the straight line to the next key, which is the curve it gives.
/// `None` when there is no bezier segment on that side.
pub fn shown_handle(keys: &[Key], index: usize, out: bool) -> Option<[f64; 2]> {
    let k = keys.get(index)?;
    if out {
        let next = keys.get(index + 1)?;
        if k.interp != Interp::Bezier {
            return None;
        }
        if k.out_handle != [0.0, 0.0] {
            return Some(k.out_handle);
        }
        Some([(next.frame - k.frame) as f64 / 3.0, (next.value - k.value) / 3.0])
    } else {
        let prev = keys.get(index.checked_sub(1)?)?;
        if prev.interp != Interp::Bezier {
            return None;
        }
        if k.in_handle != [0.0, 0.0] {
            return Some(k.in_handle);
        }
        Some([-((k.frame - prev.frame) as f64) / 3.0, -(k.value - prev.value) / 3.0])
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Hit {
    Ruler { frame: i64 },
    Handle { axis: Axis, index: usize, out: bool },
    Key { axis: Axis, index: usize },
    Curve { axis: Axis, frame: f64 },
    Empty,
    Outside,
}

/// Axes in the order they are tested and painted on top: the active one first.
fn axis_order(active: Axis) -> Vec<Axis> {
    let mut out = vec![active];
    out.extend(Axis::ALL.into_iter().filter(|a| *a != active));
    out
}

pub fn hit(plot: &Plot, song: &Song, clip: Option<&MotionClip>, graph: &Graph, selection: &BTreeSet<KeyRef>, pos: Pos2) -> Hit {
    if !plot.rect.contains(pos) || pos.x < plot.body.left() {
        return Hit::Outside;
    }
    if pos.y < plot.ruler.bottom() {
        return Hit::Ruler { frame: plot.frame_at(pos.x).round().clamp(0.0, song.length_frames as f64) as i64 };
    }
    let Some(clip) = clip else { return Hit::Empty };
    if graph.speed {
        return Hit::Empty;
    }
    let near = |p: Pos2| (p - pos).length() <= GRAB_PX;
    let order = axis_order(graph.active);
    let shown = |axis: &&Axis| graph.visible[axis.index()] && clip.has_axis(**axis);
    // Handles of selected keys sit on top of everything.
    for axis in order.iter().filter(shown) {
        let keys = clip.keys(*axis);
        for (index, key) in keys.iter().enumerate() {
            if !selection.contains(&(clip.id.clone(), *axis, index)) {
                continue;
            }
            for out in [true, false] {
                if let Some(h) = shown_handle(keys, index, out) {
                    if near(plot.handle_pos(clip, *axis, key, h)) {
                        return Hit::Handle { axis: *axis, index, out };
                    }
                }
            }
        }
    }
    let mut best: Option<(f32, Axis, usize)> = None;
    for axis in order.iter().filter(shown) {
        for (index, key) in clip.keys(*axis).iter().enumerate() {
            let d = (plot.key_pos(clip, *axis, key) - pos).length();
            if d <= GRAB_PX && best.is_none_or(|(b, _, _)| d < b) {
                best = Some((d, *axis, index));
            }
        }
    }
    if let Some((_, axis, index)) = best {
        return Hit::Key { axis, index };
    }
    let frame = plot.frame_at(pos.x);
    if frame >= clip.start_frame as f64 && frame <= clip.end_frame() as f64 {
        let ev = Evaluator::new(song, &clip.camera);
        for axis in order.iter().filter(shown) {
            // A steep curve crosses several pixels of height in one of width.
            let ys: Vec<f32> = [-3.0f32, 0.0, 3.0]
                .iter()
                .map(|dx| {
                    let f = plot.frame_at(pos.x + dx).clamp(clip.start_frame as f64, clip.end_frame() as f64);
                    plot.y_of(*axis, ev.axis_at(*axis, f))
                })
                .collect();
            let (top, bottom) = ys.iter().fold((f32::MAX, f32::MIN), |(t, b), y| (t.min(*y), b.max(*y)));
            if pos.y >= top - CURVE_PX && pos.y <= bottom + CURVE_PX {
                return Hit::Curve { axis: *axis, frame };
            }
        }
    }
    Hit::Empty
}

/// What a gesture asks the session to do.
#[derive(Clone, PartialEq, Debug)]
pub enum Outcome {
    None,
    Seek(i64),
    /// The song as it will be if the drag ends now.
    Preview(Song),
    /// The drag would be refused here.
    Blocked(String),
}

impl Graph {
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn marquee(&self) -> Option<Rect> {
        match &self.drag {
            Some(Drag::Marquee { origin, current, .. }) => Some(Rect::from_two_pos(*origin, *current)),
            _ => None,
        }
    }

    pub fn cancel(&mut self) {
        self.drag = None;
    }

    pub fn press(
        &mut self,
        plot: &Plot,
        song: &Song,
        clip: Option<&MotionClip>,
        selection: &mut BTreeSet<KeyRef>,
        pos: Pos2,
        mods: Mods,
    ) -> Outcome {
        self.drag = None;
        match hit(plot, song, clip, self, selection, pos) {
            Hit::Ruler { frame } => {
                self.drag = Some(Drag::Scrub);
                Outcome::Seek(frame)
            }
            Hit::Handle { axis, index, out } => {
                let Some(clip) = clip else { return Outcome::None };
                self.active = axis;
                self.drag = Some(Drag::Handle { clip: clip.id.clone(), axis, index, out });
                Outcome::None
            }
            Hit::Key { axis, index } => {
                let Some(clip) = clip else { return Outcome::None };
                let key = (clip.id.clone(), axis, index);
                if mods.shift {
                    if !selection.remove(&key) {
                        selection.insert(key);
                    }
                } else if !selection.contains(&key) {
                    selection.clear();
                    selection.insert(key);
                }
                selection.retain(|k| k.0 == clip.id);
                self.active = axis;
                self.drag = Some(Drag::Keys {
                    clip: clip.id.clone(),
                    axis,
                    grab_frame: plot.frame_at(pos.x),
                    grab_value: plot.value_at(axis, pos.y),
                    origin: pos,
                });
                Outcome::None
            }
            Hit::Curve { axis, .. } => {
                self.active = axis;
                if !mods.shift {
                    selection.clear();
                }
                self.drag = Some(Drag::Marquee { origin: pos, current: pos, base: selection.clone() });
                Outcome::None
            }
            Hit::Empty => {
                if !mods.shift {
                    selection.clear();
                }
                self.drag = Some(Drag::Marquee { origin: pos, current: pos, base: selection.clone() });
                Outcome::None
            }
            Hit::Outside => Outcome::None,
        }
    }

    pub fn drag_to(
        &mut self,
        plot: &Plot,
        song: &Song,
        project: &Project,
        selection: &mut BTreeSet<KeyRef>,
        pos: Pos2,
        mods: Mods,
    ) -> Outcome {
        let Some(drag) = self.drag.clone() else { return Outcome::None };
        let result: Result<Song, Refusal> = match drag {
            Drag::Scrub => {
                return Outcome::Seek(plot.frame_at(pos.x).round().clamp(0.0, song.length_frames as f64) as i64);
            }
            Drag::Marquee { origin, base, .. } => {
                let area = Rect::from_two_pos(origin, pos);
                let mut found = base.clone();
                for clip in &song.motion {
                    for axis in clip.axes.iter().filter(|a| self.visible[a.index()]) {
                        for (index, key) in clip.keys(*axis).iter().enumerate() {
                            if area.contains(plot.key_pos(clip, *axis, key))
                                && base.iter().chain(found.iter()).all(|k| k.0 == clip.id)
                            {
                                found.insert((clip.id.clone(), *axis, index));
                            }
                        }
                    }
                }
                *selection = found;
                self.drag = Some(Drag::Marquee { origin, current: pos, base });
                return Outcome::None;
            }
            Drag::Keys { clip, axis, grab_frame, grab_value, origin } => {
                let mut dframe = (plot.frame_at(pos.x) - grab_frame).round() as i64;
                let mut dvalue = plot.value_at(axis, pos.y) - grab_value;
                if mods.shift {
                    // Shift keeps the drag to the direction it mostly went in.
                    if (pos.x - origin.x).abs() >= (pos.y - origin.y).abs() {
                        dvalue = 0.0;
                    } else {
                        dframe = 0;
                    }
                }
                let Some(c) = song.motion_clip(&clip) else { return Outcome::None };
                let same: Vec<(Axis, usize)> =
                    selection.iter().filter(|k| k.0 == clip && k.1 == axis).map(|k| (k.1, k.2)).collect();
                let others: Vec<(Axis, usize)> =
                    selection.iter().filter(|k| k.0 == clip && k.1 != axis).map(|k| (k.1, k.2)).collect();
                // Values stay inside the axis's range.
                let lim = *project.axes_of(&c.camera).get(axis);
                for (_, index) in &same {
                    if let Some(k) = c.keys(axis).get(*index) {
                        dvalue = dvalue.clamp((lim.min - k.value).min(0.0), (lim.max - k.value).max(0.0));
                    }
                }
                if dframe == 0 && dvalue == 0.0 {
                    return Outcome::Preview(song.clone());
                }
                edit::shift_keys(song, project, &clip, &same, dframe, dvalue).and_then(|moved| {
                    if others.is_empty() || dframe == 0 {
                        Ok(moved)
                    } else {
                        edit::shift_keys(&moved, project, &clip, &others, dframe, 0.0)
                    }
                })
            }
            Drag::Handle { clip, axis, index, out } => {
                let Some(c) = song.motion_clip(&clip) else { return Outcome::None };
                let keys = c.keys(axis);
                let Some(k) = keys.get(index) else { return Outcome::None };
                let at = (c.start_frame + k.frame) as f64;
                let mut h = [plot.frame_at(pos.x) - at, plot.value_at(axis, pos.y) - k.value];
                // A handle never reaches back past its own key or beyond the next one.
                if out {
                    let len = keys.get(index + 1).map(|n| (n.frame - k.frame) as f64).unwrap_or(0.0);
                    h[0] = h[0].clamp(0.0, len);
                } else {
                    let len = index.checked_sub(1).map(|i| (k.frame - keys[i].frame) as f64).unwrap_or(0.0);
                    h[0] = h[0].clamp(-len, 0.0);
                }
                let (mut in_h, mut out_h) = (k.in_handle, k.out_handle);
                if out {
                    out_h = h;
                } else {
                    in_h = h;
                }
                if !mods.alt {
                    // The opposite handle stays in line, at its own length on screen.
                    if let Some(o) = shown_handle(keys, index, !out) {
                        let (sx, sy) = (plot.px_per_frame as f64, plot.px_per_value(axis));
                        let dragged = (h[0] * sx, h[1] * sy);
                        let length = dragged.0.hypot(dragged.1);
                        let other = (o[0] * sx).hypot(o[1] * sy);
                        if length > 1e-6 {
                            let mirrored = [-dragged.0 / length * other / sx, -dragged.1 / length * other / sy];
                            if out {
                                in_h = mirrored;
                            } else {
                                out_h = mirrored;
                            }
                        }
                    }
                }
                edit::set_key_handles(song, project, &clip, axis, index, in_h, out_h)
            }
        };
        match result {
            Ok(next) => Outcome::Preview(next),
            Err(r) => Outcome::Blocked(r.to_string()),
        }
    }

    /// End the gesture. True when it was one that edits the song.
    pub fn release(&mut self) -> bool {
        matches!(self.drag.take(), Some(Drag::Keys { .. } | Drag::Handle { .. }))
    }

    /// Scale each axis to the values it takes over `frames`, with room around them.
    pub fn fit_values(&mut self, song: &Song, project: &Project, frames: (i64, i64), keys: &[(Axis, Key, i64)]) {
        let ev = Evaluator::new(song, &self.camera);
        let limits = project.axes_of(&self.camera);
        let (from, to) = (frames.0 as f64, frames.1.max(frames.0 + 1) as f64);
        for axis in Axis::ALL {
            let (mut lo, mut hi) = (f64::MAX, f64::MIN);
            let mut take = |v: f64| {
                lo = lo.min(v);
                hi = hi.max(v);
            };
            for i in 0..=240 {
                take(ev.axis_at(axis, from + (to - from) * i as f64 / 240.0));
            }
            for (_, k, _) in keys.iter().filter(|k| k.0 == axis) {
                take(k.value);
                take(k.value + k.in_handle[1]);
                take(k.value + k.out_handle[1]);
            }
            let lim = limits.get(axis);
            let least = (lim.max - lim.min) * 0.04;
            if hi - lo < least {
                let mid = (hi + lo) / 2.0;
                lo = mid - least / 2.0;
                hi = mid + least / 2.0;
            }
            let pad = (hi - lo) * 0.12;
            self.ranges[axis.index()] = (lo - pad, hi + pad);
        }
    }
}

/// Stretches of frames where an axis runs faster than its limit: `(first, last)` inclusive.
pub fn over_limit(speeds: &[f64], limit: f64) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = Vec::new();
    for (f, v) in speeds.iter().enumerate() {
        if v.abs() > limit * SLACK {
            match out.last_mut() {
                Some(last) if last.1 + 1 == f as i64 => last.1 = f as i64,
                _ => out.push((f as i64, f as i64)),
            }
        }
    }
    out
}

/// The clip the editor shows: the selected one, else the selected camera's clip under the playhead.
pub fn target_clip<'a>(
    song: &'a Song,
    camera: &str,
    selection: &BTreeSet<String>,
    key_selection: &BTreeSet<KeyRef>,
    playhead: f64,
) -> Option<&'a MotionClip> {
    selection
        .iter()
        .find_map(|id| song.motion_clip(id))
        .or_else(|| key_selection.iter().next().and_then(|k| song.motion_clip(&k.0)))
        .or_else(|| {
            let f = playhead.floor() as i64;
            song.motion
                .iter()
                .filter(|c| c.camera == camera && c.start_frame <= f && f <= c.end_frame())
                .max_by_key(|c| (c.is_keys(), c.start_frame))
        })
}

/// `K`: add a key at the playhead on the selected axis lanes, or on every axis when none is selected.
pub fn add_key_at_playhead(s: &mut Session) {
    let frame = s.playhead().round() as i64;
    let axes: Vec<Axis> = if s.axis_lanes.is_empty() {
        Axis::ALL.to_vec()
    } else {
        Axis::ALL.into_iter().filter(|a| s.axis_lanes.contains(a)).collect()
    };
    let camera = s.cam_id().to_string();
    s.edit(|song, project| edit::add_key(song, project, &camera, &axes, frame));
}

fn selected_in(s: &Session, clip: &str) -> Vec<(Axis, usize)> {
    s.timeline.key_selection.iter().filter(|k| k.0 == clip).map(|k| (k.1, k.2)).collect()
}

/// Run one per-key edit over every selected key as a single undo step.
fn edit_selected(s: &mut Session, clip: &str, op: impl Fn(&Song, &Project, Axis, usize) -> Result<Song, Refusal>) {
    let keys = selected_in(s, clip);
    s.edit(|song, project| {
        let mut next = song.clone();
        for (axis, index) in &keys {
            next = op(&next, project, *axis, *index)?;
        }
        Ok(next)
    });
}

/// Fit the view to the selected keys, or to the whole clip when none is selected.
pub fn frame_selection(s: &mut Session, song: &Song, clip: Option<&MotionClip>, body_width: f32) {
    let (frames, keys) = match clip {
        Some(c) => {
            let all: Vec<(Axis, Key, i64)> =
                c.axes.iter().flat_map(|a| c.keys(*a).iter().map(|k| (*a, *k, c.start_frame + k.frame))).collect();
            let chosen: Vec<(Axis, Key, i64)> = selected_in(s, &c.id)
                .into_iter()
                .filter_map(|(a, i)| c.keys(a).get(i).map(|k| (a, *k, c.start_frame + k.frame)))
                .collect();
            if chosen.len() >= 2 {
                let lo = chosen.iter().map(|k| k.2).min().unwrap_or(c.start_frame);
                let hi = chosen.iter().map(|k| k.2).max().unwrap_or(c.end_frame());
                ((lo, hi), chosen)
            } else {
                ((c.start_frame, c.end_frame()), all)
            }
        }
        None => ((0, song.length_frames), Vec::new()),
    };
    let span = (frames.1 - frames.0).max(5) as f32;
    let view = &mut s.timeline.view;
    view.px_per_frame = ((body_width - 16.0).max(50.0) / (span * 1.12)).clamp(View::MIN_PPF, View::MAX_PPF);
    view.scroll = frames.0 as f64 - (span * 0.06) as f64;
    s.graph.fit_values(song, &s.project, frames, &keys);
}

fn toolbar(ui: &mut Ui, s: &mut Session, clip: Option<&MotionClip>) -> bool {
    let mut frame_it = false;
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        for axis in Axis::ALL {
            let on = s.graph.visible[axis.index()];
            let active = s.graph.active == axis;
            let color = axis_color(axis);
            let text = RichText::new(axis.label()).small().color(if on { color } else { MUTED });
            let button = egui::Button::new(text).small().stroke(Stroke::new(1.0, if active && on { color } else { LINE }));
            let r = ui.add(button).on_hover_text(format!(
                "Show or hide the {} curve. Click a curve to make it the one the value ruler measures",
                axis.label()
            ));
            if r.clicked() {
                s.graph.visible[axis.index()] = !on;
                if !on {
                    s.graph.active = axis;
                }
            }
        }
        ui.separator();
        if ui
            .add(egui::Button::selectable(s.graph.speed, RichText::new("Speed").small()))
            .on_hover_text("Show how fast each axis moves, against the fastest it can go")
            .clicked()
        {
            s.graph.speed = !s.graph.speed;
            s.graph.cancel();
        }
        if ui
            .add(egui::Button::new(RichText::new("Frame").small()).small())
            .on_hover_text("Fit the view to the selected keys, or the whole clip  ( F )")
            .clicked()
        {
            frame_it = true;
        }
        let Some(clip) = clip else { return };
        ui.separator();
        if clip.is_preset_move() {
            if ui
                .add(egui::Button::new(RichText::new("Convert to keys").small()).small())
                .on_hover_text("Turn this preset move into keys you can edit here")
                .clicked()
            {
                let id = clip.id.clone();
                s.edit(|song, project| edit::convert_to_keys(song, project, &id));
            }
            ui.label(RichText::new("A preset move's curve is set by its ease.").small().color(MUTED));
            return;
        }
        let chosen = selected_in(s, &clip.id);
        let any = !chosen.is_empty() && !s.graph.speed;
        let id = clip.id.clone();
        let current: Vec<Interp> = chosen.iter().filter_map(|(a, i)| clip.keys(*a).get(*i).map(|k| k.interp)).collect();
        for interp in Interp::ALL {
            let on = any && current.iter().all(|i| *i == interp);
            let tip = match interp {
                Interp::Hold => "Hold the value until the next key",
                Interp::Linear => "Straight line to the next key",
                Interp::Bezier => "Curve to the next key, shaped by the handles",
            };
            if ui
                .add_enabled(any, egui::Button::selectable(on, RichText::new(interp.label()).small()))
                .on_hover_text(tip)
                .clicked()
            {
                edit_selected(s, &id, |song, project, axis, index| edit::set_key_interp(song, project, &id, axis, index, interp));
            }
        }
        ui.separator();
        for (label, mode, tip) in [
            ("Easy Ease", EasyEase::Both, "Ease into and out of the selected keys"),
            ("Ease In", EasyEase::In, "Ease into the selected keys"),
            ("Ease Out", EasyEase::Out, "Ease out of the selected keys"),
        ] {
            if ui.add_enabled(any, egui::Button::new(RichText::new(label).small()).small()).on_hover_text(tip).clicked() {
                edit_selected(s, &id, |song, project, axis, index| edit::easy_ease(song, project, &id, axis, index, mode));
            }
        }
        if !any && !s.graph.speed {
            ui.label(RichText::new("Click a key to shape it. Double-click a curve to add one.").small().color(MUTED));
        }
    });
    ui.add_space(2.0);
    frame_it
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    let Some(song) = s.song.clone() else {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| note(ui, "No song is open."));
        return;
    };
    let playhead = s.playhead();
    let clip_id =
        target_clip(&song, s.cam_id(), &s.timeline.selection, &s.timeline.key_selection, playhead).map(|c| c.id.clone());
    let clip = clip_id.as_deref().and_then(|id| song.motion_clip(id)).cloned();
    // The curves drawn are one camera's: the shown clip's, else the selected camera's.
    s.graph.camera = clip.as_ref().map(|c| c.camera.clone()).unwrap_or_else(|| s.cam_id().to_string());
    let mut frame_it = toolbar(ui, s, clip.as_ref());

    let (resp, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let rect = resp.rect;
    s.graph.probe = Some(rect);
    let body_w = rect.width() - HEADER_W;

    // Scroll and zoom move the timeline's view: the two panels share it.
    if resp.contains_pointer() {
        let (scroll, zoom, pointer) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        if zoom != 1.0 {
            let x = pointer.map(|p| p.x - rect.left() - HEADER_W).unwrap_or(body_w / 2.0);
            s.timeline.view.zoom_around(zoom, x.max(0.0));
        }
        let travel = if scroll.x.abs() > scroll.y.abs() { scroll.x } else { scroll.y };
        if travel != 0.0 {
            s.timeline.view.scroll -= (travel / s.timeline.view.px_per_frame) as f64;
        }
        if ui.input(|i| i.key_pressed(egui::Key::F)) && !ui.ctx().text_edit_focused() {
            frame_it = true;
        }
    }
    s.timeline.view.clamp(song.length_frames, body_w);

    // Each axis is scaled to the clip being shown; a different clip is fitted afresh.
    let fit_key = clip_id.clone().unwrap_or_default();
    if s.graph.fitted.as_deref() != Some(fit_key.as_str()) {
        let (frames, keys) = match &clip {
            Some(c) => (
                (c.start_frame, c.end_frame()),
                c.axes.iter().flat_map(|a| c.keys(*a).iter().map(|k| (*a, *k, c.start_frame + k.frame))).collect::<Vec<_>>(),
            ),
            None => ((0, song.length_frames), Vec::new()),
        };
        s.graph.fit_values(&song, &s.project, frames, &keys);
        s.graph.fitted = Some(fit_key);
        if let Some(c) = &clip {
            if !s.graph.visible[s.graph.active.index()] || !c.has_axis(s.graph.active) {
                if let Some(a) = c.axes.iter().find(|a| s.graph.visible[a.index()]) {
                    s.graph.active = *a;
                }
            }
        }
    }
    if frame_it {
        frame_selection(s, &song, clip.as_ref(), body_w);
        s.timeline.view.clamp(song.length_frames, body_w);
    }

    let plot = Plot::new(rect, &s.timeline.view, s.graph.ranges);
    let m = crate::timeline_ui::mods(ui);
    let editable = s.can_edit();
    let pointer = ui.input(|i| i.pointer.interact_pos());

    let mut outcomes = Vec::new();
    let mut finished = false;
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(p) = ui.input(|i| i.pointer.press_origin()).or(pointer) {
            let target = hit(&plot, &song, clip.as_ref(), &s.graph, &s.timeline.key_selection, p);
            let edits = matches!(target, Hit::Key { .. } | Hit::Handle { .. });
            if edits && editable.is_err() {
                outcomes.push(Outcome::Blocked(editable.clone().unwrap_err()));
            } else {
                outcomes.push(s.graph.press(&plot, &song, clip.as_ref(), &mut s.timeline.key_selection, p, m));
            }
        }
    }
    if resp.dragged_by(egui::PointerButton::Primary) && s.graph.is_dragging() {
        if let Some(p) = pointer {
            outcomes.push(s.graph.drag_to(&plot, &song, &s.project, &mut s.timeline.key_selection, p, m));
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) && s.graph.is_dragging() {
        finished = s.graph.release();
    }
    if resp.clicked() {
        if let Some(p) = pointer {
            outcomes.push(s.graph.press(&plot, &song, clip.as_ref(), &mut s.timeline.key_selection, p, m));
            s.graph.release();
        }
    }
    if resp.double_clicked() {
        if let (Some(p), Some(c)) = (pointer, &clip) {
            if let Hit::Curve { axis, frame } = hit(&plot, &song, clip.as_ref(), &s.graph, &s.timeline.key_selection, p) {
                let (id, local) = (c.id.clone(), frame.round() as i64 - c.start_frame);
                if c.is_keys() {
                    if s.edit(|song, project| edit::add_key_at(song, project, &id, axis, local, None)) {
                        let index = s
                            .song
                            .as_ref()
                            .and_then(|x| x.motion_clip(&id))
                            .and_then(|c| c.keys(axis).iter().position(|k| k.frame == local));
                        s.timeline.key_selection.clear();
                        if let Some(index) = index {
                            s.timeline.key_selection.insert((id, axis, index));
                        }
                    }
                } else {
                    s.toast("Convert this preset move to keys to add keys to it", true);
                }
            }
        }
    }
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    let mut blocked: Option<String> = None;
    for o in outcomes {
        match o {
            Outcome::None => {}
            Outcome::Seek(frame) => s.seek(frame),
            Outcome::Preview(next) => s.timeline.preview = (next != song).then_some(next),
            Outcome::Blocked(why) => {
                s.timeline.preview = None;
                blocked = Some(why);
            }
        }
    }
    if finished {
        match (s.timeline.preview.take(), &blocked) {
            (Some(next), _) => s.commit_edit(next),
            (None, Some(why)) => s.toast(why.clone(), true),
            _ => {}
        }
    } else if let (Some(why), false) = (&blocked, s.graph.is_dragging()) {
        s.toast(why.clone(), true);
    }

    if let Some(p) = resp.hover_pos() {
        let over = hit(&plot, &song, clip.as_ref(), &s.graph, &s.timeline.key_selection, p);
        if matches!(over, Hit::Key { .. } | Hit::Handle { .. }) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    // While a key is dragged the curves follow the pointer.
    let shown: &Song = s.timeline.preview.as_ref().unwrap_or(&song);
    let shown_clip = clip_id.as_deref().and_then(|id| shown.motion_clip(id));
    paint(&painter, &plot, shown, shown_clip, s, playhead);
    if let (Some(why), Some(p)) = (&blocked, pointer) {
        painter.text(p + vec2(14.0, 18.0), Align2::LEFT_TOP, why, FontId::proportional(12.0), AMBER);
    }
    if let Some(area) = s.graph.marquee() {
        painter.rect(area, CornerRadius::ZERO, with_alpha(BLUE, 40), Stroke::new(1.0, BLUE), StrokeKind::Inside);
    }
    if s.is_playing() || s.graph.is_dragging() {
        ui.ctx().request_repaint();
    }
}

/// A round step that gives about `lines` grid lines across `span`.
fn nice_step(span: f64, lines: f64) -> f64 {
    let raw = (span / lines).max(1e-9);
    let mag = 10f64.powf(raw.log10().floor());
    let n = raw / mag;
    mag * if n < 1.5 {
        1.0
    } else if n < 3.5 {
        2.0
    } else if n < 7.5 {
        5.0
    } else {
        10.0
    }
}

fn diamond(painter: &Painter, c: Pos2, half: f32, fill: Color32, stroke: Stroke) {
    let pts = vec![pos2(c.x, c.y - half), pos2(c.x + half, c.y), pos2(c.x, c.y + half), pos2(c.x - half, c.y)];
    painter.add(egui::Shape::convex_polygon(pts, fill, stroke));
}

fn paint(painter: &Painter, plot: &Plot, song: &Song, clip: Option<&MotionClip>, s: &Session, playhead: f64) {
    let g = &s.graph;
    let fps = s.project.rate.fps();
    painter.rect_filled(plot.rect, 0.0, LANE);
    let pb = painter.with_clip_rect(plot.body);

    // Outside the clip being edited is dimmed.
    if let Some(c) = clip {
        let (x0, x1) = (plot.x_of(c.start_frame as f64), plot.x_of(c.end_frame() as f64));
        for (a, b) in [(plot.body.left(), x0), (x1, plot.body.right())] {
            if b > a {
                pb.rect_filled(
                    Rect::from_min_max(pos2(a, plot.body.top()), pos2(b, plot.body.bottom())),
                    0.0,
                    with_alpha(Color32::BLACK, 60),
                );
            }
        }
        pb.vline(x0, plot.body.y_range(), Stroke::new(1.0, LINE));
        pb.vline(x1, plot.body.y_range(), Stroke::new(1.0, LINE));
    }

    // Time grid and ruler, on the timeline's ticks.
    painter.rect_filled(Rect::from_min_max(plot.rect.left_top(), pos2(plot.rect.right(), plot.ruler.bottom())), 0.0, PANEL_DARK);
    let (major, _) = timeline::tick_step(plot.px_per_frame, fps);
    let pr = painter.with_clip_rect(plot.ruler);
    let mut f = ((plot.scroll.max(0.0) as i64) / major) * major;
    let last = plot.frame_at(plot.body.right()).ceil() as i64;
    while f <= last {
        let x = plot.x_of(f as f64);
        pb.vline(x, plot.body.y_range(), Stroke::new(1.0, with_alpha(LINE, 70)));
        pr.text(pos2(x + 4.0, plot.ruler.top() + 4.0), Align2::LEFT_TOP, s.timecode_at(f), FontId::monospace(10.0), MUTED);
        f += major;
    }
    painter.hline(plot.rect.x_range(), plot.ruler.bottom(), Stroke::new(1.0, LINE));

    // Value gutter.
    let gutter = Rect::from_min_max(pos2(plot.rect.left(), plot.ruler.bottom()), pos2(plot.body.left(), plot.rect.bottom()));
    painter.rect_filled(gutter, 0.0, CHARCOAL);
    painter.vline(plot.body.left(), plot.rect.y_range(), Stroke::new(1.0, LINE));
    let pg = painter.with_clip_rect(gutter);
    let active = g.active;
    let ink = axis_color(active);

    if g.speed {
        paint_speed(painter, plot, s, &pg);
    } else {
        let (lo, hi) = plot.ranges[active.index()];
        let step = nice_step(hi - lo, (plot.body.height() / 42.0).max(2.0) as f64);
        let mut v = (lo / step).ceil() * step;
        while v <= hi {
            let y = plot.y_of(active, v);
            pb.hline(plot.body.x_range(), y, Stroke::new(1.0, with_alpha(LINE, 70)));
            pg.text(pos2(gutter.right() - 8.0, y), Align2::RIGHT_CENTER, active.format(v), FontId::monospace(10.5), ink);
            v += step;
        }
        painter.text(
            pos2(plot.rect.left() + 10.0, plot.ruler.center().y),
            Align2::LEFT_CENTER,
            format!("{} ({})", active.label(), active.unit()),
            FontId::proportional(11.5),
            ink,
        );

        let ev = Evaluator::new(song, &g.camera);
        let order = axis_order(active);
        for axis in order.iter().rev().filter(|a| g.visible[a.index()]) {
            let color = axis_color(*axis);
            let in_clip = clip.is_some_and(|c| c.has_axis(*axis));
            let mut inside: Vec<Pos2> = Vec::new();
            let mut outside: Vec<Vec<Pos2>> = vec![Vec::new()];
            let mut x = plot.body.left();
            while x <= plot.body.right() {
                let frame = plot.frame_at(x);
                if frame >= 0.0 && frame <= song.length_frames as f64 {
                    let p = pos2(x, plot.y_of(*axis, ev.axis_at(*axis, frame)));
                    let within = clip.is_some_and(|c| in_clip && frame >= c.start_frame as f64 && frame <= c.end_frame() as f64);
                    if within {
                        inside.push(p);
                        if outside.last().is_some_and(|l| !l.is_empty()) {
                            outside.push(Vec::new());
                        }
                    } else if let Some(l) = outside.last_mut() {
                        l.push(p);
                    }
                }
                x += 2.0;
            }
            for run in outside.into_iter().filter(|r| r.len() > 1) {
                pb.line(run, Stroke::new(1.0, with_alpha(color, if clip.is_some() { 70 } else { 200 })));
            }
            if inside.len() > 1 {
                pb.line(inside, Stroke::new(if *axis == active { 2.0 } else { 1.5 }, color));
            }
        }

        // Keys and handles of the clip being edited.
        if let Some(c) = clip.filter(|c| c.is_keys()) {
            for axis in order.iter().rev().filter(|a| g.visible[a.index()] && c.has_axis(**a)) {
                let color = axis_color(*axis);
                let keys = c.keys(*axis);
                for (index, key) in keys.iter().enumerate() {
                    let at = plot.key_pos(c, *axis, key);
                    if at.x < plot.body.left() - 60.0 || at.x > plot.body.right() + 60.0 {
                        continue;
                    }
                    let selected = s.timeline.key_selection.contains(&(c.id.clone(), *axis, index));
                    if selected {
                        for out in [false, true] {
                            if let Some(h) = shown_handle(keys, index, out) {
                                let end = plot.handle_pos(c, *axis, key, h);
                                pb.line_segment([at, end], Stroke::new(1.0, OFF_WHITE));
                                pb.circle(end, 3.5, PANEL_DARK, Stroke::new(1.5, OFF_WHITE));
                            }
                        }
                    }
                    let fill = if selected { OFF_WHITE } else { color };
                    diamond(&pb, at, 5.0, fill, Stroke::new(1.0, if selected { color } else { PANEL_DARK }));
                }
            }
        } else if clip.is_none() {
            pb.text(
                plot.body.center_top() + vec2(0.0, 10.0),
                Align2::CENTER_TOP,
                "Select a clip on the timeline to edit its keys here.",
                FontId::proportional(12.0),
                MUTED,
            );
        }
    }

    let x = plot.x_of(playhead);
    if x >= plot.body.left() - 1.0 && x <= plot.rect.right() + 1.0 {
        let color = if s.status.hardware_live { GREEN } else { mix(GREEN, OFF_WHITE, 0.15) };
        painter.vline(x, plot.ruler.top() + 6.0..=plot.rect.bottom(), Stroke::new(1.5, color));
        let head =
            vec![pos2(x - 5.0, plot.ruler.top() + 2.0), pos2(x + 5.0, plot.ruler.top() + 2.0), pos2(x, plot.ruler.top() + 11.0)];
        painter.add(egui::Shape::convex_polygon(head, color, Stroke::NONE));
    }
}

/// Each axis's speed as a fraction of the fastest it can go. One line marks
/// the limit for all of them; anything above it is filled in the warning colour.
fn paint_speed(painter: &Painter, plot: &Plot, s: &Session, gutter: &Painter) {
    let g = &s.graph;
    let pb = painter.with_clip_rect(plot.body);
    let limit_y = plot.speed_y(1.0);
    let base_y = plot.speed_y(0.0);
    for (fraction, label) in [(0.0, "still"), (0.5, "half speed")] {
        let y = plot.speed_y(fraction);
        pb.hline(plot.body.x_range(), y, Stroke::new(1.0, with_alpha(LINE, 90)));
        gutter.text(pos2(plot.body.left() - 8.0, y), Align2::RIGHT_CENTER, label, FontId::proportional(10.5), MUTED);
    }
    let active = g.active;
    let limits = s.project.axes_of(&g.camera);
    let lim = limits.get(active);
    gutter.text(
        pos2(plot.body.left() - 8.0, limit_y),
        Align2::RIGHT_CENTER,
        format!("max {:.0} {}/s", lim.max_speed, active.unit()),
        FontId::proportional(10.5),
        AMBER,
    );
    painter.text(
        pos2(plot.rect.left() + 10.0, plot.ruler.center().y),
        Align2::LEFT_CENTER,
        format!("{} speed", active.label()),
        FontId::proportional(11.5),
        axis_color(active),
    );

    let poses = s.compiled.poses(&g.camera);
    let mut worst: Vec<(Axis, f64, i64)> = Vec::new();
    for axis in axis_order(active).iter().rev().filter(|a| g.visible[a.index()]) {
        let top = limits.get(*axis).max_speed;
        let speeds = speed_profile(poses, *axis, s.project.rate);
        if speeds.is_empty() {
            continue;
        }
        let color = axis_color(*axis);
        let mut line: Vec<Pos2> = Vec::new();
        let mut x = plot.body.left();
        while x <= plot.body.right() {
            // The fastest frame under this pixel, so a short spike is never zoomed away.
            let f0 = plot.frame_at(x).floor().max(0.0) as usize;
            let f1 = (plot.frame_at(x + 1.0).ceil().max(0.0) as usize).max(f0 + 1).min(speeds.len());
            if f0 < f1 {
                let fastest = speeds[f0..f1].iter().fold(0.0f64, |m, v| m.max(v.abs()));
                let y = plot.speed_y(fastest / top);
                if fastest > top * SLACK {
                    pb.vline(x + 0.5, y..=limit_y, Stroke::new(1.0, with_alpha(AMBER, 150)));
                }
                line.push(pos2(x, y));
            }
            x += 1.0;
        }
        if line.len() > 1 {
            pb.line(line, Stroke::new(if *axis == active { 2.0 } else { 1.25 }, color));
        }
        let peak = speeds.iter().enumerate().fold((0usize, 0.0f64), |m, (f, v)| if v.abs() > m.1 { (f, v.abs()) } else { m });
        if peak.1 > top * SLACK {
            worst.push((*axis, peak.1 / top, peak.0 as i64));
        }
    }
    pb.hline(plot.body.x_range(), limit_y, Stroke::new(1.5, AMBER));
    pb.hline(plot.body.x_range(), base_y, Stroke::new(1.0, LINE));

    worst.reverse();
    let mut y = plot.body.top() + 8.0;
    if worst.is_empty() {
        pb.text(
            pos2(plot.body.right() - 10.0, y),
            Align2::RIGHT_TOP,
            "Every axis shown stays within its speed limit.",
            FontId::proportional(11.5),
            MUTED,
        );
    }
    for (axis, times, frame) in worst {
        pb.text(
            pos2(plot.body.right() - 10.0, y),
            Align2::RIGHT_TOP,
            format!("{} reaches {:.0}% of its limit at {}", axis.label(), times * 100.0, s.timecode_at(frame)),
            FontId::proportional(11.5),
            AMBER,
        );
        y += 16.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ptz_core::eval::{keys_value, pose_at};
    use ptz_core::song::ClipBody;
    use ptz_core::FIRST_CAMERA as CAM;
    use std::collections::BTreeMap;

    fn song_with_keys() -> (Song, Project, String) {
        let project = Project::default();
        let song = Song::new("s", "S", "01:00:00:00", 600);
        let mut curves = BTreeMap::new();
        curves.insert(
            Axis::Pan,
            vec![Key::new(0, 0.0, Interp::Bezier), Key::new(100, 30.0, Interp::Bezier), Key::new(200, 10.0, Interp::Bezier)],
        );
        curves.insert(Axis::Zoom, vec![Key::new(0, 0.2, Interp::Linear), Key::new(200, 0.6, Interp::Linear)]);
        let (song, id) = edit::punch_keys(&song, &project, CAM, 100, 200, curves, None).unwrap();
        (song, project, id)
    }

    fn plot_for(song: &Song, project: &Project, clip: &str) -> (Graph, Plot) {
        let mut g = Graph::default();
        let c = song.motion_clip(clip).unwrap();
        g.fit_values(song, project, (c.start_frame, c.end_frame()), &[]);
        let view = View { px_per_frame: 2.0, scroll: 50.0, motion_open: false };
        let plot = Plot::new(Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 320.0)), &view, g.ranges);
        (g, plot)
    }

    const NO_MODS: Mods = Mods { shift: false, alt: false, command: false };

    #[test]
    fn plot_maps_frames_and_values_both_ways_on_each_axis_scale() {
        let (song, project, id) = song_with_keys();
        let (g, plot) = plot_for(&song, &project, &id);
        assert_eq!(plot.body.left(), HEADER_W, "the time axis lines up with the timeline's lanes");
        assert!((plot.frame_at(plot.x_of(137.0)) - 137.0).abs() < 1e-3);
        for axis in [Axis::Pan, Axis::Zoom] {
            let (lo, hi) = g.ranges[axis.index()];
            let mid = (lo + hi) / 2.0;
            assert!((plot.value_at(axis, plot.y_of(axis, mid)) - mid).abs() < (hi - lo) * 1e-4);
            assert!((plot.y_of(axis, mid) - plot.body.center().y).abs() < 1.0, "each axis fills the plot on its own scale");
        }
        // Pan spans 30 degrees and zoom 0.4, yet both curves use most of the height.
        let c = song.motion_clip(&id).unwrap();
        let top = |axis: Axis| c.keys(axis).iter().map(|k| plot.key_pos(c, axis, k).y).fold(f32::MAX, f32::min);
        assert!(top(Axis::Pan) < plot.body.top() + 60.0 && top(Axis::Zoom) < plot.body.top() + 60.0);
    }

    #[test]
    fn hit_finds_keys_then_curves_and_handles_only_on_selected_keys() {
        let (song, project, id) = song_with_keys();
        let (g, plot) = plot_for(&song, &project, &id);
        let c = song.motion_clip(&id).unwrap();
        let mut sel = BTreeSet::new();
        let key1 = plot.key_pos(c, Axis::Pan, &c.keys(Axis::Pan)[1]);
        assert_eq!(hit(&plot, &song, Some(c), &g, &sel, key1 + vec2(3.0, -2.0)), Hit::Key { axis: Axis::Pan, index: 1 });

        let ev = Evaluator::new(&song, CAM);
        let on_curve = pos2(plot.x_of(150.0), plot.y_of(Axis::Pan, ev.axis_at(Axis::Pan, 150.0)));
        assert!(matches!(hit(&plot, &song, Some(c), &g, &sel, on_curve), Hit::Curve { axis: Axis::Pan, .. }));
        assert_eq!(hit(&plot, &song, Some(c), &g, &sel, pos2(plot.x_of(150.0), plot.body.bottom() - 2.0)), Hit::Empty);
        assert_eq!(hit(&plot, &song, Some(c), &g, &sel, pos2(plot.x_of(150.0), 5.0)), Hit::Ruler { frame: 150 });
        assert_eq!(hit(&plot, &song, Some(c), &g, &sel, pos2(20.0, 100.0)), Hit::Outside);

        let handle = plot.handle_pos(c, Axis::Pan, &c.keys(Axis::Pan)[1], shown_handle(c.keys(Axis::Pan), 1, true).unwrap());
        assert!(
            !matches!(hit(&plot, &song, Some(c), &g, &sel, handle), Hit::Handle { .. }),
            "handles show on selected keys only"
        );
        sel.insert((id.clone(), Axis::Pan, 1));
        assert_eq!(hit(&plot, &song, Some(c), &g, &sel, handle), Hit::Handle { axis: Axis::Pan, index: 1, out: true });

        // A hidden axis cannot be grabbed.
        let mut hidden = Graph { visible: [false, true, true, true, true], ..Graph::default() };
        hidden.ranges = g.ranges;
        assert_eq!(hit(&plot, &song, Some(c), &hidden, &sel, key1), Hit::Empty);
    }

    #[test]
    fn dragging_a_key_changes_time_and_value_and_shift_keeps_one_direction() {
        let (song, project, id) = song_with_keys();
        let (mut g, plot) = plot_for(&song, &project, &id);
        let c = song.motion_clip(&id).unwrap().clone();
        let mut sel = BTreeSet::new();
        let from = plot.key_pos(&c, Axis::Pan, &c.keys(Axis::Pan)[1]);
        g.press(&plot, &song, Some(&c), &mut sel, from, NO_MODS);
        assert_eq!(sel.iter().cloned().collect::<Vec<_>>(), vec![(id.clone(), Axis::Pan, 1)]);

        // 40 px right is 20 frames at 2 px per frame; 30 px up is a known value step.
        let to = from + vec2(40.0, -30.0);
        let Outcome::Preview(next) = g.drag_to(&plot, &song, &project, &mut sel, to, NO_MODS) else { panic!("no preview") };
        let moved = next.motion_clip(&id).unwrap().keys(Axis::Pan)[1];
        assert_eq!(moved.frame, 120);
        let want = 30.0 + 30.0 / plot.px_per_value(Axis::Pan);
        assert!((moved.value - want).abs() < 1e-3, "{} vs {want}", moved.value);
        assert_eq!(next.motion_clip(&id).unwrap().keys(Axis::Zoom), c.keys(Axis::Zoom), "other axes are untouched");

        let Outcome::Preview(flat) = g.drag_to(&plot, &song, &project, &mut sel, to, Mods { shift: true, ..NO_MODS }) else {
            panic!("no preview")
        };
        let k = flat.motion_clip(&id).unwrap().keys(Axis::Pan)[1];
        assert_eq!((k.frame, k.value), (120, 30.0), "Shift keeps the larger direction only");

        // A key cannot pass its neighbour or leave the axis's range.
        let far = from + vec2(900.0, -5000.0);
        let Outcome::Preview(clamped) = g.drag_to(&plot, &song, &project, &mut sel, far, NO_MODS) else { panic!("no preview") };
        let k = clamped.motion_clip(&id).unwrap().keys(Axis::Pan)[1];
        assert_eq!(k.frame, 199);
        assert_eq!(k.value, project.first().axes.get(Axis::Pan).max);
        assert!(g.release());
    }

    #[test]
    fn dragging_a_handle_keeps_the_other_in_line_unless_alt_is_held() {
        let (song, project, id) = song_with_keys();
        let song = edit::easy_ease(&song, &project, &id, Axis::Pan, 1, EasyEase::Both).unwrap();
        let (mut g, plot) = plot_for(&song, &project, &id);
        let c = song.motion_clip(&id).unwrap().clone();
        let mut sel = BTreeSet::from([(id.clone(), Axis::Pan, 1)]);
        let key = c.keys(Axis::Pan)[1];
        assert_eq!((key.in_handle, key.out_handle), ([-42.0, 0.0], [42.0, 0.0]));
        let grab = plot.handle_pos(&c, Axis::Pan, &key, key.out_handle);
        g.press(&plot, &song, Some(&c), &mut sel, grab, NO_MODS);
        let to = grab + vec2(-20.0, -40.0);

        let Outcome::Preview(next) = g.drag_to(&plot, &song, &project, &mut sel, to, NO_MODS) else { panic!("no preview") };
        let k = next.motion_clip(&id).unwrap().keys(Axis::Pan)[1];
        assert!((k.out_handle[0] - 32.0).abs() < 1e-6 && k.out_handle[1] > 0.0, "{:?}", k.out_handle);
        // In line: the two handles point exactly opposite ways on screen, and the in handle kept its length.
        let screen = |h: [f64; 2]| (h[0] * plot.px_per_frame as f64, h[1] * plot.px_per_value(Axis::Pan));
        let (o, i) = (screen(k.out_handle), screen(k.in_handle));
        assert!((o.0 * i.1 - o.1 * i.0).abs() < 1e-6 && o.0 * i.0 + o.1 * i.1 < 0.0, "out {o:?} in {i:?}");
        assert!((i.0.hypot(i.1) - 84.0).abs() < 1e-6);

        let Outcome::Preview(broken) = g.drag_to(&plot, &song, &project, &mut sel, to, Mods { alt: true, ..NO_MODS }) else {
            panic!("no preview")
        };
        let k = broken.motion_clip(&id).unwrap().keys(Axis::Pan)[1];
        assert_eq!(k.in_handle, [-42.0, 0.0], "Alt breaks the handles: the other one stays put");
        assert!(k.out_handle[1] > 0.0);
    }

    #[test]
    fn a_marquee_selects_the_keys_inside_it() {
        let (song, project, id) = song_with_keys();
        let (mut g, plot) = plot_for(&song, &project, &id);
        let c = song.motion_clip(&id).unwrap().clone();
        let mut sel = BTreeSet::new();
        let start = pos2(plot.x_of(190.0), plot.body.top() + 2.0);
        assert_eq!(hit(&plot, &song, Some(&c), &g, &sel, start), Hit::Empty);
        g.press(&plot, &song, Some(&c), &mut sel, start, NO_MODS);
        g.drag_to(&plot, &song, &project, &mut sel, pos2(plot.x_of(310.0), plot.body.bottom() - 2.0), NO_MODS);
        assert_eq!(
            sel.iter().cloned().collect::<Vec<_>>(),
            vec![(id.clone(), Axis::Pan, 1), (id.clone(), Axis::Pan, 2), (id.clone(), Axis::Zoom, 1)]
        );
        assert!(g.marquee().is_some());
        assert!(!g.release(), "a marquee is not an edit");
    }

    #[test]
    fn an_unset_handle_is_drawn_along_the_straight_line_it_gives() {
        let keys = vec![Key::new(0, 0.0, Interp::Bezier), Key::new(90, 30.0, Interp::Linear), Key::new(120, 0.0, Interp::Bezier)];
        assert_eq!(shown_handle(&keys, 0, true), Some([30.0, 10.0]));
        assert_eq!(shown_handle(&keys, 1, false), Some([-30.0, -10.0]));
        assert_eq!(shown_handle(&keys, 1, true), None, "a linear segment has no handle");
        assert_eq!(shown_handle(&keys, 0, false), None);
        assert_eq!(shown_handle(&keys, 2, true), None);
        // Setting that handle to where it is drawn leaves the curve where it was.
        let mut set = keys.clone();
        set[0].out_handle = [30.0, 10.0];
        set[1].in_handle = [-30.0, -10.0];
        for f in 0..=90 {
            assert!((keys_value(&keys, f as f64).unwrap() - keys_value(&set, f as f64).unwrap()).abs() < 1e-9);
        }
    }

    #[test]
    fn speed_view_marks_exactly_the_stretch_the_feasibility_check_flags() {
        let project = Project::default();
        let top = project.first().axes.get(Axis::Pan).max_speed;
        let fps = project.rate.fps();
        // 100 degrees in one second is over a 60 degree per second limit; the slow return is not.
        let song = Song::new("s", "S", "01:00:00:00", 400);
        let mut curves = BTreeMap::new();
        curves.insert(
            Axis::Pan,
            vec![Key::new(0, 0.0, Interp::Linear), Key::new(25, 100.0, Interp::Linear), Key::new(225, 0.0, Interp::Linear)],
        );
        let (song, id) = edit::punch_keys(&song, &project, CAM, 50, 225, curves, None).unwrap();
        let compiled = ptz_core::compile::compile(&song, &project);
        let speeds = speed_profile(compiled.poses(CAM), Axis::Pan, project.rate);
        assert_eq!(over_limit(&speeds, top), vec![(50, 74)]);
        assert!((speeds[60] - 100.0).abs() < 1e-6 && (speeds[100] + 100.0 / 8.0).abs() < 1e-6);
        let flagged: Vec<_> = compiled
            .issues
            .iter()
            .filter_map(|i| match i {
                ptz_core::compile::Issue::TooFast { clip, axis: Axis::Pan, frame, speed, .. } => {
                    Some((clip.clone(), *frame, *speed))
                }
                _ => None,
            })
            .collect();
        assert_eq!(flagged.len(), 1, "{:?}", compiled.issues);
        assert_eq!(flagged[0].0, id);
        assert!((50..=74).contains(&flagged[0].1) && (flagged[0].2 - 100.0 * fps / 25.0).abs() < 1e-6);
        // A move inside the limit marks nothing.
        assert!(over_limit(
            &speed_profile(compiled.poses(CAM), Axis::Tilt, project.rate),
            project.first().axes.get(Axis::Tilt).max_speed
        )
        .is_empty());

        // The limit line is where a speed equal to the limit is drawn, and faster is above it.
        let (_, plot) = plot_for(&song, &project, &id);
        assert!(plot.speed_y(100.0 / top) < plot.speed_y(1.0) && plot.speed_y(1.0) < plot.speed_y(0.5));
        assert_eq!(plot.speed_y(0.0), plot.body.bottom() - PAD);
    }

    #[test]
    fn the_editor_shows_the_selected_clip_else_the_one_under_the_playhead() {
        let (song, project, id) = song_with_keys();
        let preset = crate::ui_tests::presets().remove(1);
        let (song, other) = edit::insert_preset_move(&song, &project, &preset, 400).unwrap();
        let none: BTreeSet<String> = BTreeSet::new();
        let no_keys: BTreeSet<KeyRef> = BTreeSet::new();
        assert_eq!(target_clip(&song, CAM, &none, &no_keys, 20.0).map(|c| c.id.clone()), None);
        assert_eq!(target_clip(&song, CAM, &none, &no_keys, 150.0).map(|c| c.id.clone()), Some(id.clone()));
        assert_eq!(target_clip(&song, CAM, &BTreeSet::from([other.clone()]), &no_keys, 150.0).map(|c| c.id.clone()), Some(other));
        let keyed = BTreeSet::from([(id.clone(), Axis::Pan, 0)]);
        assert_eq!(target_clip(&song, CAM, &none, &keyed, 500.0).map(|c| c.id.clone()), Some(id.clone()));
        assert!(matches!(song.motion_clip(&id).unwrap().body, ClipBody::Keys { .. }));
        assert_eq!(pose_at(&song, CAM, 200).pan, 30.0);
    }
}
