//! The camera bar: every camera in the show, which one is selected, and how each is doing.

use crate::session::Session;
use crate::theme::*;
use egui::{vec2, Key, RichText, Stroke, Ui};
use ptz_core::api::CameraStatus;
use ptz_core::transport::Output;

/// The keys that select cameras 1 to 12.
pub const KEYS: [Key; 12] = [
    Key::Num1,
    Key::Num2,
    Key::Num3,
    Key::Num4,
    Key::Num5,
    Key::Num6,
    Key::Num7,
    Key::Num8,
    Key::Num9,
    Key::Num0,
    Key::Minus,
    Key::Equals,
];

pub fn key_name(index: usize) -> &'static str {
    ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "="].get(index).copied().unwrap_or("")
}

/// One word on a camera's state, and its colour.
pub fn state(status: Option<&CameraStatus>, output: Output) -> (&'static str, egui::Color32) {
    match status {
        Some(c) if c.fault.is_some() && c.held => ("held", RED),
        Some(c) if c.fault.is_some() => ("fault", RED),
        Some(c) if c.halting => ("stopping", RED),
        Some(c) if c.preset_sync.is_some() => ("reading", AMBER),
        Some(c) if c.hardware && c.adapters.iter().any(|a| !a.connected) => ("no answer", AMBER),
        Some(c) if c.hardware && output != Output::Rehearse => ("live", GREEN),
        Some(c) if c.hardware => ("rig", OFF_WHITE),
        _ => ("sim", MUTED),
    }
}

fn detail(s: &Session, index: usize) -> String {
    let camera = &s.project.cameras[index];
    let status = s.status.camera(&camera.id);
    let mut lines = vec![format!("Camera {}: {}   ( {} )", index + 1, camera.name, key_name(index))];
    match status {
        Some(c) => {
            if let Some(fault) = &c.fault {
                lines.push(fault.clone());
                if c.held {
                    lines
                        .push("Held where it stopped. The other cameras carry on. Disarm and arm again to bring it back.".into());
                }
            }
            if let Some(progress) = &c.preset_sync {
                lines.push(progress.clone());
            }
            if c.hardware {
                for a in &c.adapters {
                    lines.push(format!("{}: {}", a.name, a.detail));
                }
            } else {
                lines.push("Simulator: no rig is driven for this camera.".into());
            }
        }
        None => lines.push("The server has not reported this camera yet.".into()),
    }
    lines.join("\n")
}

pub fn bar(ui: &mut Ui, s: &mut Session) {
    let selected = s.cam_index();
    let output = s.status.transport.output;
    let mut choose = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.label(RichText::new("Cameras").color(MUTED).small());
        egui::ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
            for (i, camera) in s.project.cameras.iter().enumerate() {
                let on = i == selected;
                let (word, ink) = state(s.status.camera(&camera.id), output);
                let color = camera_color(i);
                let text =
                    RichText::new(format!("{} {}", i + 1, camera.name)).size(12.5).color(if on { BASE } else { OFF_WHITE });
                let button = egui::Button::new(text)
                    .fill(if on { color } else { mix(PANEL, color, 0.16) })
                    .stroke(Stroke::new(if on { 1.5 } else { 1.0 }, if on { OFF_WHITE } else { mix(LINE, color, 0.5) }))
                    .min_size(vec2(0.0, 24.0));
                let r = ui.add(button).on_hover_text(detail(s, i));
                if r.clicked() {
                    choose = Some(camera.id.clone());
                }
                // The state sits under the chip's right corner, out of the name's way.
                let dot = r.rect.right_top() + vec2(-5.0, 5.0);
                ui.painter().circle_filled(dot, 3.5, ink);
                ui.painter().circle_stroke(dot, 3.5, Stroke::new(1.0, if on { BASE } else { PANEL }));
                if matches!(word, "held" | "fault" | "stopping" | "no answer" | "reading") {
                    ui.label(RichText::new(word).color(ink).small().strong());
                }
            }
            if s.status.transport.record {
                if let Some(i) = s.project.camera_index(&s.status.record.camera) {
                    ui.add_space(6.0);
                    ui.label(RichText::new(format!("Record writes camera {}", i + 1)).color(RED).small());
                }
            }
        });
    });
    if let Some(id) = choose {
        s.select_camera(&id);
        s.axis_lanes.clear();
    }
}

/// Number keys select a camera.
pub fn shortcuts(ctx: &egui::Context, s: &mut Session) {
    let pressed = ctx.input(|i| {
        i.events.iter().find_map(|e| match e {
            egui::Event::Key { key, pressed: true, modifiers, repeat: false, .. } if !modifiers.command && !modifiers.alt => {
                KEYS.iter().position(|k| k == key)
            }
            _ => None,
        })
    });
    if let Some(camera) = pressed.and_then(|i| s.project.cameras.get(i)) {
        let id = camera.id.clone();
        if s.cam_id() != id {
            s.select_camera(&id);
            s.axis_lanes.clear();
        }
    }
}
