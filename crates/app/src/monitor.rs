//! Program monitor: what the selected camera sees of the stage from where it stands, through its
//! lens, at its pose. It stands in for the camera picture, so a move can be judged before any hardware is live.

use crate::session::Session;
use crate::stage;
use crate::theme::*;
use egui::{pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Sense, Stroke, StrokeKind, Ui};
use ptz_core::api::PoseSource;
use ptz_core::pose::Axis;

/// The widest a monitor picture is drawn, in pixels; a larger window scales it up.
pub const WIDEST_PX: f32 = 1280.0;

pub fn badge(painter: &Painter, at: Pos2, anchor: Align2, text: &str, fill: Color32, ink: Color32) -> Rect {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(12.0), ink);
    let size = galley.size() + vec2(14.0, 6.0);
    let r = anchor.anchor_size(at, size);
    painter.rect_filled(r, 4.0, fill);
    painter.galley(r.min + vec2(7.0, 3.0), galley, ink);
    r
}

/// Frame guides: thirds and a centre cross.
pub fn guides(painter: &Painter, rect: Rect) {
    let p = painter.with_clip_rect(rect);
    let guide = Stroke::new(1.0, with_alpha(OFF_WHITE, 28));
    for t in [1.0 / 3.0, 2.0 / 3.0] {
        p.vline(rect.left() + rect.width() * t, rect.y_range(), guide);
        p.hline(rect.x_range(), rect.top() + rect.height() * t, guide);
    }
    let c = rect.center();
    p.line_segment([c - vec2(8.0, 0.0), c + vec2(8.0, 0.0)], Stroke::new(1.0, with_alpha(OFF_WHITE, 120)));
    p.line_segment([c - vec2(0.0, 8.0), c + vec2(0.0, 8.0)], Stroke::new(1.0, with_alpha(OFF_WHITE, 120)));
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
    painter.rect_filled(area, 0.0, PANEL_DARK);

    let readout_h = 26.0;
    let room = Rect::from_min_max(area.min + vec2(8.0, 8.0), area.max - vec2(8.0, readout_h + 8.0));
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
    let border = if t.estop {
        RED
    } else if live {
        GREEN
    } else {
        LINE
    };
    painter.rect_stroke(pic, 0.0, Stroke::new(if live || t.estop { 2.0 } else { 1.0 }, border), StrokeKind::Outside);

    let simulated = !s.cam_status().hardware;
    let held = s.cam_status().held;
    let (text, fill_c, ink) = if t.estop {
        ("E-STOP".to_string(), RED, OFF_WHITE)
    } else if held {
        ("HELD  this camera stopped where it was".to_string(), RED, OFF_WHITE)
    } else if live {
        (if simulated { "LIVE  simulator".to_string() } else { "LIVE  rig is moving".to_string() }, GREEN, BASE)
    } else {
        ("REHEARSE  nothing is sent to the rig".to_string(), with_alpha(BASE, 230), OFF_WHITE)
    };
    badge(&painter, pic.left_top() + vec2(8.0, 8.0), Align2::LEFT_TOP, &text, fill_c, ink);
    let what = match source {
        PoseSource::Timeline => "Programmed pose",
        PoseSource::Rig => "Rig pose",
        PoseSource::Record => "Recording: rig on record axes",
    };
    badge(&painter, pic.right_top() + vec2(-8.0, 8.0), Align2::RIGHT_TOP, what, with_alpha(PANEL_DARK, 220), OFF_WHITE);
    if s.status.transport.record {
        painter.circle_filled(pic.right_top() + vec2(-16.0, 44.0), 6.0, RED);
    }
    badge(
        &painter,
        pic.left_bottom() + vec2(8.0, -8.0),
        Align2::LEFT_BOTTOM,
        &s.timecode_at(s.playhead().floor() as i64),
        with_alpha(PANEL_DARK, 220),
        GREEN,
    );
    let camera = s.cam();
    let lens = format!("{}   {:.0} mm", s.project.label(&camera.id), camera.lens.focal_mm(pose.zoom));
    badge(
        &painter,
        pic.right_bottom() + vec2(-8.0, -8.0),
        Align2::RIGHT_BOTTOM,
        &lens,
        with_alpha(PANEL_DARK, 220),
        camera_color(index),
    );
    if let Some(wait) = stage_wait(s) {
        badge(&painter, pic.center_bottom() + vec2(0.0, -8.0), Align2::CENTER_BOTTOM, wait, with_alpha(PANEL_DARK, 220), AMBER);
    }

    // Pose readout under the picture.
    let y = pic.bottom() + 6.0 + readout_h / 2.0;
    let step = (pic.width() / 5.0).min(150.0);
    for (i, axis) in Axis::ALL.iter().enumerate() {
        let x = pic.left() + step * i as f32;
        painter.circle_filled(pos2(x + 4.0, y), 3.5, axis_color(*axis));
        painter.text(
            pos2(x + 12.0, y),
            Align2::LEFT_CENTER,
            format!("{} {}", axis.label(), axis.format_in(pose.get(*axis), s.project.units)),
            FontId::proportional(11.5),
            OFF_WHITE,
        );
    }
}
