//! The timeline as an egui widget: one painted surface, no per-clip widgets.

use crate::session::Session;
use crate::theme::*;
use crate::timeline::{self, ClipBox, ClipKind, Doc, Ghost, Hit, Lane, Layout, Mods, Outcome, Stamp, HEADER_W};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
use ptz_core::api::{MediaInfo, PeakLevel};
use ptz_core::pose::{Axis, Preset};
use ptz_core::song::{ClipBody, MediaClip, Song, Track};

/// Drag payloads from the browser tabs.
#[derive(Clone)]
pub struct PresetDrag(pub Preset);
#[derive(Clone)]
pub struct MediaDrag(pub MediaInfo);

pub fn mods(ui: &Ui) -> Mods {
    ui.input(|i| Mods { shift: i.modifiers.shift, alt: i.modifiers.alt, command: i.modifiers.command })
}

fn media_track(info: &MediaInfo) -> Track {
    match info.kind {
        ptz_core::api::MediaKind::Video => Track::Video,
        ptz_core::api::MediaKind::Audio => Track::Audio,
    }
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    let Some(song) = s.song.clone() else {
        empty(ui, |ui| note(ui, "No song is open. Use New or Load in the header."));
        return;
    };

    toolbar(ui, s, &song);

    let avail = ui.available_size();
    let height = avail.y.max(Layout::content_height(&song, &s.project, &s.timeline.view));
    let (resp, painter) = ui.allocate_painter(vec2(avail.x, height), Sense::click_and_drag());
    let rect = resp.rect;
    s.probe_timeline = Some(rect);
    let body_w = rect.width() - HEADER_W;
    if s.fit_requested && body_w > 50.0 {
        s.timeline.view.fit(song.length_frames, body_w);
        s.fit_requested = false;
    }

    // Scroll and zoom.
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
    }
    s.timeline.view.clamp(song.length_frames, body_w);

    request_peaks(s, &song);

    let playhead = s.playhead();
    let base = timeline::layout(&song, &s.project, &s.timeline.view, rect);
    request_filmstrip(s, ui.ctx(), &song, &base);
    let m = mods(ui);
    let editable = s.can_edit();
    let pointer = ui.input(|i| i.pointer.interact_pos());

    // Pointer gestures. The gesture code sees the song as it was when the drag began.
    let mut outcomes = Vec::new();
    {
        let doc = Doc { song: &song, project: &s.project, playhead };
        if resp.drag_started_by(egui::PointerButton::Primary) {
            let origin = ui.input(|i| i.pointer.press_origin()).or(pointer);
            if let Some(p) = origin {
                let target = timeline::hit(&base, p);
                let edits =
                    !matches!(target, Hit::Ruler { .. } | Hit::Empty { .. } | Hit::Twirl(_) | Hit::Header { .. } | Hit::Outside);
                if edits && editable.is_err() {
                    outcomes.push(Outcome::Refused(editable.clone().unwrap_err()));
                } else {
                    outcomes.push(s.timeline.press(&base, &doc, p, m));
                }
            }
        }
        if resp.dragged_by(egui::PointerButton::Primary) && s.timeline.is_dragging() {
            if let Some(p) = pointer {
                outcomes.push(s.timeline.drag_to(&base, &doc, p, m));
            }
        }
        if resp.drag_stopped_by(egui::PointerButton::Primary) && s.timeline.is_dragging() {
            outcomes.push(s.timeline.release(m));
        }
        if resp.clicked() {
            if let Some(p) = pointer {
                outcomes.push(s.timeline.press(&base, &doc, p, m));
                outcomes.push(s.timeline.release(m));
            }
        }
    }
    if resp.clicked() || resp.drag_started() {
        resp.request_focus();
    }
    // Touching a camera's lane, or one of its moves, selects that camera.
    if resp.clicked() || resp.drag_started_by(egui::PointerButton::Primary) {
        let origin = ui.input(|i| i.pointer.press_origin()).or(pointer);
        let camera = origin.and_then(|p| match timeline::hit(&base, p) {
            Hit::Header { lane } | Hit::Empty { lane: Some(lane), .. } => {
                lane.camera().and_then(|i| s.project.cameras.get(i)).map(|c| c.id.clone())
            }
            Hit::ClipBody { id } | Hit::ClipStart { id } | Hit::ClipEnd { id } | Hit::Key { clip: id, .. } => {
                song.motion_clip(&id).map(|c| c.camera.clone())
            }
            _ => None,
        });
        if let Some(camera) = camera {
            let before = s.cam_id().to_string();
            s.select_camera(&camera);
            if s.cam_id() != before {
                s.axis_lanes.clear();
            }
        }
    }
    // Axis lane headers: the dot record-enables the axis, the name selects the lane for K.
    if resp.clicked() {
        if let Some(p) = pointer {
            if let Hit::Header { lane: Lane::Axis(camera, axis) } = timeline::hit(&base, p) {
                if record_dot(&base, camera, axis).is_some_and(|c| (c - p).length() <= 11.0) {
                    s.toggle_record_axis(axis);
                } else if !s.axis_lanes.remove(&axis) {
                    s.axis_lanes.insert(axis);
                }
            }
        }
    }

    // Drops from the Presets and Media tabs.
    let mut ghost: Option<Ghost> = None;
    if let (Some(payload), Some(p)) = (resp.dnd_hover_payload::<PresetDrag>(), ui.ctx().pointer_latest_pos()) {
        let doc = Doc { song: &song, project: &s.project, playhead };
        ghost = s.timeline.preset_ghost(&base, &doc, &payload.0, p, m);
    }
    if let (Some(payload), Some(p)) = (resp.dnd_hover_payload::<MediaDrag>(), ui.ctx().pointer_latest_pos()) {
        let doc = Doc { song: &song, project: &s.project, playhead };
        let info = &payload.0;
        let stamp = Stamp::of(info, &doc, m);
        ghost = s.timeline.media_ghost(&base, &doc, &info.name, media_track(info), info.duration_frames, stamp, p, m);
    }
    // Taking a payload of the wrong type would throw it away, so ask for the type first.
    let holds = |ui: &Ui, preset: bool| {
        if preset {
            egui::DragAndDrop::has_payload_of_type::<PresetDrag>(ui.ctx())
        } else {
            egui::DragAndDrop::has_payload_of_type::<MediaDrag>(ui.ctx())
        }
    };
    if let Some(payload) = holds(ui, true).then(|| resp.dnd_release_payload::<PresetDrag>()).flatten() {
        if let Some(p) = ui.ctx().pointer_latest_pos() {
            let doc = Doc { song: &song, project: &s.project, playhead };
            outcomes.push(match &editable {
                Ok(()) => s.timeline.drop_preset(&base, &doc, &payload.0, p, m),
                Err(why) => Outcome::Refused(why.clone()),
            });
        }
    }
    if let Some(payload) = holds(ui, false).then(|| resp.dnd_release_payload::<MediaDrag>()).flatten() {
        if let Some(p) = ui.ctx().pointer_latest_pos() {
            let doc = Doc { song: &song, project: &s.project, playhead };
            let info = &payload.0;
            let stamp = Stamp::of(info, &doc, m);
            let track = media_track(info);
            let outcome = match &editable {
                Ok(()) => s.timeline.drop_media(&base, &doc, &info.id, &info.name, track, info.duration_frames, stamp, p, m),
                Err(why) => Outcome::Refused(why.clone()),
            };
            if let Outcome::StampOutside { at_frame } = outcome {
                s.stamped_drop = Some(crate::session::StampedDrop { media: info.clone(), at_frame });
            } else {
                outcomes.push(outcome);
            }
        }
    }

    for o in outcomes {
        match o {
            Outcome::None => {}
            Outcome::Seek(frame) => s.seek(frame),
            Outcome::Edit { song, .. } => s.commit_edit(song),
            Outcome::Refused(why) => s.toast(why, true),
            Outcome::StampOutside { .. } => {}
        }
    }

    // Double-click a marker to rename it.
    if resp.double_clicked() {
        if let Some(p) = pointer {
            if let Hit::Marker { id } = timeline::hit(&base, p) {
                let name = song.markers.iter().find(|mk| mk.id == id).map(|mk| mk.name.clone()).unwrap_or_default();
                ui.ctx().data_mut(|d| d.insert_temp(crate::app::marker_rename_id(), (id, name)));
            }
        }
    }

    // Cursor and hover text.
    let hover = resp.hover_pos().map(|p| (p, timeline::hit(&base, p)));
    if let Some((_, h)) = &hover {
        let icon = match h {
            Hit::ClipStart { .. } | Hit::ClipEnd { .. } => Some(egui::CursorIcon::ResizeHorizontal),
            Hit::ClipBody { .. } | Hit::Marker { .. } | Hit::Key { .. } => Some(egui::CursorIcon::Grab),
            Hit::Twirl(_) => Some(egui::CursorIcon::PointingHand),
            _ => None,
        };
        if let Some(icon) = icon {
            ui.ctx().set_cursor_icon(icon);
        }
    }

    context_menu(&resp, s, &song, &base);

    // Paint the preview while a drag is in progress, else the song itself.
    let shown: &Song = s.timeline.preview.as_ref().unwrap_or(&song);
    let l = if s.timeline.preview.is_some() { timeline::layout(shown, &s.project, &s.timeline.view, rect) } else { base.clone() };
    paint(&painter, &l, shown, s, playhead);

    // Refused drag: where it would have gone, in the warning colour.
    if let Some(attempt) = s.timeline.attempt.clone() {
        for id in &attempt.ids {
            if let Some(c) = base.clip(id) {
                let r = c.rect.translate(vec2(attempt.delta as f32 * base.px_per_frame, 0.0));
                painter.with_clip_rect(base.body).rect(
                    r,
                    CornerRadius::same(RADIUS),
                    with_alpha(AMBER, 40),
                    Stroke::new(RULE_STRONG, AMBER),
                    StrokeKind::Inside,
                );
            }
        }
        if let Some(p) = pointer {
            hint_box(&painter, p + vec2(14.0, 18.0), &attempt.reason, AMBER, rect);
        }
    }

    if let Some(g) = &ghost {
        paint_ghost(&painter, &l, g, pointer, rect);
    }
    if let Some(f) = s.timeline.snap_frame {
        let x = l.x_of(f as f64);
        if x >= l.body.left() {
            painter.vline(x, l.ruler.top()..=l.rect.bottom(), Stroke::new(1.0, with_alpha(OFF_WHITE, 170)));
        }
    }
    if let Some(mr) = s.timeline.marquee() {
        painter.rect(mr, CornerRadius::ZERO, with_alpha(ACCENT, 40), Stroke::new(RULE, ACCENT), StrokeKind::Inside);
    }

    // Issue text under the pointer.
    if let (Some((p, Hit::ClipBody { id } | Hit::ClipStart { id } | Hit::ClipEnd { id })), false) =
        (&hover, s.timeline.is_dragging())
    {
        let issues = s.issues_for(id);
        if !issues.is_empty() {
            let text = issues.iter().map(|i| i.message_in(s.project.units)).collect::<Vec<_>>().join("\n");
            hint_box(&painter, *p + vec2(14.0, 18.0), &text, AMBER, rect);
        }
    }

    if s.is_playing() || s.timeline.is_dragging() {
        ui.ctx().request_repaint();
    }
}

fn toolbar(ui: &mut Ui, s: &mut Session, song: &Song) {
    crate::theme::toolbar(ui, |ui| {
        let fps = s.project.rate.fps();
        if ui.button("Fit").on_hover_text("Fit the song to the window  ( \\ )").clicked() {
            s.fit_requested = true;
        }
        if ui.button("-").on_hover_text("Zoom out").clicked() {
            s.timeline.view.zoom_around(1.0 / 1.5, 300.0);
        }
        if ui.button("+").on_hover_text("Zoom in").clicked() {
            s.timeline.view.zoom_around(1.5, 300.0);
        }
        ui.separator();
        if ui.button("Add marker").on_hover_text("Add a marker at the playhead  ( M )").clicked() {
            add_marker(s);
        }
        let n = s.timeline.selection.len();
        if ui.add_enabled(n > 0, egui::Button::new("Delete")).on_hover_text("Delete the selection  ( Delete )").clicked() {
            delete_selection(s);
        }
        if ui.add_enabled(n > 0, egui::Button::new("Duplicate")).on_hover_text("Copy after the selection  ( Cmd-D )").clicked() {
            duplicate_selection(s);
        }
        ui.separator();
        ui.label(
            egui::RichText::new(format!(
                "{} moves   {:.1} s   {} fps",
                song.motion.len(),
                song.length_frames as f64 / fps,
                s.project.rate.label()
            ))
            .color(MUTED)
            .small(),
        );
        if song.motion.is_empty() && song.media.is_empty() {
            // The next thing to do with an empty song, so it is said in the accent.
            ui.label(egui::RichText::new("Drag a preset from the Presets tab onto the timeline to start.").color(ACCENT).small());
        }
    });
}

/// Centre of the record-enable dot on one of a camera's axis lanes.
pub fn record_dot(l: &Layout, camera: usize, axis: Axis) -> Option<Pos2> {
    l.lane_rect(Lane::Axis(camera, axis)).map(|r| pos2(l.body.left() - 14.0, r.center().y))
}

pub fn add_marker(s: &mut Session) {
    let frame = s.playhead().round() as i64;
    let n = s.song.as_ref().map(|x| x.markers.len() + 1).unwrap_or(1);
    s.edit(|song, project| ptz_core::edit::add_marker(song, project, frame, &format!("Marker {n}")));
}

pub fn delete_selection(s: &mut Session) {
    if !s.timeline.key_selection.is_empty() {
        let keys: Vec<_> = s.timeline.key_selection.iter().cloned().collect();
        let clips: std::collections::BTreeSet<String> = keys.iter().map(|k| k.0.clone()).collect();
        let done = s.edit(|song, project| {
            let mut next = song.clone();
            for clip in &clips {
                let sel: Vec<(Axis, usize)> = keys.iter().filter(|k| &k.0 == clip).map(|k| (k.1, k.2)).collect();
                next = ptz_core::edit::delete_keys(&next, project, clip, &sel)?;
            }
            Ok(next)
        });
        if done {
            s.timeline.key_selection.clear();
        }
        return;
    }
    let ids = s.selected_ids();
    if ids.is_empty() {
        return;
    }
    if s.edit(|song, project| ptz_core::edit::delete(song, project, &ids)) {
        s.timeline.selection.clear();
    }
}

pub fn duplicate_selection(s: &mut Session) {
    let ids = s.selected_ids();
    let mut new_ids = Vec::new();
    let ok = s.edit(|song, project| {
        let (next, created) = ptz_core::edit::duplicate(song, project, &ids)?;
        new_ids = created;
        Ok(next)
    });
    if ok {
        s.timeline.selection = new_ids.into_iter().collect();
    }
}

fn context_menu(resp: &egui::Response, s: &mut Session, song: &Song, layout: &Layout) {
    if resp.secondary_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Hit::ClipBody { id } | Hit::ClipStart { id } | Hit::ClipEnd { id } | Hit::Marker { id } =
                timeline::hit(layout, p)
            {
                if !s.timeline.selection.contains(&id) {
                    s.timeline.selection.clear();
                    s.timeline.selection.insert(id);
                }
            }
        }
    }
    resp.context_menu(|ui| {
        let ids = s.selected_ids();
        let one_move = (ids.len() == 1).then(|| song.motion_clip(&ids[0])).flatten().cloned();
        if ids.is_empty() {
            if ui.button("Add marker at playhead").clicked() {
                add_marker(s);
                ui.close();
            }
            return;
        }
        if let Some(clip) = &one_move {
            if let ClipBody::PresetMove { auto_duration, .. } = &clip.body {
                if ui.add_enabled(!*auto_duration, egui::Button::new("Auto time")).clicked() {
                    let id = clip.id.clone();
                    s.edit(|song, project| ptz_core::edit::set_auto_duration(song, project, &id));
                    ui.close();
                }
                if ui.button("Convert to keys").clicked() {
                    let id = clip.id.clone();
                    s.edit(|song, project| ptz_core::edit::convert_to_keys(song, project, &id));
                    ui.close();
                }
                ui.separator();
            }
        }
        if ui.button("Duplicate").clicked() {
            duplicate_selection(s);
            ui.close();
        }
        if ui.button("Delete").clicked() {
            delete_selection(s);
            ui.close();
        }
    });
}

fn hint_box(painter: &Painter, at: Pos2, text: &str, color: Color32, bounds: Rect) {
    let galley = painter.layout(text.to_string(), small(), OFF_WHITE, 340.0);
    let size = galley.size() + vec2(2.0 * GAP, 2.0 * U);
    let mut min = at;
    min.x = min.x.min(bounds.right() - size.x - 4.0).max(bounds.left() + 4.0);
    min.y = min.y.min(bounds.bottom() - size.y - 4.0).max(bounds.top() + 4.0);
    let r = Rect::from_min_size(min, size);
    painter.rect(r, CornerRadius::same(RADIUS), PANEL_DARK, Stroke::new(RULE, color), StrokeKind::Inside);
    painter.galley(r.min + vec2(GAP, U), galley, OFF_WHITE);
}

fn paint_ghost(painter: &Painter, l: &Layout, g: &Ghost, pointer: Option<Pos2>, bounds: Rect) {
    let Some(lane) = l.lane_rect(g.lane) else { return };
    let color = if g.refusal.is_some() { AMBER } else { ACCENT };
    let r = Rect::from_min_max(
        pos2(l.x_of(g.start as f64), lane.top() + 3.0),
        pos2(l.x_of((g.start + g.duration) as f64), lane.bottom() - 3.0),
    );
    let p = painter.with_clip_rect(l.body);
    p.rect(r, CornerRadius::same(RADIUS), with_alpha(color, 70), Stroke::new(RULE_STRONG, color), StrokeKind::Inside);
    let text = match &g.refusal {
        Some(why) => format!("{}\n{why}", g.label),
        None => g.label.clone(),
    };
    let at = pointer.unwrap_or(r.left_bottom()) + vec2(14.0, 18.0);
    hint_box(painter, at, &text, color, bounds);
}

fn hatch(painter: &Painter, r: Rect, color: Color32) {
    let p = painter.with_clip_rect(r.intersect(painter.clip_rect()));
    let step = 8.0;
    let mut x = r.left() - r.height();
    while x < r.right() {
        p.line_segment([pos2(x, r.bottom()), pos2(x + r.height(), r.top())], Stroke::new(1.5, color));
        x += step;
    }
}

fn diamond(painter: &Painter, c: Pos2, half: f32, fill: Color32, stroke: Stroke) {
    let pts = vec![pos2(c.x, c.y - half), pos2(c.x + half, c.y), pos2(c.x, c.y + half), pos2(c.x - half, c.y)];
    painter.add(egui::Shape::convex_polygon(pts, fill, stroke));
}

fn paint_ruler(painter: &Painter, l: &Layout, s: &Session) {
    let fps = s.project.rate.fps();
    painter.rect_filled(Rect::from_min_max(l.rect.left_top(), pos2(l.rect.right(), l.ruler.bottom())), 0.0, PANEL_DARK);
    let (major, minor) = timeline::tick_step(l.px_per_frame, fps);
    let p = painter.with_clip_rect(l.ruler);
    let first = ((l.scroll.max(0.0) as i64) / minor) * minor;
    let last = l.frame_at(l.ruler.right()).ceil() as i64;
    let mut f = first;
    while f <= last {
        let x = l.x_of(f as f64);
        if f % major == 0 {
            p.vline(x, l.ruler.bottom() - 2.0 * U..=l.ruler.bottom(), Stroke::new(RULE, MUTED));
            p.text(pos2(x + U, l.ruler.top() + U), Align2::LEFT_TOP, s.timecode_at(f), mono(MICRO), MUTED);
        } else {
            p.vline(x, l.ruler.bottom() - U..=l.ruler.bottom(), Stroke::new(RULE, CONTROL_LINE));
        }
        f += minor;
    }
    painter.hline(l.rect.left()..=l.rect.right(), l.ruler.bottom(), Stroke::new(RULE, LINE));
    painter.text(
        pos2(l.rect.left() + GAP, l.ruler.center().y),
        Align2::LEFT_CENTER,
        s.timecode_at(s.playhead().floor() as i64),
        mono(SMALL),
        clock_ink(&s.status.transport, s.status.hardware_live),
    );
}

/// The colour that says what a clip is: on its leading edge, and behind it as a tint.
fn clip_ink(kind: ClipKind) -> Color32 {
    match kind {
        ClipKind::Move | ClipKind::AxisPart => MOVE_CLIP,
        ClipKind::Keys => KEYS_CLIP,
        ClipKind::Audio => AUDIO_CLIP,
        ClipKind::Video => VIDEO_CLIP,
    }
}

/// A clip is a dark plate with a little of its kind's colour in it, and more of it when selected.
/// The full colours are left to the states that are drawn over it.
fn clip_fill(kind: ClipKind, selected: bool) -> Color32 {
    mix(PANEL, clip_ink(kind), if selected { 0.42 } else { 0.24 })
}

/// The width of the strip down a clip's leading edge.
const CLIP_EDGE: f32 = 3.0;

/// The waveform level that suits the zoom: the coarsest with at least one bucket per pixel.
pub fn peak_level_for(sample_rate: u32, fps: f64, px_per_frame: f32) -> u32 {
    let samples_per_pixel = sample_rate as f64 / fps / px_per_frame.max(1e-6) as f64;
    let mut level = 256u32;
    while (level as f64) * 2.0 <= samples_per_pixel && level < (1 << 24) {
        level *= 2;
    }
    level
}

/// Ask the server for the waveform of every media clip on the timeline, at the level the zoom needs.
fn request_peaks(s: &mut Session, song: &Song) {
    let fps = s.project.rate.fps();
    for clip in &song.media {
        let Some(info) = s.media.iter().find(|m| m.id == clip.media_id) else { continue };
        let want = peak_level_for(info.sample_rate, fps, s.timeline.view.px_per_frame);
        if s.peaks_requested.get(&clip.media_id) != Some(&want) {
            s.peaks_requested.insert(clip.media_id.clone(), want);
            s.link.send(crate::link::Cmd::FetchPeaks { media: clip.media_id.clone(), spp: want });
        }
    }
}

/// Height of the waveform under a video clip's pictures.
const VIDEO_WAVE_H: f32 = 14.0;

/// The part of a video clip that shows pictures: everything above its waveform.
pub fn filmstrip_rect(clip: Rect) -> Rect {
    Rect::from_min_max(clip.min + vec2(0.0, 1.0), pos2(clip.right(), clip.bottom() - VIDEO_WAVE_H - 1.0))
}

/// One picture of a filmstrip: where it is drawn, and which frame of the file it shows.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Tile {
    pub rect: Rect,
    pub frame: i64,
}

/// The pictures across the visible part of a video clip. Tiles are laid out from the
/// file's first frame, so trimming or moving the clip never changes which pictures are
/// needed, and frames are rounded to a power of two so zooming reuses most of them.
pub fn filmstrip_tiles(strip: Rect, visible: Rect, clip: &MediaClip, px_per_frame: f32) -> Vec<Tile> {
    let tile_w = (strip.height() * 16.0 / 9.0).max(8.0);
    if strip.height() < 8.0 || px_per_frame <= 0.0 || visible.width() <= 0.0 {
        return Vec::new();
    }
    let per_tile = (tile_w / px_per_frame) as f64;
    let step = (2f64.powf(per_tile.max(1.0).log2().floor()) as i64).max(1);
    let origin = strip.left() - clip.in_frame as f32 * px_per_frame;
    let from = visible.left().max(strip.left());
    let to = visible.right().min(strip.right());
    let first = ((from - origin) / tile_w).floor().max(0.0) as i64;
    let mut out = Vec::new();
    let mut i = first;
    loop {
        let x = origin + i as f32 * tile_w;
        if x >= to || out.len() >= 200 {
            break;
        }
        let frame = ((i as f64 * per_tile) as i64 / step * step).clamp(0, (clip.media_frames - 1).max(0));
        out.push(Tile { rect: Rect::from_min_size(pos2(x, strip.top()), vec2(tile_w, strip.height())), frame });
        i += 1;
    }
    out
}

/// Ask for the filmstrip pictures of every video clip in view, and turn the ones that have arrived into textures.
fn request_filmstrip(s: &mut Session, ctx: &egui::Context, song: &Song, l: &Layout) {
    for c in l.clips.iter().filter(|c| c.kind == ClipKind::Video) {
        let Some(clip) = song.media_clip(&c.id) else { continue };
        if !s.media.iter().any(|m| m.id == clip.media_id && m.proxy_frames > 0) {
            continue;
        }
        let visible = c.rect.intersect(l.body);
        for tile in filmstrip_tiles(filmstrip_rect(c.rect), visible, clip, l.px_per_frame) {
            let key = crate::frames::key(&clip.media_id, tile.frame, true);
            s.frames.want(&s.link, &key);
            s.frames.texture(ctx, &key);
        }
    }
}

/// Which media second each pixel column of a clip shows, and the waveform there.
/// `in_s..out_s` is the stretch of the file the clip plays.
pub fn waveform_columns(level: &PeakLevel, r: Rect, visible: Rect, in_s: f64, out_s: f64) -> Vec<(f32, f32, f32)> {
    let mut out = Vec::new();
    if level.pairs.is_empty() || r.width() < 1.0 || visible.width() <= 0.0 {
        return out;
    }
    let bucket = level.bucket_seconds();
    let per_px = (out_s - in_s) / r.width() as f64;
    let mut x = visible.left().floor();
    while x < visible.right() {
        let t0 = in_s + (x - r.left()) as f64 * per_px;
        let t1 = t0 + per_px;
        let i0 = (t0 / bucket).floor().max(0.0) as usize;
        let i1 = ((t1 / bucket).ceil() as usize).max(i0 + 1).min(level.pairs.len());
        if i0 < i1 {
            let (lo, hi) = level.pairs[i0..i1].iter().fold((0i8, 0i8), |(lo, hi), p| (lo.min(p.0), hi.max(p.1)));
            out.push((x, lo as f32 / 127.0, hi as f32 / 127.0));
        }
        x += 1.0;
    }
    out
}

fn paint_waveform(painter: &Painter, r: Rect, level: &PeakLevel, in_s: f64, out_s: f64, color: Color32) {
    let visible = r.intersect(painter.clip_rect());
    let mid = r.center().y;
    let half = r.height() / 2.0 - 2.0;
    painter.hline(visible.x_range(), mid, Stroke::new(1.0, with_alpha(color, 60)));
    for (x, lo, hi) in waveform_columns(level, r, visible, in_s, out_s) {
        let (top, bottom) = (mid - hi * half, mid - lo * half);
        if bottom - top >= 0.5 {
            painter.vline(x + 0.5, top..=bottom.max(top + 1.0), Stroke::new(1.0, color));
        }
    }
}

fn paint_clip(painter: &Painter, l: &Layout, c: &ClipBox, song: &Song, s: &Session) {
    let visible = c.rect.intersect(l.body);
    if visible.width() <= 0.0 {
        return;
    }
    let selected = s.timeline.selection.contains(&c.id);
    let p = painter.with_clip_rect(l.body);
    let issues = s.issues_for(&c.id);
    let accepted = song.motion_clip(&c.id).is_some_and(|m| m.accepted.is_some());
    let warn = !issues.is_empty();

    if c.kind == ClipKind::AxisPart {
        let Lane::Axis(_, axis) = c.lane else { return };
        let color = axis_color(axis);
        let bar = c.rect.shrink2(vec2(0.0, U));
        p.rect_filled(bar, RADIUS, with_alpha(color, if selected { 70 } else { 36 }));
        let Some(clip) = song.motion_clip(&c.id) else { return };
        // The axis curve inside the clip, scaled to the lane.
        let ev = ptz_core::eval::Evaluator::new(song, &clip.camera);
        let steps = (bar.width() / 3.0).clamp(2.0, 400.0) as usize;
        let values: Vec<f64> = (0..=steps)
            .map(|i| ev.axis_at(axis, clip.start_frame as f64 + clip.duration_frames as f64 * i as f64 / steps as f64))
            .collect();
        let (lo, hi) = values.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let span = (hi - lo).max(1e-9);
        let pts: Vec<Pos2> = values
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let t = if hi - lo < 1e-9 { 0.5 } else { (v - lo) / span };
                pos2(bar.left() + bar.width() * i as f32 / steps as f32, bar.bottom() - 2.0 - (bar.height() - 4.0) * t as f32)
            })
            .collect();
        p.line(pts, Stroke::new(1.5, color));
        if warn && issues.iter().any(|i| issue_axis(i) == Some(axis)) {
            p.rect_stroke(bar, RADIUS, Stroke::new(RULE, AMBER), StrokeKind::Inside);
        }
        return;
    }

    let fill = clip_fill(c.kind, selected);
    p.rect_filled(c.rect, RADIUS, fill);
    match c.kind {
        ClipKind::Audio | ClipKind::Video => {
            if let Some(m) = song.media_clip(&c.id) {
                let wave = if c.kind == ClipKind::Video {
                    let strip = filmstrip_rect(c.rect);
                    let pictures = p.with_clip_rect(visible.intersect(strip));
                    pictures.rect_filled(strip, 0.0, BASE);
                    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
                    for tile in filmstrip_tiles(strip, visible, m, l.px_per_frame) {
                        if let Some(texture) = s.frames.ready(&crate::frames::key(&m.media_id, tile.frame, true)) {
                            let tint = if selected { Color32::WHITE } else { Color32::from_gray(215) };
                            pictures.image(texture.id(), tile.rect, uv, tint);
                        }
                        pictures.vline(tile.rect.left(), tile.rect.y_range(), Stroke::new(1.0, with_alpha(BASE, 160)));
                    }
                    c.rect.with_min_y(strip.bottom() + 1.0).shrink2(vec2(0.0, 1.0))
                } else {
                    c.rect.shrink2(vec2(0.0, 2.0))
                };
                if let Some(level) = s.peaks.get(&m.media_id) {
                    let fps = s.project.rate.fps();
                    let pc = p.with_clip_rect(visible);
                    let color = if m.muted { with_alpha(OFF_WHITE, 50) } else { with_alpha(OFF_WHITE, 170) };
                    let wave = Rect::from_x_y_ranges(c.rect.x_range(), wave.y_range());
                    paint_waveform(&pc, wave, level, m.in_frame as f64 / fps, m.out_frame as f64 / fps, color);
                }
                let label = if m.muted { format!("{} (muted)", m.name) } else { m.name.clone() };
                if c.kind == ClipKind::Video {
                    // A plate under the name, so it reads over any picture.
                    let text_w = label.chars().count() as f32 * 6.2 + 10.0;
                    let plate = Rect::from_min_size(c.rect.left_top() + vec2(CLIP_EDGE + 1.0, 2.0), vec2(text_w, 4.0 * U));
                    p.with_clip_rect(visible).rect_filled(plate, RADIUS, plate_fill());
                }
                p.with_clip_rect(visible.shrink(2.0)).text(
                    c.rect.left_top() + vec2(CLIP_EDGE + U, U),
                    Align2::LEFT_TOP,
                    label,
                    small(),
                    OFF_WHITE,
                );
            }
        }
        ClipKind::Move | ClipKind::Keys => {
            if let Some(clip) = song.motion_clip(&c.id) {
                // The move's shape: its fastest axis, start to end.
                if c.rect.width() > 24.0 {
                    let ev = ptz_core::eval::Evaluator::new(song, &clip.camera);
                    let limits = s.project.axes_of(&clip.camera);
                    let axis = clip
                        .axes
                        .iter()
                        .copied()
                        .max_by(|a, b| {
                            let range = |ax: Axis| {
                                let lim = limits.get(ax);
                                let d = ev.axis_at(ax, clip.end_frame() as f64) - ev.axis_at(ax, clip.start_frame as f64);
                                (d / (lim.max - lim.min)).abs()
                            };
                            range(*a).total_cmp(&range(*b))
                        })
                        .unwrap_or(Axis::Pan);
                    let a = ev.axis_at(axis, clip.start_frame as f64);
                    let b = ev.axis_at(axis, clip.end_frame() as f64);
                    if (b - a).abs() > 1e-9 {
                        let steps = (c.rect.width() / 4.0).clamp(4.0, 200.0) as usize;
                        let pts: Vec<Pos2> = (0..=steps)
                            .map(|i| {
                                let f = clip.start_frame as f64 + clip.duration_frames as f64 * i as f64 / steps as f64;
                                let t = ((ev.axis_at(axis, f) - a) / (b - a)).clamp(-0.2, 1.2) as f32;
                                pos2(
                                    c.rect.left() + c.rect.width() * i as f32 / steps as f32,
                                    c.rect.bottom() - 3.0 - (c.rect.height() - 6.0) * t,
                                )
                            })
                            .collect();
                        p.with_clip_rect(visible).line(pts, Stroke::new(RULE, with_alpha(clip_ink(c.kind), 150)));
                    }
                }
                if let ClipBody::Keys { curves } = &clip.body {
                    let mut frames: Vec<i64> = curves.values().flatten().map(|k| k.frame).collect();
                    frames.sort_unstable();
                    frames.dedup();
                    if (frames.len() as f32) * 7.0 < c.rect.width() * 1.5 {
                        for f in frames {
                            let x = l.x_of((clip.start_frame + f) as f64);
                            diamond(&p.with_clip_rect(visible), pos2(x, c.rect.bottom() - 7.0), 3.0, OFF_WHITE, Stroke::NONE);
                        }
                    }
                }
                if warn {
                    hatch(&p.with_clip_rect(visible), c.rect, with_alpha(AMBER, if accepted { 60 } else { 130 }));
                }
                let mut text = timeline::move_label(song, &c.id, &s.project);
                if let ClipBody::PresetMove { auto_duration: false, .. } = &clip.body {
                    text.push_str("  custom");
                }
                if warn {
                    if let Some(first) = issues.first() {
                        text = format!("{}: {text}", issue_short(first));
                    }
                }
                p.with_clip_rect(visible.shrink(2.0)).text(
                    pos2(c.rect.left().max(l.body.left()) + CLIP_EDGE + U, c.rect.top() + U),
                    Align2::LEFT_TOP,
                    text,
                    small(),
                    OFF_WHITE,
                );
            }
        }
        ClipKind::AxisPart => {}
    }
    // What it is, down its leading edge.
    let edge = Rect::from_min_size(c.rect.min, vec2(CLIP_EDGE.min(c.rect.width()), c.rect.height()));
    p.rect_filled(edge, 0.0, clip_ink(c.kind));
    // An issue is an amber rule round the clip; the selection is a white one.
    let stroke = if warn {
        Stroke::new(if selected { RULE_STRONG } else { 1.5 }, AMBER)
    } else if selected {
        Stroke::new(1.5, Color32::WHITE)
    } else {
        Stroke::NONE
    };
    p.rect_stroke(c.rect, RADIUS, stroke, StrokeKind::Inside);
    if selected && warn {
        p.rect_stroke(c.rect.expand(1.0), RADIUS, Stroke::new(RULE, Color32::WHITE), StrokeKind::Outside);
    }
}

fn issue_axis(issue: &ptz_core::compile::Issue) -> Option<Axis> {
    use ptz_core::compile::Issue::*;
    match issue {
        TooFast { axis, .. } | TooMuchAcceleration { axis, .. } | OutOfRange { axis, .. } | Jump { axis, .. } => Some(*axis),
        KeepOut { .. } | MissingCamera { .. } => None,
    }
}

fn issue_short(issue: &ptz_core::compile::Issue) -> String {
    use ptz_core::compile::Issue::*;
    match issue {
        TooFast { axis, .. } => format!("{} too fast", axis.label()),
        TooMuchAcceleration { axis, .. } => format!("{} too abrupt", axis.label()),
        OutOfRange { axis, .. } => format!("{} out of range", axis.label()),
        Jump { axis, .. } => format!("{} jumps", axis.label()),
        KeepOut { .. } => "Keep-out".into(),
        MissingCamera { .. } => "No camera".into(),
    }
}

fn paint(painter: &Painter, l: &Layout, song: &Song, s: &Session, playhead: f64) {
    painter.rect_filled(l.rect, 0.0, LANE);

    // Lanes.
    for (i, lane) in l.lanes.iter().enumerate() {
        let fill = match lane.lane {
            Lane::Axis(..) => PANEL_DARK,
            _ if i % 2 == 0 => LANE,
            _ => LANE_ALT,
        };
        painter.rect_filled(lane.rect, 0.0, fill);
        painter.hline(l.rect.left()..=l.rect.right(), lane.rect.bottom(), Stroke::new(RULE, LINE));
    }
    // Past the end of the song.
    let end_x = l.x_of(song.length_frames as f64);
    if end_x < l.body.right() {
        let beyond = Rect::from_min_max(pos2(end_x.max(l.body.left()), l.body.top()), l.body.right_bottom());
        painter.rect_filled(beyond, 0.0, with_alpha(Color32::BLACK, 90));
        painter.vline(end_x.max(l.body.left()), l.body.top()..=l.body.bottom(), Stroke::new(RULE, CONTROL_LINE));
    }

    // Ruler grid lines through the lanes.
    let (major, _) = timeline::tick_step(l.px_per_frame, s.project.rate.fps());
    let mut f = ((l.scroll.max(0.0) as i64) / major) * major;
    let last = l.frame_at(l.body.right()).ceil() as i64;
    let pb = painter.with_clip_rect(l.body);
    while f <= last.min(song.length_frames) {
        pb.vline(l.x_of(f as f64), l.body.top()..=l.body.bottom(), Stroke::new(RULE, with_alpha(LINE, 130)));
        f += major;
    }

    // Marker lines under the clips.
    for m in &l.markers {
        pb.vline(m.x, l.body.top()..=l.body.bottom(), Stroke::new(RULE, with_alpha(OFF_WHITE, 50)));
    }

    for c in &l.clips {
        paint_clip(painter, l, c, song, s);
    }

    // Keys on the axis lanes.
    for k in &l.keys {
        if k.pos.x < l.body.left() - 6.0 || k.pos.x > l.body.right() + 6.0 {
            continue;
        }
        let selected = s.timeline.key_selection.contains(&(k.clip.clone(), k.axis, k.index));
        let fill = if selected { OFF_WHITE } else { axis_color(k.axis) };
        diamond(&pb, k.pos, 5.0, fill, Stroke::new(RULE, if selected { ACCENT } else { PANEL_DARK }));
    }

    // Marker flags.
    if let Some(lane) = l.lane_rect(Lane::Markers) {
        for (m, mk) in l.markers.iter().zip(&song.markers) {
            let selected = s.timeline.selection.contains(&m.id);
            // A marker is a place, not a warning: plain ink, and white when selected.
            let color = if selected { Color32::WHITE } else { mix(MUTED, OFF_WHITE, 0.5) };
            let flag = vec![pos2(m.x, lane.top() + 3.0), pos2(m.x + 8.0, lane.top() + 7.0), pos2(m.x, lane.top() + 11.0)];
            pb.add(egui::Shape::convex_polygon(flag, color, Stroke::NONE));
            pb.vline(m.x, lane.top() + 3.0..=lane.bottom(), Stroke::new(1.5, color));
            pb.text(pos2(m.x + 3.0 * U, lane.center().y), Align2::LEFT_CENTER, &mk.name, small(), color);
        }
    }

    // Header column.
    let header = Rect::from_min_max(pos2(l.rect.left(), l.ruler.bottom()), pos2(l.body.left(), l.rect.bottom()));
    painter.rect_filled(header, 0.0, PANEL);
    for lane in &l.lanes {
        let y = lane.rect.center().y;
        match lane.lane {
            Lane::Motion(camera) => {
                let many = s.project.cameras.len() > 1;
                let active = camera == s.cam_index();
                if many && active {
                    // The camera in hand: a lighter ground and the accent down its leading edge.
                    let r = Rect::from_min_max(lane.rect.left_top(), pos2(l.body.left(), lane.rect.bottom()));
                    painter.rect_filled(r, 0.0, CONTROL);
                    painter.rect_filled(Rect::from_min_size(r.left_top(), vec2(CLIP_EDGE, r.height())), 0.0, ACCENT);
                }
                let c = l.twirl(camera).map(|t| t.center()).unwrap_or(pos2(l.rect.left() + 13.0, y));
                let pts = if s.timeline.view.is_open(camera) {
                    vec![pos2(c.x - 5.0, c.y - 3.0), pos2(c.x + 5.0, c.y - 3.0), pos2(c.x, c.y + 4.0)]
                } else {
                    vec![pos2(c.x - 3.0, c.y - 5.0), pos2(c.x + 4.0, c.y), pos2(c.x - 3.0, c.y + 5.0)]
                };
                painter.add(egui::Shape::convex_polygon(pts, OFF_WHITE, Stroke::NONE));
                if many {
                    // The camera's number in its colour, then its name.
                    let chip = Rect::from_center_size(pos2(l.rect.left() + 36.0, y), NUMBER_TAG);
                    number_tag(painter, chip, camera + 1, camera_color(camera));
                    let name = s.project.cameras.get(camera).map(|c| c.name.as_str()).unwrap_or("");
                    let text = painter.layout_no_wrap(name.to_string(), small(), OFF_WHITE);
                    let clip = Rect::from_min_max(
                        pos2(chip.right() + GAP, lane.rect.top()),
                        pos2(l.body.left() - 4.0, lane.rect.bottom()),
                    );
                    painter.with_clip_rect(clip).galley(pos2(clip.left(), y - text.size().y / 2.0), text, OFF_WHITE);
                } else {
                    painter.text(pos2(l.rect.left() + 26.0, y), Align2::LEFT_CENTER, lane.lane.label(), small(), OFF_WHITE);
                }
            }
            Lane::Missing => {
                painter.text(pos2(l.rect.left() + PAD, y), Align2::LEFT_CENTER, lane.lane.label(), small(), AMBER);
            }
            Lane::Axis(camera, axis) => {
                let mine = camera == s.cam_index();
                let chosen = mine && s.axis_lanes.contains(&axis);
                if chosen {
                    let r = Rect::from_min_max(
                        pos2(l.rect.left() + 22.0, lane.rect.top() + 3.0),
                        pos2(l.body.left() - 28.0, lane.rect.bottom() - 3.0),
                    );
                    painter.rect_filled(r, RADIUS, with_alpha(ACCENT, 90));
                }
                painter.circle_filled(pos2(l.rect.left() + 32.0, y), 3.5, axis_color(axis));
                let ink = if chosen { OFF_WHITE } else { MUTED };
                painter.text(pos2(l.rect.left() + 44.0, y), Align2::LEFT_CENTER, axis.label(), small(), ink);
                // Record-enable: a ring when off, filled when Record will write this axis, lit during a pass.
                let records = s.project.camera_index(s.record_camera_shown()) == Some(camera);
                if let Some(c) = record_dot(l, camera, axis) {
                    if records && s.record_axes_shown().contains(&axis) {
                        let live = s.status.transport.record;
                        painter.circle_filled(c, 5.5, if live { RED } else { mix(RED, PANEL, 0.35) });
                        if s.status.record.active {
                            painter.circle_stroke(c, 8.0, Stroke::new(1.5, RED));
                        }
                    } else {
                        painter.circle_stroke(c, 5.0, Stroke::new(1.5, mix(RED, PANEL, 0.55)));
                    }
                }
            }
            other => {
                painter.text(pos2(l.rect.left() + PAD, y), Align2::LEFT_CENTER, other.label(), small(), OFF_WHITE);
            }
        }
        painter.hline(header.left()..=header.right(), lane.rect.bottom(), Stroke::new(RULE, LINE));
    }
    painter.vline(l.body.left(), l.rect.top()..=l.rect.bottom(), Stroke::new(RULE, LINE));

    paint_ruler(painter, l, s);

    // Playhead over everything.
    let x = l.x_of(playhead);
    if x >= l.body.left() - 1.0 && x <= l.rect.right() + 1.0 {
        let color = playhead_ink(&s.status.transport, s.status.hardware_live);
        painter.vline(x, l.ruler.top() + 8.0..=l.rect.bottom(), Stroke::new(1.5, color));
        let head = vec![pos2(x - 6.0, l.ruler.top() + 2.0), pos2(x + 6.0, l.ruler.top() + 2.0), pos2(x, l.ruler.top() + 13.0)];
        painter.add(egui::Shape::convex_polygon(head, color, Stroke::NONE));
    }
}
