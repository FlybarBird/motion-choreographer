//! The camera bar: every camera in the show, which one is selected, and how each is doing.

use crate::session::Session;
use crate::theme::*;
use egui::{vec2, Key, RichText, Stroke, Ui};
use ptz_core::api::CameraStatus;
use ptz_core::pose::{Axis, Pose};
use ptz_core::transport::Output;

/// What the rig page and the camera bar say when a default position and zoom is stored.
pub fn default_line(pose: Pose) -> String {
    format!(
        "Default: {} pan, {} tilt, zoom {}",
        Axis::Pan.format(pose.pan),
        Axis::Tilt.format(pose.tilt),
        Axis::Zoom.format(pose.zoom)
    )
}

pub const NO_DEFAULT: &str = "No default stored. A new song starts this camera at home.";

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
        // Amber while the rig travels to the playhead, green once it follows the timeline: as the transport's rule is.
        Some(c) if c.hardware && output == Output::Positioning => ("live", AMBER),
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
    if let Some(pose) = camera.default_pose {
        lines.push(default_line(pose));
    }
    lines.join("\n")
}

/// A camera on the bar: its number on its colour, its name, and a mark of its state. The one in hand
/// has the accent rule under it, like the tab in front. The mark is a ring for the simulator and a
/// dot for a rig, so that the two are not told apart by colour alone.
fn camera_chip(ui: &mut Ui, index: usize, name: &str, on: bool, word: &str, ink: egui::Color32) -> egui::Response {
    let label = format!("{} {name}", index + 1);
    let font = if on { semibold(BODY) } else { body() };
    let galley = ui.painter().layout_no_wrap(name.to_string(), font, OFF_WHITE);
    let mark = 2.0 * U;
    let width = U + NUMBER_TAG.x + GAP + galley.size().x + GAP + mark + GAP;
    let (rect, response) = ui.allocate_exact_size(vec2(width, CONTROL_H), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, &label));
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let fill = if on {
            CONTROL
        } else if response.hovered() {
            CONTROL_HOVER
        } else {
            egui::Color32::TRANSPARENT
        };
        p.rect(rect, RADIUS, fill, Stroke::new(RULE, if on { CONTROL_LINE } else { LINE }), egui::StrokeKind::Inside);
        let tag = egui::Rect::from_center_size(egui::pos2(rect.left() + U + NUMBER_TAG.x / 2.0, rect.center().y), NUMBER_TAG);
        number_tag(p, tag, index + 1, camera_color(index));
        let ink_name = if on { egui::Color32::WHITE } else { OFF_WHITE };
        p.galley(egui::pos2(tag.right() + GAP, rect.center().y - galley.size().y / 2.0), galley, ink_name);
        let at = egui::pos2(rect.right() - GAP - mark / 2.0, rect.center().y);
        if word == "sim" {
            p.circle_stroke(at, mark / 2.0 - 1.0, Stroke::new(RULE + 0.5, ink));
        } else {
            p.circle_filled(at, mark / 2.0, ink);
        }
        if on {
            p.hline(rect.x_range(), rect.bottom() - RULE, Stroke::new(RULE_STRONG, ACCENT));
        }
    }
    response
}

pub fn bar(ui: &mut Ui, s: &mut Session) {
    let selected = s.cam_index();
    let output = s.status.transport.output;
    let mut choose = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = U;
        ui.label(caps("Cameras").color(MUTED));
        ui.add_space(U);
        // Show settings keeps the right end; the cameras scroll in what is left.
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let open = s.settings.is_some();
            // It says what a click does, and its words are not those of the heading it opens.
            let (glyph, words, look, tip) = if open {
                (icon::X, "Close settings", Look::Solid(ACCENT), "Back to the timeline and monitors")
            } else {
                (
                    icon::GEAR_SIX,
                    crate::settings_ui::TITLE,
                    Look::Quiet,
                    "Cameras, rigs, lenses, the stage, audio output and timecode inputs",
                )
            };
            if icon_button(ui, true, glyph, words, look, vec2(0.0, CONTROL_H)).on_hover_text(tip).clicked() {
                s.settings = if open { None } else { Some(crate::settings_ui::Page::default()) };
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                egui::ScrollArea::horizontal().auto_shrink([false, true]).show(ui, |ui| {
                    for (i, camera) in s.project.cameras.iter().enumerate() {
                        let on = i == selected;
                        let (word, ink) = state(s.status.camera(&camera.id), output);
                        let r = camera_chip(ui, i, &camera.name, on, word, ink).on_hover_text(detail(s, i));
                        if r.clicked() {
                            choose = Some(camera.id.clone());
                        }
                        // A state that needs the operator is said in a word beside the camera.
                        if matches!(word, "held" | "fault" | "stopping" | "no answer" | "reading") {
                            ui.label(RichText::new(word).color(ink).font(semibold(SMALL)));
                            ui.add_space(U);
                        }
                    }
                    if s.status.transport.record {
                        ui.add_space(GAP);
                        ui.label(RichText::new("Record writes each camera you move").color(RED).small());
                    }
                });
            });
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
