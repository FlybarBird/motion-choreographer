//! Project settings shown in the Inspector when nothing is selected.

use crate::link::Cmd;
use crate::panels::section;
use crate::session::Session;
use crate::theme::*;
use egui::{RichText, Ui};
use ptz_core::pose::{Axis, CalPoint, ChannelRoute, Cmp, KeepOutRule, Project, RigKind};
use ptz_core::transport::{Output, Run};

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

/// A text box that hides what is typed and reports it once, when editing ends.
fn secret_field(ui: &mut Ui, salt: &str, current: &str, width: f32) -> Option<String> {
    let id = ui.make_persistent_id(salt);
    let mut buf = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| current.to_string());
    let resp = ui.add(egui::TextEdit::singleline(&mut buf).password(true).desired_width(width).id(id.with("edit")));
    if resp.lost_focus() {
        ui.data_mut(|d| d.remove::<String>(id));
        return (buf != current).then_some(buf);
    }
    if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, buf));
    } else {
        ui.data_mut(|d| d.remove::<String>(id));
    }
    None
}

fn axis_combo(ui: &mut Ui, salt: (&str, usize), axis: &mut Axis) {
    egui::ComboBox::from_id_salt(salt).selected_text(axis.label()).width(82.0).show_ui(ui, |ui| {
        for choice in Axis::ALL {
            ui.selectable_value(axis, choice, choice.label());
        }
    });
}

fn cmp_combo(ui: &mut Ui, salt: (&str, usize), cmp: &mut Cmp) {
    egui::ComboBox::from_id_salt(salt).selected_text(cmp.word()).width(62.0).show_ui(ui, |ui| {
        for choice in [Cmp::Below, Cmp::Above] {
            ui.selectable_value(cmp, choice, choice.word());
        }
    });
}

fn unit_suffix(axis: Axis) -> String {
    match axis.unit() {
        "" => String::new(),
        unit => format!(" {unit}"),
    }
}

/// Keep-out rules: poses the rig must never take, whatever the timeline says.
fn keep_out(ui: &mut Ui, project: &mut Project, editable: bool) {
    section(ui, "Keep-out rules");
    ui.label(
        RichText::new(
            "A pose that breaks a rule is an issue on the timeline that cannot be accepted, and holds the rig if it is ever commanded live.",
        )
        .color(MUTED)
        .small(),
    );
    let mut remove = None;
    ui.add_enabled_ui(editable, |ui| {
        for (i, rule) in project.keep_out.iter_mut().enumerate() {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if let Some(name) = crate::panels::text_field(ui, &format!("rule_name_{i}"), &rule.name, 150.0) {
                    rule.name = name;
                }
                if ui.small_button("Remove").on_hover_text("Delete this rule").clicked() {
                    remove = Some(i);
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label("When");
                axis_combo(ui, ("rule_when_axis", i), &mut rule.when_axis);
                ui.label("is");
                cmp_combo(ui, ("rule_when", i), &mut rule.when);
                let step = step_for(rule.when_axis);
                ui.add(egui::DragValue::new(&mut rule.when_value).speed(step).suffix(unit_suffix(rule.when_axis)));
            });
            ui.horizontal_wrapped(|ui| {
                axis_combo(ui, ("rule_then_axis", i), &mut rule.then_axis);
                ui.label("must be");
                cmp_combo(ui, ("rule_must", i), &mut rule.must_be);
                let step = step_for(rule.then_axis);
                ui.add(egui::DragValue::new(&mut rule.must_value).speed(step).suffix(unit_suffix(rule.then_axis)));
            });
            if rule.when_axis == rule.then_axis {
                ui.label(RichText::new("A rule ties one axis to another. Pick two different axes.").color(AMBER).small());
            }
        }
        ui.add_space(4.0);
        if ui
            .button("Add a keep-out rule")
            .on_hover_text("Starts as: when elevation is below 200 mm, tilt must be above -10 degrees")
            .clicked()
        {
            let mut rule = KeepOutRule::example();
            rule.name = format!("Rule {}", project.keep_out.len() + 1);
            project.keep_out.push(rule);
        }
    });
    if let Some(i) = remove {
        project.keep_out.remove(i);
    }
}

/// Which rig the server drives, how to reach it, and how closely it must follow.
fn rig(ui: &mut Ui, s: &mut Session, project: &mut Project, editable: bool) {
    section(ui, "Rig");
    let t = s.status.transport;
    let parked = t.run == Run::Stopped && t.output == Output::Rehearse && !t.estop;
    if !parked {
        ui.label(RichText::new("Stop and disarm to change which rig the server drives.").color(AMBER).small());
    }
    let rig = &mut project.rig;
    ui.add_enabled_ui(editable && parked, |ui| {
        egui::Grid::new("rig_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label("Drives");
            let shown = |kind: RigKind| match kind {
                RigKind::Simulator => "The simulator",
                RigKind::Hardware => "FR7 and AutoPod",
            };
            egui::ComboBox::from_id_salt("rig_kind").selected_text(shown(rig.kind)).width(170.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut rig.kind, RigKind::Simulator, shown(RigKind::Simulator));
                ui.selectable_value(&mut rig.kind, RigKind::Hardware, shown(RigKind::Hardware));
            });
            ui.end_row();
            if rig.kind == RigKind::Hardware {
                ui.label("FR7 address");
                if let Some(host) = crate::panels::text_field(ui, "fr7_host", &rig.fr7_host, 170.0) {
                    rig.fr7_host = host.trim().to_string();
                }
                ui.end_row();
                ui.label("FR7 web port");
                ui.add(egui::DragValue::new(&mut rig.fr7_http_port).range(1..=65535))
                    .on_hover_text("The camera's web interface, which gives preset names and pictures");
                ui.end_row();
                ui.label("FR7 login");
                ui.horizontal(|ui| {
                    if let Some(user) = crate::panels::text_field(ui, "fr7_user", &rig.fr7_user, 80.0) {
                        rig.fr7_user = user.trim().to_string();
                    }
                    if let Some(password) = secret_field(ui, "fr7_password", &rig.fr7_password, 80.0) {
                        rig.fr7_password = password;
                    }
                });
                ui.end_row();
                ui.label("AutoPod address");
                if let Some(host) = crate::panels::text_field(ui, "autopod_host", &rig.autopod_host, 170.0) {
                    rig.autopod_host = host.trim().to_string();
                }
                ui.end_row();
                ui.label("Counts per degree");
                ui.add(egui::DragValue::new(&mut rig.fr7_counts_per_degree).speed(0.1).range(1.0..=100000.0));
                ui.end_row();
                ui.label("Counts run backwards");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut rig.fr7_invert_pan, "Pan");
                    ui.checkbox(&mut rig.fr7_invert_tilt, "Tilt");
                });
                ui.end_row();
                ui.label("Plain moves");
                ui.checkbox(&mut rig.fr7_absolute_moves, "The head runs them itself").on_hover_text(
                    "A preset move is sent as one command and the head ramps by itself, instead of being steered along the curve. Use it if bring-up shows steering strays",
                );
                ui.end_row();
                ui.label("Speed steps");
                ui.horizontal(|ui| match (rig.fr7_speed_steps.first().copied(), rig.fr7_speed_steps.last().copied()) {
                    (Some(low), Some(top)) => {
                        ui.label(RichText::new(format!("measured: {low:.2} to {top:.1} °/s")).small());
                        if ui.small_button("Forget").on_hover_text("Go back to Sony's published range, spread evenly").clicked() {
                            rig.fr7_speed_steps.clear();
                        }
                    }
                    _ => {
                        ui.label(RichText::new("assumed until bring-up measures them").color(AMBER).small());
                    }
                });
                ui.end_row();
                ui.label("Query delay");
                ui.add(egui::DragValue::new(&mut rig.query_delay_ms).speed(0.1).range(0.0..=500.0).suffix(" ms"))
                    .on_hover_text("How old a position is when it arrives. Zero uses the round trip measured as it runs");
                ui.end_row();
            }
            ui.label("Arm moves at");
            let mut percent = rig.positioning_speed * 100.0;
            if ui.add(egui::Slider::new(&mut percent, 5.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
                rig.positioning_speed = percent / 100.0;
            }
            ui.end_row();
            ui.label("Tracking fault after");
            ui.add(egui::DragValue::new(&mut rig.tracking_window_ms).range(100..=5000).suffix(" ms"))
                .on_hover_text("How long the rig may be further from the timeline than the tolerance before it is held and disarmed");
            ui.end_row();
        });
        ui.label(RichText::new("Tolerance: how far each axis may be from the timeline while live").color(MUTED).small());
        ui.horizontal_wrapped(|ui| {
            for axis in Axis::ALL {
                let step = step_for(axis) / 5.0;
                ui.label(RichText::new(axis.label()).color(axis_color(axis)).small());
                let mut v = rig.tolerance.get(axis);
                if ui.add(egui::DragValue::new(&mut v).speed(step).range(step..=100000.0)).changed() {
                    rig.tolerance.set(axis, v);
                }
            }
        });

        if rig.kind == RigKind::Hardware && !rig.autopod_host.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("AutoPod height map: the leg box's raw position at measured heights").color(MUTED).small());
            let mut remove = None;
            egui::Grid::new("height_map").num_columns(3).spacing([10.0, 4.0]).show(ui, |ui| {
                for (i, point) in rig.autopod_calibration.iter_mut().enumerate() {
                    ui.add(egui::DragValue::new(&mut point.raw).speed(10.0).prefix("raw "));
                    ui.add(egui::DragValue::new(&mut point.mm).speed(1.0).suffix(" mm"));
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let Some(i) = remove {
                rig.autopod_calibration.remove(i);
            }
            let raw = s.status.adapters.iter().find(|a| a.name == "AutoPod").and_then(|a| a.raw_position);
            let id = ui.make_persistent_id("height_now");
            let mut mm = ui.data_mut(|d| d.get_temp::<f64>(id)).unwrap_or(0.0);
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut mm).speed(1.0).range(0.0..=5000.0).suffix(" mm"));
                let add = ui
                    .add_enabled(raw.is_some(), egui::Button::new("Add the current height").small())
                    .on_hover_text("Measure the column now, enter the height, and add it with the raw position the leg box reports")
                    .on_disabled_hover_text("The AutoPod is not answering, so its raw position is not known");
                if let (true, Some(raw)) = (add.clicked(), raw) {
                    rig.autopod_calibration.retain(|p| p.raw != raw);
                    rig.autopod_calibration.push(CalPoint { raw, mm });
                    rig.autopod_calibration.sort_by(|a, b| a.raw.total_cmp(&b.raw));
                }
                if let Some(raw) = raw {
                    ui.label(RichText::new(format!("raw {raw:.0}")).monospace().small().color(MUTED));
                }
            });
            ui.data_mut(|d| d.insert_temp(id, mm));
            if rig.autopod_calibration.len() < 3 {
                ui.label(
                    RichText::new(format!(
                        "{} of 3 heights. Arm is refused until the map has three; ptz-bringup measures them with you.",
                        rig.autopod_calibration.len()
                    ))
                    .color(AMBER)
                    .small(),
                );
            }
        }
    });
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

    keep_out(ui, &mut project, editable);
    rig(ui, s, &mut project, editable);

    if project != s.project {
        // Shown at once; the server's answer replaces it.
        s.project = project.clone();
        if let Some(song) = &s.song {
            s.compiled = ptz_core::compile::compile(song, &s.project);
        }
        s.link.send(Cmd::PutProject(Box::new(project)));
    }
}
