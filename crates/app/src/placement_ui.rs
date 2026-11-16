//! Camera placement: adjust the installation while looking through its lens.

use crate::session::Session;
use crate::stage_ui::{self, Probe};
use crate::theme::*;
use egui::{vec2, Align2, Sense, Ui};

/// Abandon an unfinished preview drag, including when selection or permission changes.
pub fn cancel_drag(s: &mut Session) {
    if let Some((id, from)) = s.stage_view.preview_drag.take() {
        if let Some(index) = s.project.camera_index(&id) {
            s.project.cameras[index].mount = from;
        }
    }
}

fn preview(ui: &mut Ui, s: &mut Session) {
    let size = vec2(ui.available_width(), ui.available_width() * 9.0 / 16.0);
    let (response, painter) = ui.allocate_painter(size, Sense::click_and_drag());
    let rect = response.rect;
    let editable = s.can_edit().is_ok();
    let wrong_camera = s.stage_view.preview_drag.as_ref().is_some_and(|(id, _)| id != s.cam_id());
    if !editable || wrong_camera || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel_drag(s);
    }
    if editable && response.drag_started_by(egui::PointerButton::Primary) {
        s.stage_view.preview_drag = Some((s.cam_id().to_owned(), s.cam().mount));
    }
    if response.dragged_by(egui::PointerButton::Primary) && s.stage_view.preview_drag.is_some() {
        let delta = response.drag_delta();
        let index = s.cam_index();
        let mount = &mut s.project.cameras[index].mount;
        mount.heading_deg = stage_ui::wrap_heading(mount.heading_deg - f64::from(delta.x) * 0.2);
        mount.tilt_deg = (mount.tilt_deg + f64::from(delta.y) * 0.2).clamp(-90.0, 90.0);
    }
    if response.drag_stopped() {
        if let Some((id, from)) = s.stage_view.preview_drag.take() {
            if let Some(index) = s.project.camera_index(&id) {
                stage_ui::commit(s, index, from);
            }
        }
    }

    let index = s.cam_index();
    let (pose, _) = s.monitor_pose();
    let mut poses = s.poses();
    poses[index] = pose;
    let camera = crate::stage::view_of(s.cam(), &pose);
    s.stage_view.placement_preview = Some(Probe { rect, camera });
    let size = crate::stage::pixel_size(rect, ui.ctx().pixels_per_point(), 720.0);
    let texture = crate::stage::camera_picture(ui.ctx(), s, "placement", index, &poses, size, true);
    crate::stage::paint_picture(&painter, rect, texture);
    crate::monitor::guides(&painter, rect);
    crate::monitor::badge(
        &painter,
        rect.left_top() + vec2(GAP, GAP),
        Align2::LEFT_TOP,
        "SIMULATED VIEW",
        plate_fill(),
        OFF_WHITE,
    );
    if let Some(wait) = crate::monitor::stage_wait(s) {
        crate::monitor::badge(&painter, rect.center(), Align2::CENTER_CENTER, wait, plate_fill(), AMBER);
    }
    response.on_hover_text("Drag the picture to adjust mounting heading and tilt. Escape cancels the drag");
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(s.project.label(s.cam_id())).color(camera_color(s.cam_index())).strong());
        if ui.button("Done placing").on_hover_text("Return to the timeline and saved panel layout").clicked() {
            cancel_drag(s);
            s.stage_view.placement = false;
            s.stage_view.fit_requested = true;
        }
    });
    preview(ui, s);
    ui.label(egui::RichText::new("Drag the preview to adjust the angle.").small().color(MUTED));

    let why = s.can_edit().err();
    let index = s.cam_index();
    let from = s.cam().mount;
    let mut mount = from;
    let units = s.project.units;
    egui::ScrollArea::vertical().id_salt("placement_controls").auto_shrink([false, false]).show(ui, |ui| {
        ui.add_enabled_ui(why.is_none() && s.stage_view.preview_drag.is_none(), |ui| {
            egui::Grid::new("placement_fields").num_columns(2).spacing([GAP, GAP]).show(ui, |ui| {
                for (label, value) in [("Placement X", &mut mount.x), ("Placement Y", &mut mount.y), ("Mount height", &mut mount.z)] {
                    let text = ui.label(label);
                    ui.add(crate::units::distance_box(value, units).range(-1000.0..=1000.0)).labelled_by(text.id);
                    ui.end_row();
                }
                let text = ui.label("Heading");
                ui.add(egui::DragValue::new(&mut mount.heading_deg).speed(0.5).range(-360.0..=360.0).suffix("°"))
                    .labelled_by(text.id);
                ui.end_row();
                let text = ui.label("Tilt offset");
                ui.add(egui::DragValue::new(&mut mount.tilt_deg).speed(0.2).range(-90.0..=90.0).suffix("°"))
                    .labelled_by(text.id);
                ui.end_row();
                let text = ui.label("Lens offset");
                ui.add(crate::units::height_box(&mut mount.base_height_mm, units).range(0.0..=5000.0))
                    .labelled_by(text.id);
                ui.end_row();
            });
            ui.checkbox(&mut mount.inverted, "Inverted mount");
        });
        if let Some(why) = &why {
            ui.label(egui::RichText::new(why.replace("to edit the song", "to place cameras")).small().color(AMBER));
        }
        let note = if mount.inverted {
            "Mount height is the ceiling or truss attachment. The lens hangs below it by the lens offset plus column extension."
        } else {
            "Mount height is the floor or riser. The lens sits above it by the lens offset plus column extension."
        };
        ui.label(egui::RichText::new(note).small().color(MUTED));
        ui.label(egui::RichText::new("Heading and tilt offset set the zero-angle view. The preview also includes the current camera pose and lens.").small().color(MUTED));
        if mount.inverted {
            ui.label(egui::RichText::new("Set image flip on the camera and check Pan / Tilt direction in Rig settings for an upright picture and correct movement.").small().color(MUTED));
        }
    });
    if mount != from && why.is_none() {
        s.project.cameras[index].mount = mount;
        stage_ui::commit(s, index, from);
    }
}
