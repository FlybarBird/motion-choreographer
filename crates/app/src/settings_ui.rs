//! Project settings shown in the Inspector when nothing is selected.

use crate::link::Cmd;
use crate::panels::section;
use crate::session::Session;
use crate::theme::*;
use egui::{RichText, Ui};
use ptz_core::pose::{Axis, ChannelRoute};

fn route_label(route: ChannelRoute) -> &'static str {
    match route {
        ChannelRoute::Program => "Program (the song's audio)",
        ChannelRoute::Ltc => "LTC timecode",
        ChannelRoute::Silent => "Silent",
    }
}

fn route_combo(ui: &mut Ui, label: &str, route: &mut ChannelRoute) {
    ui.label(label);
    egui::ComboBox::from_id_salt(("route", label)).selected_text(route_label(*route)).width(210.0).show_ui(ui, |ui| {
        for choice in [ChannelRoute::Program, ChannelRoute::Ltc, ChannelRoute::Silent] {
            ui.selectable_value(route, choice, route_label(choice));
        }
    });
    ui.end_row();
}

/// A device picker: the automatic choice first, then what the server found. A name that is
/// set but not present stays in the list, marked, so it is not silently replaced.
fn device_combo(ui: &mut Ui, salt: &str, value: &mut String, automatic: &str, found: &[String]) {
    let shown = if value.is_empty() { automatic.to_string() } else { value.clone() };
    egui::ComboBox::from_id_salt(salt).selected_text(shown).width(190.0).show_ui(ui, |ui| {
        ui.selectable_value(value, String::new(), automatic);
        for name in found {
            ui.selectable_value(value, name.clone(), name);
        }
        if !value.is_empty() && !found.contains(value) {
            let missing = format!("{value} (not connected)");
            ui.selectable_value(value, value.clone(), missing);
        }
    });
}

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

    section(ui, "Audio output");
    let audio = &s.status.audio;
    let device = match (&audio.output, &audio.silent_reason) {
        (Some(name), _) => format!("{name}, {} Hz", audio.device_rate),
        (None, Some(why)) => why.clone(),
        (None, None) => "No output".to_string(),
    };
    ui.label(RichText::new(device).color(MUTED).small());
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("routing_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            route_combo(ui, "Left", &mut project.routing.left);
            route_combo(ui, "Right", &mut project.routing.right);
            if project.routing.left == ChannelRoute::Ltc || project.routing.right == ChannelRoute::Ltc {
                ui.label("LTC level");
                ui.add(egui::Slider::new(&mut project.ltc_level, 0.05..=1.0).fixed_decimals(2));
                ui.end_row();
            }
        });
    });
    ui.label(
        RichText::new("Program is the mono mix of every unmuted clip. LTC carries the song's timecode while it plays.")
            .color(MUTED)
            .small(),
    );

    section(ui, "Timecode inputs");
    ui.label(RichText::new("Where Lock listens. These are devices on the server machine.").color(MUTED).small());
    if s.inputs.is_none() {
        s.inputs = Some(Default::default());
        s.link.send(Cmd::RefreshInputs);
    }
    let choices = s.inputs.clone().unwrap_or_default();
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("inputs_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("LTC input");
            ui.horizontal(|ui| {
                device_combo(ui, "ltc_device", &mut project.inputs.ltc_device, "Default input", &choices.audio_inputs);
                let mut channel = project.inputs.ltc_channel + 1;
                if ui.add(egui::DragValue::new(&mut channel).range(1..=64).prefix("channel ")).changed() {
                    project.inputs.ltc_channel = channel - 1;
                }
            });
            ui.end_row();
            ui.label("MTC input");
            device_combo(ui, "mtc_port", &mut project.inputs.mtc_port, "First MIDI input", &choices.midi_inputs);
            ui.end_row();
            ui.label("Art-Net");
            if let Some(bind) = crate::panels::text_field(ui, "artnet_bind", &project.inputs.artnet_bind, 150.0) {
                match bind.trim().parse::<std::net::SocketAddr>() {
                    Ok(_) => project.inputs.artnet_bind = bind.trim().to_string(),
                    Err(_) => {
                        s.toast(format!("{bind} is not an address and port. Use 0.0.0.0:6454 to listen on every network"), true)
                    }
                }
            }
            ui.end_row();
            ui.label("Freewheel");
            ui.add(egui::DragValue::new(&mut project.freewheel_frames).range(0..=250).suffix(" frames"))
                .on_hover_text("How long the playhead carries on when code drops out, before it stops and holds");
            ui.end_row();
        });
    });
    for problem in [&choices.audio_problem, &choices.midi_problem].into_iter().flatten() {
        ui.label(RichText::new(problem).color(AMBER).small());
    }
    if ui.small_button("List devices again").clicked() {
        s.link.send(Cmd::RefreshInputs);
    }

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
