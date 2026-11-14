//! Graph editor: value and speed curves for the keys clips.

use crate::panels::note;
use crate::session::Session;
use egui::Ui;
use ptz_core::edit;
use ptz_core::pose::Axis;

/// Add a key on every axis at the playhead (the K shortcut).
pub fn add_key_at_playhead(s: &mut Session) {
    let frame = s.playhead().round() as i64;
    s.edit(|song, project| edit::add_key(song, project, &Axis::ALL, frame));
}

pub fn show(ui: &mut Ui, _s: &mut Session) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| note(ui, "Select a keys clip to see its curves."));
}
