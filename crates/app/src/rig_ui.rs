//! Rig controller: live pose readout and connection state.

use crate::panels::{note, section};
use crate::session::Session;
use crate::theme::*;
use egui::{RichText, Ui};
use ptz_core::pose::Axis;

pub fn show(ui: &mut Ui, s: &mut Session) {
    egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
        section(ui, "Rig");
        if s.status.adapters.is_empty() {
            note(ui, "The server has not reported a rig.");
        }
        for a in &s.status.adapters {
            ui.horizontal(|ui| {
                let (dot, word) = if a.connected { (GREEN, "connected") } else { (RED, "not connected") };
                ui.label(RichText::new("●").color(dot));
                ui.label(RichText::new(&a.name).strong());
                ui.label(RichText::new(word).color(MUTED));
            });
            if !a.detail.is_empty() {
                ui.label(RichText::new(&a.detail).color(MUTED).small());
            }
        }
        section(ui, "Position");
        egui::Grid::new("rig_pose").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            for axis in Axis::ALL {
                ui.label(RichText::new(axis.label()).color(axis_color(axis)));
                ui.label(RichText::new(axis.format(s.status.rig_pose.get(axis))).monospace());
                ui.end_row();
            }
        });
    });
}
