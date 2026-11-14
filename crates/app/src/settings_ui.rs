//! Project settings shown in the Inspector when nothing is selected.

use crate::link::Cmd;
use crate::panels::section;
use crate::session::Session;
use crate::theme::*;
use egui::{RichText, Ui};
use ptz_core::pose::Axis;

fn step_for(axis: Axis) -> f64 {
    match axis {
        Axis::Pan | Axis::Tilt => 0.5,
        Axis::Zoom | Axis::Focus => 0.01,
        Axis::Elevation => 5.0,
    }
}

pub fn project(ui: &mut Ui, s: &mut Session) {
    let editable = s.can_edit().is_ok();
    let mut project = s.project.clone();

    section(ui, "Rig limits");
    ui.label(
        RichText::new("Auto time and the too-fast warning come from these. Set them to what the rig does safely.")
            .color(MUTED)
            .small(),
    );
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("limits_grid").num_columns(5).spacing([8.0, 6.0]).show(ui, |ui| {
            for head in ["", "Min", "Max", "Top speed /s", "Accel /s2"] {
                ui.label(RichText::new(head).small().color(MUTED));
            }
            ui.end_row();
            for axis in Axis::ALL {
                let lim = project.axes.get_mut(axis);
                let step = step_for(axis);
                ui.label(RichText::new(axis.label()).color(axis_color(axis)));
                ui.add(egui::DragValue::new(&mut lim.min).speed(step).range(-100000.0..=lim.max - step));
                ui.add(egui::DragValue::new(&mut lim.max).speed(step).range(lim.min + step..=100000.0));
                ui.add(egui::DragValue::new(&mut lim.max_speed).speed(step).range(step..=100000.0));
                ui.add(egui::DragValue::new(&mut lim.max_accel).speed(step).range(step..=1000000.0));
                ui.end_row();
            }
        });
    });

    if project != s.project {
        // Shown at once; the server's answer replaces it.
        s.project = project.clone();
        if let Some(song) = &s.song {
            s.compiled = ptz_core::compile::compile(song, &s.project);
        }
        s.link.send(Cmd::PutProject(Box::new(project)));
    }
}
