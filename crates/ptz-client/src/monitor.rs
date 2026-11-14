//! Simulated program monitor — synthetic stage framed by live PTZ/pedestal pose.

use crate::theme::{BLUE, CHARCOAL, GREEN, NAVY, OFF_WHITE};
use egui::{Color32, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use ptz_core::RigPose;

/// Draw a moving “camera view” of a stage so the operator sees moves during playback.
pub fn draw_program_monitor(ui: &mut Ui, pose: &RigPose, playing: bool, preview: bool) -> egui::Response {
    let desired = Vec2::new(ui.available_width(), ui.available_height().max(180.0));
    let (resp, painter) = ui.allocate_painter(desired, Sense::hover());
    let rect = resp.rect;

    painter.rect_filled(rect, 0.0, CHARCOAL);

    let pan_n = (pose.pan_deg as f32 / 60.0).clamp(-1.5, 1.5);
    let tilt_n = (pose.tilt_deg as f32 / 40.0).clamp(-1.5, 1.5);
    let zoom = (0.35 + pose.zoom as f32 * 1.8).clamp(0.25, 3.0);
    let elev = ((pose.elevation_m as f32 - 1.0) / 1.5).clamp(-0.5, 1.5);

    let center = rect.center() + Vec2::new(-pan_n * rect.width() * 0.35, tilt_n * rect.height() * 0.25);
    let stage_w = rect.width() * 0.55 / zoom;
    let stage_h = rect.height() * 0.22 / zoom;
    let stage = Rect::from_center_size(
        center + Vec2::new(0.0, 20.0 - elev * 30.0),
        Vec2::new(stage_w, stage_h),
    );

    let backdrop = Rect::from_center_size(
        center + Vec2::new(0.0, -stage_h * 1.1 - elev * 10.0),
        Vec2::new(stage_w * 1.35, stage_h * 2.2),
    );
    painter.rect_filled(backdrop, 4.0, Color32::from_rgb(0x12, 0x1a, 0x3a));
    painter.rect_stroke(backdrop, 4.0, Stroke::new(1.5_f32, BLUE), egui::StrokeKind::Outside);

    painter.rect_filled(stage, 2.0, Color32::from_rgb(0x3a, 0x2f, 0x22));
    painter.rect_stroke(
        stage,
        2.0,
        Stroke::new(1.0_f32, OFF_WHITE.gamma_multiply(0.35)),
        egui::StrokeKind::Outside,
    );

    draw_talent(
        &painter,
        stage.center() + Vec2::new(-stage_w * 0.22, -stage_h * 0.8),
        zoom,
        "LECTERN",
    );
    draw_talent(
        &painter,
        stage.center() + Vec2::new(stage_w * 0.18, -stage_h * 0.75),
        zoom,
        "A",
    );
    draw_talent(
        &painter,
        stage.center() + Vec2::new(stage_w * 0.32, -stage_h * 0.75),
        zoom,
        "B",
    );

    painter.rect_stroke(rect.shrink(2.0), 0.0, Stroke::new(2.0_f32, NAVY), egui::StrokeKind::Inside);
    painter.rect_stroke(
        rect.shrink(rect.width() * 0.05),
        0.0,
        Stroke::new(1.0_f32, GREEN.gamma_multiply(0.35)),
        egui::StrokeKind::Inside,
    );

    let mode = if preview {
        "PREVIEW"
    } else if playing {
        "LIVE"
    } else {
        "PGM"
    };
    let hud = format!(
        "{mode}  pan {:+.1}°  tilt {:+.1}°  zoom {:.0}%  elev {:.2}m",
        pose.pan_deg,
        pose.tilt_deg,
        pose.zoom * 100.0,
        pose.elevation_m
    );
    painter.text(
        Pos2::new(rect.left() + 10.0, rect.top() + 8.0),
        egui::Align2::LEFT_TOP,
        hud,
        egui::FontId::monospace(12.0),
        if preview {
            BLUE
        } else if playing {
            GREEN
        } else {
            OFF_WHITE
        },
    );

    if playing || preview {
        let t = ui.input(|i| i.time);
        let y = rect.top() + ((t * 40.0) as f32 % rect.height());
        painter.hline(
            rect.x_range(),
            y,
            Stroke::new(1.0_f32, OFF_WHITE.gamma_multiply(0.08)),
        );
    }

    resp
}

fn draw_talent(painter: &egui::Painter, at: Pos2, zoom: f32, label: &str) {
    let h = (36.0 / zoom).clamp(8.0, 48.0);
    let w = h * 0.45;
    let body = Rect::from_center_size(at, Vec2::new(w, h));
    painter.rect_filled(body, 2.0, Color32::from_rgb(0xc8, 0xc2, 0xb4));
    painter.circle_filled(
        at - Vec2::new(0.0, h * 0.65),
        w * 0.55,
        Color32::from_rgb(0xe8, 0xd8, 0xc0),
    );
    painter.text(
        at + Vec2::new(0.0, h * 0.7),
        egui::Align2::CENTER_TOP,
        label,
        egui::FontId::proportional((10.0 / zoom).clamp(7.0, 12.0)),
        OFF_WHITE,
    );
}
