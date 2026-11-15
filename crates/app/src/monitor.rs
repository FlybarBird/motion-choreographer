//! Program monitor: what the selected camera sees of the stage from where it stands, through its
//! lens, at its pose. It stands in for the camera picture, so a move can be judged before any hardware is live.

use crate::session::Session;
use crate::stage;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
use ptz_core::api::PoseSource;
use ptz_core::pose::Axis;

/// The widest a monitor picture is drawn, in pixels; a larger window scales it up.
pub const WIDEST_PX: f32 = 1280.0;

/// A word on a plate over a picture, drawn as every such plate in the app is.
pub fn badge(painter: &Painter, at: Pos2, anchor: Align2, text: &str, fill: Color32, ink: Color32) -> Rect {
    plate(painter, at, anchor, text, fill, ink)
}

/// Frame guides: thirds and a centre cross.
pub fn guides(painter: &Painter, rect: Rect) {
    let p = painter.with_clip_rect(rect);
    let guide = Stroke::new(RULE, with_alpha(OFF_WHITE, 28));
    for t in [1.0 / 3.0, 2.0 / 3.0] {
        p.vline(rect.left() + rect.width() * t, rect.y_range(), guide);
        p.hline(rect.x_range(), rect.top() + rect.height() * t, guide);
    }
    let c = rect.center();
    p.line_segment([c - vec2(GAP, 0.0), c + vec2(GAP, 0.0)], Stroke::new(RULE, with_alpha(OFF_WHITE, 120)));
    p.line_segment([c - vec2(0.0, GAP), c + vec2(0.0, GAP)], Stroke::new(RULE, with_alpha(OFF_WHITE, 120)));
}

/// The largest 16:9 rectangle that fits in `room`, centred.
pub fn picture_rect(room: Rect) -> Rect {
    let (w, h) = if room.width() / room.height() > 16.0 / 9.0 {
        (room.height() * 16.0 / 9.0, room.height())
    } else {
        (room.width(), room.width() * 9.0 / 16.0)
    };
    Rect::from_center_size(room.center(), vec2(w, h))
}

/// What is being waited for before the stage is as the project says, in words.
pub fn stage_wait(s: &Session) -> Option<&'static str> {
    if s.world.waiting_for_model(&s.project.stage) {
        Some("Fetching the stage model")
    } else if s.world.is_building() {
        Some("Preparing the stage model")
    } else {
        None
    }
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    let avail = ui.available_size();
    let (resp, painter) = ui.allocate_painter(avail, Sense::hover());
    let area = resp.rect;
    painter.rect_filled(area, 0.0, BASE);

    let readout_h = CONTROL_H;
    let room = Rect::from_min_max(area.min + vec2(GAP, GAP), area.max - vec2(GAP, readout_h + GAP));
    if room.width() < 40.0 || room.height() < 30.0 {
        return;
    }
    let pic = picture_rect(room);

    let (pose, source) = s.monitor_pose();
    let index = s.cam_index();
    let size = stage::pixel_size(pic, ui.ctx().pixels_per_point(), WIDEST_PX);
    let mut poses = s.poses();
    poses[index] = pose;
    let texture = stage::camera_picture(ui.ctx(), s, "program", index, &poses, size, true);
    stage::paint_picture(&painter, pic, texture);
    guides(&painter, pic);

    let live = s.status.hardware_live;
    let t = &s.status.transport;
    let simulated = !s.cam_status().hardware;
    let held = s.cam_status().held;
    // The picture is framed in the state of the machine, as the rule under the transport is, and in
    // red while this camera is held where it stopped.
    let machine = machine_ink(t, live);
    let frame = if held { Some(RED) } else { machine };
    let rule = Stroke::new(if frame.is_some() { RULE_STRONG } else { RULE }, frame.unwrap_or(CONTROL_LINE));
    painter.rect_stroke(pic, 0.0, rule, StrokeKind::Outside);

    let (text, fill_c, ink) = if t.estop {
        ("E-STOP".to_string(), RED, ink_on(RED))
    } else if held {
        ("HELD  this camera stopped where it was".to_string(), RED, ink_on(RED))
    } else if live {
        let fill = machine.unwrap_or(GREEN);
        (if simulated { "LIVE  simulator".to_string() } else { "LIVE  rig is moving".to_string() }, fill, ink_on(fill))
    } else {
        ("REHEARSE  nothing is sent to the rig".to_string(), plate_fill(), OFF_WHITE)
    };
    let what = match source {
        PoseSource::Timeline => "Programmed pose",
        PoseSource::Rig => "Rig pose",
        PoseSource::Record => "Recording: rig on record axes",
    };
    // In a narrow picture the state is said in its one word, so that it does not run under the plate beside it.
    let width = |text: &str| painter.layout_no_wrap(text.to_string(), small(), ink).size().x + 2.0 * GAP;
    let state =
        if width(&text) + width(what) + 3.0 * GAP > pic.width() { text.split("  ").next().unwrap_or(&text) } else { &text };
    badge(&painter, pic.left_top() + vec2(GAP, GAP), Align2::LEFT_TOP, state, fill_c, ink);
    let source = badge(&painter, pic.right_top() + vec2(-GAP, GAP), Align2::RIGHT_TOP, what, plate_fill(), OFF_WHITE);
    if s.status.transport.record {
        painter.circle_filled(source.right_bottom() + vec2(-GAP, 2.0 * GAP), 6.0, RED);
    }
    badge(
        &painter,
        pic.left_bottom() + vec2(GAP, -GAP),
        Align2::LEFT_BOTTOM,
        &s.timecode_at(s.playhead().floor() as i64),
        plate_fill(),
        clock_ink(t, live),
    );
    let camera = s.cam();
    let lens = format!("{}   {:.0} mm", s.project.label(&camera.id), camera.lens.focal_mm(pose.zoom));
    badge(&painter, pic.right_bottom() + vec2(-GAP, -GAP), Align2::RIGHT_BOTTOM, &lens, plate_fill(), camera_color(index));
    if let Some(wait) = stage_wait(s) {
        badge(&painter, pic.center_bottom() + vec2(0.0, -GAP), Align2::CENTER_BOTTOM, wait, plate_fill(), AMBER);
    }

    // Pose readout under the picture: each axis as its colour, its name, and its value in figures.
    // Where the picture is too narrow for the names they are cut to their first letters.
    let y = pic.bottom() + U + readout_h / 2.0;
    let swatch = 2.0 * U;
    let values =
        Axis::ALL.map(|axis| painter.layout_no_wrap(axis.format_in(pose.get(axis), s.project.units), mono(SMALL), OFF_WHITE));
    let named = |short: bool| {
        Axis::ALL.map(|axis| {
            let name = if short { &axis.label()[..1] } else { axis.label() };
            painter.layout_no_wrap(name.to_string(), small(), MUTED)
        })
    };
    let needs = |names: &[std::sync::Arc<egui::Galley>; 5]| {
        names.iter().zip(&values).map(|(n, v)| swatch + U + n.size().x + U + v.size().x).sum::<f32>() + 4.0 * GAP
    };
    let names = Some(named(false)).filter(|n| needs(n) <= pic.width()).unwrap_or_else(|| named(true));
    let gap = ((pic.width() - needs(&names)) / 4.0 + GAP).clamp(U, 6.0 * U);
    let mut x = pic.left();
    for ((axis, name), value) in Axis::ALL.iter().zip(names).zip(values) {
        painter.rect_filled(Rect::from_min_size(pos2(x, y - swatch / 2.0), vec2(swatch, swatch)), 0.0, axis_color(*axis));
        x += swatch + U;
        let (name_w, value_w) = (name.size().x, value.size().x);
        painter.galley(pos2(x, y - name.size().y / 2.0), name, MUTED);
        x += name_w + U;
        painter.galley(pos2(x, y - value.size().y / 2.0), value, OFF_WHITE);
        x += value_w + gap;
    }
}
