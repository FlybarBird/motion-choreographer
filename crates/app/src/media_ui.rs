//! Media browser and the reference monitor.

use crate::panels::note;
use crate::session::Session;
use egui::Ui;

pub fn media(ui: &mut Ui, s: &mut Session) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| {
        if s.media.is_empty() {
            note(ui, "No media has been imported.");
        }
    });
}

pub fn reference(ui: &mut Ui, _s: &mut Session) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| note(ui, "No reference video is on the timeline."));
}
