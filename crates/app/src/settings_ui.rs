//! The Show settings area: everything that belongs to the whole show and not to one song.

use crate::link::Cmd;
use crate::session::Session;
use crate::theme::*;
use crate::units::{distance_box, height_box};
use egui::{RichText, Ui};
use ptz_core::camera::{CameraConfig, OnFault, MAX_CAMERAS};
use ptz_core::lens::{self, Lens};
use ptz_core::pose::{Axis, CalPoint, ChannelRoute, Cmp, KeepOutRule, RigKind, Units};
use ptz_core::transport::{Output, Run};

fn route_label(route: ChannelRoute) -> &'static str {
    match route {
        ChannelRoute::Program => "Program (the song's audio)",
        ChannelRoute::Ltc => "LTC timecode",
        ChannelRoute::Silent => "Silent",
    }
}

fn route_combo(ui: &mut Ui, label: &str, route: &mut ChannelRoute) {
    field(ui, label);
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

/// A box for a value of one axis: elevation in the show's units, the others as they are.
fn axis_box(value: &mut f64, axis: Axis, units: Units) -> egui::DragValue<'_> {
    match axis {
        Axis::Elevation => height_box(value, units),
        _ => egui::DragValue::new(value).speed(step_for(axis)).suffix(unit_suffix(axis)),
    }
}

/// Keep-out rules: poses this camera's rig must never take, whatever the timeline says.
fn keep_out(ui: &mut Ui, camera: &mut CameraConfig, units: Units, editable: bool) {
    ui.label(
        RichText::new(
            "A pose that breaks a rule is an issue on the timeline that cannot be accepted, and holds the rig if it is ever commanded live.",
        )
        .color(MUTED)
        .small(),
    );
    let mut remove = None;
    ui.add_enabled_ui(editable, |ui| {
        for (i, rule) in camera.keep_out.iter_mut().enumerate() {
            ui.add_space(U);
            ui.horizontal(|ui| {
                if let Some(name) = crate::panels::text_field(ui, &format!("rule_name_{i}"), &rule.name, 150.0) {
                    rule.name = name;
                }
                if ui.button("Remove").on_hover_text("Delete this rule").clicked() {
                    remove = Some(i);
                }
            });
            ui.horizontal_wrapped(|ui| {
                field(ui, "When");
                axis_combo(ui, ("rule_when_axis", i), &mut rule.when_axis);
                field(ui, "is");
                cmp_combo(ui, ("rule_when", i), &mut rule.when);
                ui.add(axis_box(&mut rule.when_value, rule.when_axis, units));
            });
            ui.horizontal_wrapped(|ui| {
                axis_combo(ui, ("rule_then_axis", i), &mut rule.then_axis);
                field(ui, "must be");
                cmp_combo(ui, ("rule_must", i), &mut rule.must_be);
                ui.add(axis_box(&mut rule.must_value, rule.then_axis, units));
            });
            if rule.when_axis == rule.then_axis {
                ui.label(RichText::new("A rule ties one axis to another. Pick two different axes.").color(AMBER).small());
            }
        }
        ui.add_space(U);
        if ui
            .button("Add a keep-out rule")
            .on_hover_text("Starts as: when elevation is below 200 mm, tilt must be above -10 degrees")
            .clicked()
        {
            let mut rule = KeepOutRule::example();
            rule.name = format!("Rule {}", camera.keep_out.len() + 1);
            camera.keep_out.push(rule);
        }
    });
    if let Some(i) = remove {
        camera.keep_out.remove(i);
    }
}

/// Which rig the server drives for this camera, how to reach it, and how closely it must follow.
fn rig(ui: &mut Ui, s: &mut Session, camera: &mut CameraConfig, units: Units, editable: bool) {
    let t = s.status.transport;
    let parked = t.run == Run::Stopped && t.output == Output::Rehearse && !t.estop;
    if !parked {
        ui.label(RichText::new("Stop and disarm to change which rig the server drives.").color(AMBER).small());
    }
    let raw = s.cam_status().adapters.iter().find(|a| a.name == "AutoPod+").and_then(|a| a.raw_position);
    let rig = &mut camera.rig;
    ui.add_enabled_ui(editable && parked, |ui| {
        egui::Grid::new("rig_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
            field(ui, "Drives");
            let shown = |kind: RigKind| match kind {
                RigKind::Simulator => "The simulator",
                RigKind::Hardware => "FR7 and AutoPod+",
            };
            egui::ComboBox::from_id_salt("rig_kind").selected_text(shown(rig.kind)).width(170.0).show_ui(ui, |ui| {
                ui.selectable_value(&mut rig.kind, RigKind::Simulator, shown(RigKind::Simulator));
                ui.selectable_value(&mut rig.kind, RigKind::Hardware, shown(RigKind::Hardware));
            });
            ui.end_row();
            if rig.kind == RigKind::Hardware {
                field(ui, "FR7 address");
                if let Some(host) = crate::panels::text_field(ui, "fr7_host", &rig.fr7_host, 170.0) {
                    rig.fr7_host = host.trim().to_string();
                }
                ui.end_row();
                field(ui, "FR7 web port");
                ui.add(egui::DragValue::new(&mut rig.fr7_http_port).range(1..=65535))
                    .on_hover_text("The camera's web interface, which gives preset names and pictures");
                ui.end_row();
                field(ui, "FR7 login");
                ui.horizontal(|ui| {
                    if let Some(user) = crate::panels::text_field(ui, "fr7_user", &rig.fr7_user, 80.0) {
                        rig.fr7_user = user.trim().to_string();
                    }
                    if let Some(password) = secret_field(ui, "fr7_password", &rig.fr7_password, 80.0) {
                        rig.fr7_password = password;
                    }
                });
                ui.end_row();
                field(ui, "AutoPod+ address");
                if let Some(host) = crate::panels::text_field(ui, "autopod_host", &rig.autopod_host, 170.0) {
                    rig.autopod_host = host.trim().to_string();
                }
                ui.end_row();
                field(ui, "Counts per degree");
                ui.add(egui::DragValue::new(&mut rig.fr7_counts_per_degree).speed(0.1).range(1.0..=100000.0));
                ui.end_row();
                field(ui, "Counts run backwards");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut rig.fr7_invert_pan, "Pan");
                    ui.checkbox(&mut rig.fr7_invert_tilt, "Tilt");
                });
                ui.end_row();
                field(ui, "Plain moves");
                ui.checkbox(&mut rig.fr7_absolute_moves, "The head runs them itself").on_hover_text(
                    "A preset move is sent as one command and the head ramps by itself, instead of being steered along the curve. Use it if bring-up shows steering strays",
                );
                ui.end_row();
                field(ui, "Speed steps");
                ui.horizontal(|ui| match (rig.fr7_speed_steps.first().copied(), rig.fr7_speed_steps.last().copied()) {
                    (Some(low), Some(top)) => {
                        ui.label(RichText::new(format!("measured: {low:.2} to {top:.1} °/s")).small());
                        if ui.button("Forget").on_hover_text("Go back to Sony's published range, spread evenly").clicked() {
                            rig.fr7_speed_steps.clear();
                        }
                    }
                    _ => {
                        ui.label(RichText::new("assumed until bring-up measures them").color(AMBER).small());
                    }
                });
                ui.end_row();
                field(ui, "Query delay");
                ui.add(egui::DragValue::new(&mut rig.query_delay_ms).speed(0.1).range(0.0..=500.0).suffix(" ms"))
                    .on_hover_text("How old a position is when it arrives. Zero uses the round trip measured as it runs");
                ui.end_row();
            }
            field(ui, "Arm moves at");
            let mut percent = rig.positioning_speed * 100.0;
            if ui.add(egui::Slider::new(&mut percent, 5.0..=100.0).suffix("%").fixed_decimals(0)).changed() {
                rig.positioning_speed = percent / 100.0;
            }
            ui.end_row();
            field(ui, "Tracking fault after");
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
                let field = match axis {
                    Axis::Elevation => height_box(&mut v, units),
                    _ => egui::DragValue::new(&mut v).speed(step),
                };
                if ui.add(field.range(step..=100000.0)).changed() {
                    rig.tolerance.set(axis, v);
                }
            }
        });

        if rig.kind == RigKind::Hardware && !rig.autopod_host.is_empty() {
            ui.add_space(U);
            ui.label(RichText::new("AutoPod+ height map: the leg box's raw position at measured heights").color(MUTED).small());
            ui.label(
                RichText::new(
                    "Measure how far the column is raised above fully down, and type it as the height. Fully down is 0. Double-click a box to type in it.",
                )
                .color(MUTED)
                .small(),
            );
            let not_answering = "The AutoPod+ is not answering, so its raw position is not known";
            let (mut remove, mut retake) = (None, None);
            egui::Grid::new("height_map").num_columns(4).spacing([PAD, U]).show(ui, |ui| {
                if !rig.autopod_calibration.is_empty() {
                    for head in ["Raw position", "Measured height", "", ""] {
                        ui.label(RichText::new(head).small().color(MUTED));
                    }
                    ui.end_row();
                }
                for (i, point) in rig.autopod_calibration.iter_mut().enumerate() {
                    // Each change here sets the rig up again, so a typed number is taken once, when it is finished.
                    ui.add(egui::DragValue::new(&mut point.raw).speed(10.0).update_while_editing(false))
                        .on_hover_text("What the leg box reported at this height");
                    ui.add(height_box(&mut point.mm, units).update_while_editing(false))
                        .on_hover_text("Set height: what you measured with the column at this raw position");
                    let update = ui
                        .add_enabled(raw.is_some(), egui::Button::new("Update to current"))
                        .on_hover_text("The column is at this height now: take the raw position it reports for this row")
                        .on_disabled_hover_text(not_answering);
                    if update.clicked() {
                        retake = Some(i);
                    }
                    if ui.button("Remove").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if let (Some(i), Some(raw)) = (retake, raw) {
                // One height for each raw position: a row that already had this one gives way.
                let mm = rig.autopod_calibration[i].mm;
                rig.autopod_calibration.remove(i);
                rig.autopod_calibration.retain(|p| p.raw != raw);
                rig.autopod_calibration.push(CalPoint { raw, mm });
                rig.autopod_calibration.sort_by(|a, b| a.raw.total_cmp(&b.raw));
            }
            if let Some(i) = remove {
                rig.autopod_calibration.remove(i);
            }
            let id = ui.make_persistent_id("height_now");
            let mut mm = ui.data_mut(|d| d.get_temp::<f64>(id)).unwrap_or(0.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("The column is now at").small());
                ui.add(height_box(&mut mm, units).range(0.0..=5000.0))
                    .on_hover_text("Set height: what you measure with the column where it is now");
                let add = ui
                    .add_enabled(raw.is_some(), egui::Button::new("Add the current height"))
                    .on_hover_text("Measure the column now, enter the height, and add it with the raw position the leg box reports")
                    .on_disabled_hover_text(not_answering);
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

/// A page of the Show settings area.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum Page {
    #[default]
    Cameras,
    General,
    Stage,
    Programming,
    Rig,
    Lens,
    Position,
    Limits,
    KeepOut,
    Audio,
    Timecode,
    Panel,
}

impl Page {
    /// What the show has and how it runs.
    pub const SHOW: [Page; 4] = [Page::Cameras, Page::General, Page::Stage, Page::Programming];
    /// What belongs to the camera selected on the camera bar.
    pub const CAMERA: [Page; 5] = [Page::Rig, Page::Lens, Page::Position, Page::Limits, Page::KeepOut];
    /// Devices on the server machine.
    pub const SERVER: [Page; 3] = [Page::Audio, Page::Timecode, Page::Panel];

    pub fn title(self) -> &'static str {
        match self {
            Page::Cameras => "Cameras",
            Page::General => "Frame rate, units and faults",
            Page::Stage => "Stage model",
            Page::Programming => "Programming",
            Page::Rig => "Rig connection",
            Page::Lens => "Lens",
            Page::Position => "Position on the stage",
            Page::Limits => "Rig limits",
            Page::KeepOut => "Keep-out rules",
            Page::Audio => "Audio output",
            Page::Timecode => "Timecode inputs",
            Page::Panel => "Control panel",
        }
    }

    pub fn of_one_camera(self) -> bool {
        Page::CAMERA.contains(&self)
    }
}

pub const TITLE: &str = "Show settings";

/// The Show settings area. It takes the place of the timeline and monitors while it is open; the
/// header, the transport with E-STOP and the camera bar stay where they are, and the button at the
/// end of the camera bar, or Esc, closes it.
pub fn area(ui: &mut Ui, s: &mut Session, page: &mut Page) {
    use egui::{Frame, Margin};
    egui::Panel::left("settings_nav")
        .resizable(false)
        .exact_size(60.0 * U)
        .frame(Frame::new().fill(PANEL_DARK).inner_margin(Margin::symmetric(0, PAD as i8)))
        .show(ui, |ui| nav(ui, s, page));
    egui::CentralPanel::default()
        .frame(Frame::new().fill(PANEL).inner_margin(Margin::symmetric((6.0 * U) as i8, (4.0 * U) as i8)))
        .show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt(("settings_page", *page)).auto_shrink([false, false]).show(ui, |ui| {
                ui.set_max_width(720.0);
                body(ui, s, *page);
                ui.add_space(6.0 * U);
            });
        });
}

/// Why nothing here can be changed just now, from why the song cannot be edited.
pub fn read_only(why: &str) -> String {
    let why = why.replace("to edit the song", "to change settings").replace("to edit", "to change settings");
    format!("{why}. Until then the settings can only be read.")
}

/// The selected camera, as the camera bar shows it.
fn camera_tag(ui: &mut Ui, s: &Session) {
    let at = s.cam_index();
    let name = s.project.cameras.get(at).map(|c| c.name.clone()).unwrap_or_default();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        let (rect, _) = ui.allocate_exact_size(NUMBER_TAG, egui::Sense::hover());
        number_tag(ui.painter(), rect, at + 1, camera_color(at));
        ui.add(egui::Label::new(RichText::new(name).font(semibold(BODY))).truncate());
    });
}

/// A page in the list: a row the width of the list. The page that is open has the accent down its
/// leading edge and a lighter ground, as the camera in hand has on the timeline.
fn nav_item(ui: &mut Ui, open: bool, title: &str, height: f32) -> egui::Response {
    // The row is ruled from the edge of the list; what is pressed begins at the margin, with its name.
    let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), egui::Sense::hover());
    let rect = row.with_min_x(row.left() + PAD);
    let response = ui.interact(rect, ui.id().with(("settings_page", title)), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, open, title));
    if ui.is_rect_visible(row) {
        let p = ui.painter();
        if open {
            p.rect_filled(row, 0.0, CONTROL);
            p.rect_filled(row.with_max_x(row.left() + 3.0), 0.0, ACCENT);
        } else if response.hovered() {
            p.rect_filled(row, 0.0, mix(PANEL_DARK, CONTROL, 0.6));
        }
        let ink = if open { egui::Color32::WHITE } else { OFF_WHITE };
        let font = if open { semibold(BODY) } else { crate::theme::body() };
        p.text(egui::pos2(rect.left(), rect.center().y), egui::Align2::LEFT_CENTER, title, font, ink);
    }
    response
}

fn nav(ui: &mut Ui, s: &Session, page: &mut Page) {
    // In a short window the list is set closer, so that every page is on it without scrolling.
    let tight = ui.available_height() < 600.0;
    // Everything in the list starts at one margin; a group is headed by a rule and its name in capitals.
    let inset = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui)| {
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            add(ui);
        });
    };
    let group = |ui: &mut Ui, title: &str| {
        ui.add_space(if tight { 0.0 } else { U });
        hairline(ui);
        ui.add_space(if tight { 0.0 } else { U });
        inset(ui, &mut |ui| {
            ui.label(caps(title).color(MUTED));
        });
    };
    let items = |ui: &mut Ui, page: &mut Page, pages: &[Page]| {
        for p in pages {
            if nav_item(ui, *page == *p, p.title(), if tight { CONTROL_SMALL + 2.0 } else { CONTROL_H + U }).clicked() {
                *page = *p;
            }
        }
    };
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = if tight { 2.0 } else { U };
        inset(ui, &mut |ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            ui.label(RichText::new(icon::GEAR_SIX).font(icon_font(crate::theme::TITLE)));
            ui.label(RichText::new(TITLE).font(semibold(crate::theme::TITLE)));
        });
        group(ui, "The show");
        items(ui, page, &Page::SHOW);
        group(ui, "The selected camera");
        inset(ui, &mut |ui| camera_tag(ui, s));
        items(ui, page, &Page::CAMERA);
        group(ui, "The server");
        items(ui, page, &Page::SERVER);
    });
}

fn body(ui: &mut Ui, s: &mut Session, page: Page) {
    let editable = s.can_edit().is_ok();
    let mut project = s.project.clone();
    if let Err(why) = s.can_edit() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;
            ui.label(RichText::new(icon::LOCK_SIMPLE).font(icon_font(crate::theme::TITLE)).color(AMBER));
            ui.label(RichText::new(read_only(&why)).color(AMBER));
        });
        ui.add_space(GAP);
    }
    ui.label(RichText::new(page.title()).font(semibold(DISPLAY)));
    if page.of_one_camera() {
        camera_tag(ui, s);
        ui.label(
            RichText::new("These are this camera's. Choose another on the camera bar, or with its number key.")
                .color(MUTED)
                .small(),
        );
    }
    hairline(ui);
    ui.add_space(U);
    match page {
        Page::Cameras => cameras(ui, s, &mut project, editable),
        Page::General => general(ui, s, &mut project, editable),
        Page::Stage => stage_model(ui, s, &mut project, editable),
        Page::Programming => programming(ui, &mut project, editable),
        Page::Audio => audio_output(ui, s, &mut project, editable),
        Page::Timecode => timecode_inputs(ui, s, &mut project, editable),
        Page::Panel => control_panel(ui, s, &mut project, editable),
        Page::Rig | Page::Lens | Page::Position | Page::Limits | Page::KeepOut => {
            let at = project.camera_index(&s.active_camera).unwrap_or(0);
            let units = project.units;
            let camera = &mut project.cameras[at];
            match page {
                Page::Rig => rig(ui, s, camera, units, editable),
                Page::Lens => lens(ui, s, camera, units, editable),
                Page::Position => position(ui, s, camera, units, editable),
                Page::Limits => limits(ui, camera, units, editable),
                _ => keep_out(ui, camera, units, editable),
            }
        }
    }
    if project != s.project {
        s.put_project(project);
    }
}

/// The frame rate the show is programmed at, the units it is read in, and what the other cameras do when one faults.
fn general(ui: &mut Ui, s: &mut Session, project: &mut ptz_core::pose::Project, editable: bool) {
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("general_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
            field(ui, "Frame rate");
            egui::ComboBox::from_id_salt("rate").selected_text(s.project.rate.label()).show_ui(ui, |ui| {
                for rate in ptz_core::FrameRate::ALL {
                    if ui.selectable_label(s.project.rate == rate, rate.label()).clicked() && rate != s.project.rate {
                        // Sent as asked and shown once the server has taken it: it can refuse a change of rate.
                        let mut p = s.project.clone();
                        p.rate = rate;
                        s.link.send(Cmd::PutProject(Box::new(p)));
                    }
                }
            });
            ui.end_row();
            field(ui, "Measurement units");
            egui::ComboBox::from_id_salt("units")
                .selected_text(project.units.label())
                .width(260.0)
                .show_ui(ui, |ui| {
                    for choice in Units::ALL {
                        ui.selectable_value(&mut project.units, choice, choice.label());
                    }
                })
                .response
                .on_hover_text("How heights and distances are shown and typed at every station. The show itself does not change");
            ui.end_row();
            field(ui, "When a camera faults");
            egui::ComboBox::from_id_salt("on_fault").selected_text(project.on_fault.label()).width(260.0).show_ui(ui, |ui| {
                for choice in OnFault::ALL {
                    ui.selectable_value(&mut project.on_fault, choice, choice.label());
                }
            });
            ui.end_row();
        });
    });
    ui.add_space(U);
    ui.label(
        RichText::new(
            "Set the frame rate once, before programming. It must match the timecode the show will be locked to; code at another rate is refused.",
        )
        .color(MUTED)
        .small(),
    );
    ui.label(RichText::new("E-Stop always stops every camera, whatever is chosen here.").color(MUTED).small());
}

/// How near a captured pose must be to a preset the camera has for that preset to be used again.
fn programming(ui: &mut Ui, project: &mut ptz_core::pose::Project, editable: bool) {
    ui.label(
        RichText::new(
            "In Program mode on the control panel, a camera's number stores where its rig stands. \
             When every axis is within these ranges of a preset that camera already has, that preset is used and no new one is stored.",
        )
        .color(MUTED)
        .small(),
    );
    ui.add_space(GAP);
    let units = project.units;
    let matching = &mut project.preset_match;
    ui.add_enabled_ui(editable, |ui| {
        ui.checkbox(&mut matching.enabled, "Use a preset within range");
        ui.add_space(U);
        ui.add_enabled_ui(matching.enabled, |ui| {
            egui::Grid::new("preset_match_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
                ui.label(RichText::new("").small());
                ui.label(RichText::new("Within").small().color(MUTED));
                ui.end_row();
                for axis in Axis::ALL {
                    let mut value = matching.range.get(axis);
                    ui.label(RichText::new(axis.label()).color(axis_color(axis)));
                    match axis {
                        Axis::Pan | Axis::Tilt => {
                            ui.add(egui::DragValue::new(&mut value).speed(0.1).range(0.0..=90.0).max_decimals(2).suffix(" deg"));
                        }
                        // Zoom and focus are kept as a share of their travel and typed as a percentage of it.
                        Axis::Zoom | Axis::Focus => {
                            let mut percent = value * 100.0;
                            ui.add(egui::DragValue::new(&mut percent).speed(0.1).range(0.0..=100.0).max_decimals(1).suffix(" %"));
                            value = percent / 100.0;
                        }
                        Axis::Elevation => {
                            ui.add(height_box(&mut value, units).range(0.0..=100000.0));
                        }
                    }
                    if (value - matching.range.get(axis)).abs() > 1e-9 {
                        matching.range.set(axis, value.max(0.0));
                    }
                    ui.end_row();
                }
            });
        });
    });
    ui.add_space(U);
    ui.label(
        RichText::new(
            "Wider ranges give fewer presets that are nearly the same; narrower ones keep small differences apart. \
             The nearest preset in range is the one used. Presets stored in the app are not looked up this way.",
        )
        .color(MUTED)
        .small(),
    );
}

/// Where the server plays the song's audio and timecode.
fn audio_output(ui: &mut Ui, s: &mut Session, project: &mut ptz_core::pose::Project, editable: bool) {
    let audio = &s.status.audio;
    let device = match (&audio.output, &audio.silent_reason) {
        (Some(name), _) => format!("{name}, {} Hz", audio.device_rate),
        (None, Some(why)) => why.clone(),
        (None, None) => "No output".to_string(),
    };
    ui.label(RichText::new(device).color(MUTED).small());
    // What is set here is not all of it while a file gives up a channel to timecode.
    let mirrored = s.status.audio.routing != s.project.routing;
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("routing_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
            route_combo(ui, "Left", &mut project.routing.left);
            route_combo(ui, "Right", &mut project.routing.right);
            if project.routing.left == ChannelRoute::Ltc || project.routing.right == ChannelRoute::Ltc {
                field(ui, "LTC level");
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
    if mirrored {
        ui.label(
            RichText::new(
                "This song has a file whose LTC channel is read, so Program plays on both outputs while it is open: \
                 the output set to Silent carries it too.",
            )
            .color(MUTED)
            .small(),
        );
    }
}

/// What the server says about the control panel, and whether that is good news.
pub fn panel_state(status: &ptz_core::api::Status, wanted: Option<&str>) -> (String, bool) {
    let (host, panel) = (status.panel_host.as_ref(), status.panel.as_ref());
    if let Some(why) = host.and_then(|h| h.problem.as_ref()) {
        return (format!("The server could not start the panel: {why}. It tries again every few seconds."), false);
    }
    match (panel, host) {
        (Some(p), _) if p.connected => {
            let name = if p.name.is_empty() { p.model.clone() } else { format!("{} ({})", p.name, p.model) };
            let more = if p.message.is_empty() { String::new() } else { format!(" {}", p.message) };
            (format!("Connected to {name}. It is the station \"{}\".{more}", p.station), true)
        }
        (_, Some(h)) => (
            format!("Looking for the panel at {}. Check that it is on, on this network, and in Raw Panel mode.", h.address),
            false,
        ),
        (Some(p), None) => (format!("The station \"{}\" runs a panel, and is looking for it.", p.station), false),
        (None, None) if wanted.is_some() => ("Starting.".to_string(), false),
        (None, None) => ("No panel is in use.".to_string(), true),
    }
}

/// The SKAARHOJ panel the server runs as one more station of the show.
fn control_panel(ui: &mut Ui, s: &mut Session, project: &mut ptz_core::pose::Project, editable: bool) {
    ui.label(
        RichText::new(
            "A SKAARHOJ PTZ Pro in Raw Panel mode. The server connects to it and it runs the show as one more station.",
        )
        .color(MUTED)
        .small(),
    );
    let from_command_line = s.status.panel_host.as_ref().is_some_and(|h| h.from_command_line);
    ui.add_enabled_ui(editable && !from_command_line, |ui| {
        egui::Grid::new("panel_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
            field(ui, "Panel address");
            if let Some(address) = crate::panels::text_field(ui, "panel_address", &project.panel.address, 190.0) {
                let was = std::mem::replace(&mut project.panel.address, address.trim().to_string());
                if project.panel.address.is_empty() {
                    project.panel.enabled = false;
                } else if let Err(why) = project.panel.validate() {
                    project.panel.address = was;
                    s.toast(why, true);
                }
            }
            ui.end_row();
            ui.label("");
            ui.add_enabled(!project.panel.address.is_empty(), egui::Checkbox::new(&mut project.panel.enabled, "Use this panel"))
                .on_disabled_hover_text("Give the panel's address first");
            ui.end_row();
        });
    });
    ui.add_space(U);
    if from_command_line {
        ui.label(
            RichText::new(
                "This server was started with a panel of its own (--panel). It is changed where the server is started.",
            )
            .color(AMBER)
            .small(),
        );
    }
    let (state, good) = panel_state(&s.status, s.project.panel.wanted());
    let state = RichText::new(state);
    ui.label(if good { state } else { state.color(AMBER) });
    ui.add_space(U);
    ui.label(
        RichText::new(
            "The address is the panel's own, like 192.168.0.60; port 9923 is used unless another is given. \
             A layout other than the PTZ Pro's is read from panel.json in the server's data folder.",
        )
        .color(MUTED)
        .small(),
    );
}

/// Where Lock listens for timecode.
fn timecode_inputs(ui: &mut Ui, s: &mut Session, project: &mut ptz_core::pose::Project, editable: bool) {
    ui.label(RichText::new("Where Lock listens. These are devices on the server machine.").color(MUTED).small());
    if s.inputs.is_none() {
        s.inputs = Some(Default::default());
        s.link.send(Cmd::RefreshInputs);
    }
    let choices = s.inputs.clone().unwrap_or_default();
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("inputs_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
            field(ui, "LTC input");
            ui.horizontal(|ui| {
                device_combo(ui, "ltc_device", &mut project.inputs.ltc_device, "Default input", &choices.audio_inputs);
                let mut channel = project.inputs.ltc_channel + 1;
                if ui.add(egui::DragValue::new(&mut channel).range(1..=64).prefix("channel ")).changed() {
                    project.inputs.ltc_channel = channel - 1;
                }
            });
            ui.end_row();
            field(ui, "MTC input");
            device_combo(ui, "mtc_port", &mut project.inputs.mtc_port, "First MIDI input", &choices.midi_inputs);
            ui.end_row();
            field(ui, "Art-Net");
            if let Some(bind) = crate::panels::text_field(ui, "artnet_bind", &project.inputs.artnet_bind, 150.0) {
                match bind.trim().parse::<std::net::SocketAddr>() {
                    Ok(_) => project.inputs.artnet_bind = bind.trim().to_string(),
                    Err(_) => {
                        s.toast(format!("{bind} is not an address and port. Use 0.0.0.0:6454 to listen on every network"), true)
                    }
                }
            }
            ui.end_row();
            field(ui, "Freewheel");
            ui.add(egui::DragValue::new(&mut project.freewheel_frames).range(0..=250).suffix(" frames"))
                .on_hover_text("How long the playhead carries on when code drops out, before it stops and holds");
            ui.end_row();
        });
    });
    for problem in [&choices.audio_problem, &choices.midi_problem].into_iter().flatten() {
        ui.label(RichText::new(problem).color(AMBER).small());
    }
    if ui.button("List devices again").clicked() {
        s.link.send(Cmd::RefreshInputs);
    }
}

/// How far and how fast each axis of this camera's rig may go.
fn limits(ui: &mut Ui, camera: &mut CameraConfig, units: Units, editable: bool) {
    ui.label(
        RichText::new("Auto time and the too-fast warning come from these. Set them to what this camera's rig does safely.")
            .color(MUTED)
            .small(),
    );
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("limits_grid").num_columns(5).spacing([PAD, GAP]).show(ui, |ui| {
            for head in ["", "Min", "Max", "Top speed /s", "Accel /s2"] {
                ui.label(RichText::new(head).small().color(MUTED));
            }
            ui.end_row();
            for axis in Axis::ALL {
                let lim = camera.axes.get_mut(axis);
                let step = step_for(axis);
                ui.label(RichText::new(axis.label()).color(axis_color(axis)));
                // Elevation is a length, shown in the show's units; the rest are degrees and shares.
                let field = |value| match axis {
                    Axis::Elevation => height_box(value, units),
                    _ => egui::DragValue::new(value).speed(step),
                };
                let (low, high) = (lim.min, lim.max);
                ui.add(field(&mut lim.min).range(-100000.0..=high - step));
                ui.add(field(&mut lim.max).range(low + step..=100000.0));
                ui.add(field(&mut lim.max_speed).range(step..=100000.0));
                ui.add(field(&mut lim.max_accel).range(step..=1000000.0));
                ui.end_row();
            }
        });
    });
}

/// The camera list: which cameras the show has, their order and names, and which one is selected.
fn cameras(ui: &mut Ui, s: &mut Session, project: &mut ptz_core::pose::Project, editable: bool) {
    let t = s.status.transport;
    let parked = t.run == Run::Stopped && t.output == Output::Rehearse && !t.estop;
    let selected = s.cam_id().to_string();
    let mut select = None;
    for (i, camera) in project.cameras.iter_mut().enumerate() {
        let status = s.status.camera(&camera.id);
        ui.horizontal(|ui| {
            let on = camera.id == selected;
            // The camera's number in its colour, in a thin square of it: heavier for the camera in hand.
            let chip = egui::Button::new(RichText::new(format!("{}", i + 1)).font(semibold(SMALL)).color(camera_color(i)))
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(if on { RULE_STRONG } else { RULE }, camera_color(i)))
                .min_size(egui::vec2(CONTROL_H, CONTROL_H));
            if ui.add(chip).on_hover_text("Select this camera").clicked() {
                select = Some(camera.id.clone());
            }
            ui.add_enabled_ui(editable, |ui| {
                if let Some(name) = crate::panels::text_field(ui, &format!("camera_name_{}", camera.id), &camera.name, 130.0) {
                    if !name.trim().is_empty() {
                        camera.name = name.trim().to_string();
                    }
                }
            });
            let (text, ink) = match status {
                Some(c) if c.fault.is_some() => ("fault", RED),
                Some(c) if c.hardware && c.adapters.iter().all(|a| a.connected) => ("rig connected", OFF_WHITE),
                Some(c) if c.hardware => ("rig not answering", AMBER),
                _ => ("simulator", MUTED),
            };
            ui.label(RichText::new(text).color(ink).small());
        });
    }
    if let Some(id) = select {
        s.select_camera(&id);
    }
    let at = project.camera_index(s.cam_id()).unwrap_or(0);
    let mine = s.presets.iter().filter(|p| p.camera == project.cameras[at].id).count();
    let why = if !editable {
        Some("Take the edit lease to change the cameras")
    } else if !parked {
        Some("Stop and disarm to add or remove a camera")
    } else {
        None
    };
    ui.horizontal_wrapped(|ui| {
        let full = project.cameras.len() >= MAX_CAMERAS;
        let add = ui
            .add_enabled(why.is_none() && !full, egui::Button::new("Add a camera"))
            .on_hover_text(
                "A new camera with the selected one's lens, limits and rules, on the simulator, two metres to its right",
            )
            .on_disabled_hover_text(why.unwrap_or("A show holds twelve cameras"));
        if add.clicked() {
            let from = project.cameras[at].clone();
            let id = project.free_camera_id();
            let name = format!("Camera {}", project.cameras.len() + 1);
            let mut camera = CameraConfig { id: id.clone(), name, ..from };
            camera.rig = ptz_core::pose::RigConfig {
                tolerance: camera.rig.tolerance,
                tracking_window_ms: camera.rig.tracking_window_ms,
                positioning_speed: camera.rig.positioning_speed,
                ..Default::default()
            };
            // To the right of the camera it was copied from, as that camera looks at the stage.
            let heading = camera.mount.heading_deg.to_radians();
            camera.mount.x += 2.0 * heading.cos();
            camera.mount.y += 2.0 * heading.sin();
            project.cameras.push(camera);
            s.active_camera = id;
        }
        let only = project.cameras.len() == 1;
        let confirm_id = ui.make_persistent_id("remove_camera");
        let asked = ui.data(|d| d.get_temp::<String>(confirm_id));
        let label = match &asked {
            Some(id) if *id == project.cameras[at].id => {
                format!("Remove it and its {mine} preset{}", if mine == 1 { "" } else { "s" })
            }
            _ => "Remove camera".to_string(),
        };
        let remove = ui
            .add_enabled(why.is_none() && !only, egui::Button::new(label))
            .on_hover_text(
                "Take the selected camera out of the show. Its presets go with it; its moves stay on the timeline as issues",
            )
            .on_disabled_hover_text(why.unwrap_or("A show needs one camera"));
        if remove.clicked() {
            let id = project.cameras[at].id.clone();
            if mine > 0 && asked.as_deref() != Some(id.as_str()) {
                ui.data_mut(|d| d.insert_temp(confirm_id, id));
            } else {
                ui.data_mut(|d| d.remove::<String>(confirm_id));
                project.cameras.remove(at);
                s.active_camera = project.cameras[at.min(project.cameras.len() - 1)].id.clone();
            }
        } else if asked.is_some() && asked.as_deref() != Some(project.cameras[at.min(project.cameras.len() - 1)].id.as_str()) {
            ui.data_mut(|d| d.remove::<String>(confirm_id));
        }
        let up = ui.add_enabled(why.is_none() && at > 0, egui::Button::new("Move up")).on_hover_text("Earlier in the numbering");
        if up.clicked() {
            project.cameras.swap(at, at - 1);
        }
        let down = ui
            .add_enabled(why.is_none() && at + 1 < project.cameras.len(), egui::Button::new("Move down"))
            .on_hover_text("Later in the numbering");
        if down.clicked() {
            project.cameras.swap(at, at + 1);
        }
    });
    ui.label(
        RichText::new(format!("{} of {MAX_CAMERAS} cameras. Keys 1 to 9, 0, - and = select them.", project.cameras.len()))
            .color(MUTED)
            .small(),
    );
}

/// The lens on the selected camera, and what it sees now.
fn lens(ui: &mut Ui, s: &Session, camera: &mut CameraConfig, units: Units, editable: bool) {
    let (pose, _) = s.camera_pose(&camera.id);
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("lens_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
            field(ui, "Lens");
            let presets = Lens::presets();
            let known = presets.iter().any(|p| p.name == camera.lens.name);
            let shown = if known { camera.lens.name.clone() } else { lens::CUSTOM.to_string() };
            egui::ComboBox::from_id_salt("lens_preset").selected_text(shown).width(210.0).show_ui(ui, |ui| {
                for preset in &presets {
                    if ui.selectable_label(camera.lens.name == preset.name, &preset.name).clicked() {
                        // The sensor is the camera's, not the lens's: it stays as set.
                        camera.lens = Lens { sensor_width_mm: camera.lens.sensor_width_mm, ..preset.clone() };
                    }
                }
                if ui.selectable_label(!known, lens::CUSTOM).clicked() && known {
                    camera.lens.name = lens::CUSTOM.to_string();
                }
            });
            ui.end_row();
            field(ui, "Focal length");
            ui.horizontal(|ui| {
                let (mut wide, mut tele) = (camera.lens.wide_mm, camera.lens.tele_mm);
                let a = ui.add(egui::DragValue::new(&mut wide).speed(0.5).range(4.0..=tele).suffix(" mm"));
                field(ui, "to");
                let b = ui.add(egui::DragValue::new(&mut tele).speed(0.5).range(wide..=2000.0).suffix(" mm"));
                if a.changed() || b.changed() {
                    camera.lens = Lens { name: lens::CUSTOM.to_string(), wide_mm: wide, tele_mm: tele, ..camera.lens.clone() };
                }
            })
            .response
            .on_hover_text("The widest and the longest end. A prime has the same number twice");
            ui.end_row();
            field(ui, "Aperture");
            let mut f = camera.lens.f_number;
            if ui.add(egui::DragValue::new(&mut f).speed(0.1).range(0.7..=64.0).prefix("f/")).changed() {
                camera.lens.f_number = f;
            }
            ui.end_row();
            field(ui, "Sensor");
            let sensor = |w: f64| {
                if (w - lens::FULL_FRAME_WIDTH_MM).abs() < 0.05 {
                    "Full frame".to_string()
                } else if (w - lens::SUPER_35_WIDTH_MM).abs() < 0.05 {
                    "Super 35".to_string()
                } else {
                    format!("{w:.1} mm wide")
                }
            };
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("lens_sensor")
                    .selected_text(sensor(camera.lens.sensor_width_mm))
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut camera.lens.sensor_width_mm, lens::FULL_FRAME_WIDTH_MM, "Full frame");
                        ui.selectable_value(&mut camera.lens.sensor_width_mm, lens::SUPER_35_WIDTH_MM, "Super 35");
                    });
                ui.add(egui::DragValue::new(&mut camera.lens.sensor_width_mm).speed(0.1).range(4.0..=70.0).suffix(" mm"))
                    .on_hover_text("Width of the part of the sensor that is recorded, 16:9");
            });
            ui.end_row();
        });
    });
    if let Err(why) = camera.lens.validate() {
        ui.label(RichText::new(why).color(AMBER).small());
        return;
    }
    let l = &camera.lens;
    let line = |ui: &mut Ui, what: &str, value: String| {
        ui.label(RichText::new(what).color(MUTED).small());
        ui.label(RichText::new(value).monospace().small());
        ui.end_row();
    };
    egui::Grid::new("lens_now").num_columns(2).spacing([4.0 * U, U]).show(ui, |ui| {
        line(ui, "Now", format!("{:.0} mm at zoom {:.0}%", l.focal_mm(pose.zoom), pose.zoom * 100.0));
        line(
            ui,
            "Angle of view",
            format!(
                "{:.1}° wide, {:.1}° high, {:.1}° diagonal",
                l.hfov_deg(pose.zoom),
                l.vfov_deg(pose.zoom),
                l.dfov_deg(pose.zoom)
            ),
        );
        line(
            ui,
            "Range",
            format!("{:.1}° at {:.0} mm to {:.1}° at {:.0} mm", l.hfov_deg(0.0), l.wide_mm, l.hfov_deg(1.0), l.tele_mm),
        );
        // The distance the figures below are worked out for: what the centre of the picture rests on, or a typed one.
        let id = ui.make_persistent_id("lens_distance");
        let (mut typed, mut use_typed) = ui.data_mut(|d| d.get_temp::<(f64, bool)>(id)).unwrap_or((10.0, false));
        let seen = s.world.centre_distance(camera, &pose).map(f64::from);
        ui.label(RichText::new("Centre of frame").color(MUTED).small());
        match seen {
            Some(d) => ui.label(RichText::new(format!("{} away", units.distance(d, 1))).monospace().small()),
            None => ui.label(RichText::new("looks at nothing on the stage").color(AMBER).small()),
        };
        ui.end_row();
        ui.label(RichText::new("At a distance of").color(MUTED).small());
        ui.horizontal(|ui| {
            if seen.is_some() {
                ui.checkbox(&mut use_typed, "")
                    .on_hover_text("Work the figures out for a typed distance instead of the centre of the frame");
            }
            let from_picture = seen.filter(|_| !use_typed);
            let mut shown = from_picture.unwrap_or(typed);
            let field = ui.add_enabled(from_picture.is_none(), distance_box(&mut shown, units).range(0.3..=500.0));
            if field.changed() {
                typed = shown;
            }
        });
        ui.end_row();
        ui.data_mut(|d| d.insert_temp(id, (typed, use_typed)));
        let distance = seen.filter(|_| !use_typed).unwrap_or(typed).max(0.3);
        let (w, h) = l.frame_at(pose.zoom, distance);
        line(ui, "The frame is", format!("{} wide, {} high", units.distance(w, 2), units.distance(h, 2)));
        let (near, far) = l.sharp_range_m(pose.zoom, distance);
        let far = far.map(|f| units.distance(f, 1)).unwrap_or_else(|| "infinity".into());
        line(ui, "Sharp from", format!("{} to {far} at f/{:.1}", units.distance(near, 1), l.f_number));
        line(ui, "Hyperfocal", units.distance(l.hyperfocal_m(pose.zoom), 1));
    });
    ui.label(
        RichText::new("Zoom is taken to run evenly in stops from the wide end to the long end. Bring-up can check it against the real lens.")
            .color(MUTED)
            .small(),
    );
}

/// Where the selected camera stands on the stage.
fn position(ui: &mut Ui, s: &Session, camera: &mut CameraConfig, units: Units, editable: bool) {
    ui.add_enabled_ui(editable, |ui| {
        let m = &mut camera.mount;
        egui::Grid::new("mount_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
            field(ui, "Floor position");
            ui.horizontal(|ui| {
                ui.add(distance_box(&mut m.x, units).range(-1000.0..=1000.0).prefix("x "));
                ui.add(distance_box(&mut m.y, units).range(-1000.0..=1000.0).prefix("y "));
                ui.add(distance_box(&mut m.z, units).range(-100.0..=100.0).prefix("z "))
                    .on_hover_text("Height of the floor or platform the pedestal stands on");
            });
            ui.end_row();
            field(ui, "Heading");
            ui.add(egui::DragValue::new(&mut m.heading_deg).speed(0.5).range(-360.0..=360.0).suffix("°"))
                .on_hover_text("Where pan 0 points, counter-clockwise seen from above. 0 looks along +Y, towards the stage");
            ui.end_row();
            field(ui, "Lens height");
            ui.add(height_box(&mut m.base_height_mm, units).range(0.0..=5000.0))
                .on_hover_text("Height of the lens above the floor with the column fully down");
            ui.end_row();
        });
        ui.horizontal_wrapped(|ui| {
            // The middle of the venue when one is imported; the middle of the built-in deck otherwise.
            let (middle, what) = match &s.project.stage.model {
                Some(model) => {
                    let (min, max) = model.bounds_m();
                    ([(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0], "the middle of the model")
                }
                None => ([0.0, 12.5], "the middle of the built-in stage"),
            };
            if ui.button("Face the stage").on_hover_text(format!("Turn the heading so pan 0 points at {what}")).clicked() {
                let to = crate::stage::heading_toward(m, [middle[0] as f32, middle[1] as f32, 0.0]);
                m.heading_deg = (to * 10.0).round() / 10.0;
            }
            if ui
                .button("Drop to the surface below")
                .on_hover_text(
                    "Set the floor height to the surface of the stage under the camera: a riser, a balcony, or the floor",
                )
                .clicked()
            {
                let from = [m.x as f32, m.y as f32, (m.z + (m.base_height_mm / 1000.0).max(0.5)) as f32];
                if let Some(p) = s.world.ground(from, [0.0, 0.0, -1.0]) {
                    m.z = (p[2] as f64 * 100.0).round() / 100.0;
                }
            }
        });
    });
    ui.label(RichText::new("The Stage view places cameras by dragging.").color(MUTED).small());
}

/// The venue: an imported model, where it stands, and whether the built-in stage is drawn.
fn stage_model(ui: &mut Ui, s: &mut Session, project: &mut ptz_core::pose::Project, editable: bool) {
    use crate::stage::Import;
    let why = (!editable).then(|| match s.can_edit() {
        Err(why) => why.replace("to edit the song", "to change the stage").replace("to edit", "to change the stage"),
        Ok(()) => String::new(),
    });
    ui.horizontal(|ui| {
        let import = ui
            .add_enabled(editable, egui::Button::new("Import a model"))
            .on_hover_text("A Wavefront .obj or an Autodesk .3ds file of the venue. It can also be dropped on the window")
            .on_disabled_hover_text(why.clone().unwrap_or_default());
        if import.clicked() {
            let picked = rfd::FileDialog::new()
                .set_title("Import a stage model")
                .add_filter("Stage model", &["obj", "3ds", "OBJ", "3DS"])
                .pick_file();
            if let Some(path) = picked {
                s.stage_import_wanted = Some(path);
            }
        }
        ui.add_enabled_ui(editable, |ui| {
            let id = ui.make_persistent_id("stage_path");
            let mut path = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_default();
            let field = ui.add(egui::TextEdit::singleline(&mut path).desired_width(150.0).hint_text("or a path, then Enter"));
            if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !path.trim().is_empty() {
                s.stage_import_wanted = Some(std::path::PathBuf::from(path.trim().trim_matches('"')));
                path.clear();
            }
            ui.data_mut(|d| d.insert_temp(id, path));
        });
    });
    match &s.world.import {
        Some(Import::Reading(name)) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(format!("Reading {name}")).color(AMBER));
            });
        }
        Some(Import::Uploading(name)) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new(format!("Sending {name} to the server")).color(AMBER));
            });
        }
        Some(Import::Failed { name, why }) => {
            ui.label(RichText::new(format!("{name} was not imported: {why}")).color(AMBER));
        }
        Some(Import::Done(_)) | None => {}
    }

    let Some(model) = project.stage.model.as_mut() else {
        ui.label(
            RichText::new("The built-in stage is shown. Import the venue to see what each camera sees from where it will stand.")
                .color(MUTED)
                .small(),
        );
        return;
    };
    ui.label(RichText::new(format!("{}   {} triangles", model.name, model.triangles)).strong());
    let size = model.size_m();
    let (low, high) = model.bounds_m();
    let units = project.units;
    let wide = |m: f64| units.distance(m, 1);
    ui.label(
        RichText::new(format!("{} wide, {} deep, {} high", wide(size[0]), wide(size[1]), wide(size[2]))).monospace().small(),
    )
    .on_hover_text("The size the model has with the unit below. If it is wrong, the unit is wrong");
    ui.label(
        RichText::new(format!(
            "It spans x {:.1} to {:.1}, y {:.1} to {:.1}, z {:.1} to {:.1} {}",
            low[0] / units.m_per_long(),
            high[0] / units.m_per_long(),
            low[1] / units.m_per_long(),
            high[1] / units.m_per_long(),
            low[2] / units.m_per_long(),
            high[2] / units.m_per_long(),
            units.long()
        ))
        .color(MUTED)
        .small(),
    );
    if s.world.waiting_for_model(&s.project.stage) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new("Fetching the model from the server").color(AMBER).small());
        });
    }
    ui.add_enabled_ui(editable, |ui| {
        egui::Grid::new("stage_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
            field(ui, "One unit is");
            egui::ComboBox::from_id_salt("stage_unit").selected_text(model.unit.label()).width(130.0).show_ui(ui, |ui| {
                for unit in ptz_core::camera::Unit::ALL {
                    ui.selectable_value(&mut model.unit, unit, unit.label());
                }
            });
            ui.end_row();
            field(ui, "Up");
            egui::ComboBox::from_id_salt("stage_up").selected_text(model.up.label()).width(130.0).show_ui(ui, |ui| {
                for up in ptz_core::camera::UpAxis::ALL {
                    ui.selectable_value(&mut model.up, up, up.label());
                }
            });
            ui.end_row();
            field(ui, "Turned");
            ui.add(egui::DragValue::new(&mut model.rotation_deg).speed(0.5).range(-360.0..=360.0).suffix("°"))
                .on_hover_text("About the vertical, counter-clockwise seen from above");
            ui.end_row();
            field(ui, "Moved");
            ui.horizontal(|ui| {
                for (k, axis) in ["x ", "y ", "z "].into_iter().enumerate() {
                    ui.add(distance_box(&mut model.offset[k], units).range(-100000.0..=100000.0).prefix(axis));
                }
            });
            ui.end_row();
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Sit on the floor").on_hover_text("Move the model so its lowest point is at floor height").clicked() {
                model.sit_on_floor();
            }
            if ui.button("Centre").on_hover_text("Move the model so the middle of its footprint is at the origin").clicked() {
                model.centre();
            }
        });
    });
    ui.add_enabled_ui(editable, |ui| {
        ui.checkbox(&mut project.stage.show_builtin, "Show the built-in stage as well");
        let confirm = ui.make_persistent_id("remove_stage_model");
        let asked = ui.data(|d| d.get_temp::<bool>(confirm)).unwrap_or(false);
        let label = if asked { "Remove it for every station" } else { "Remove the model" };
        let remove = ui.button(label).on_hover_text("Take the model off the server. The built-in stage is shown again");
        if remove.clicked() {
            if asked {
                s.link.send(Cmd::RemoveStageModel);
            }
            ui.data_mut(|d| d.insert_temp(confirm, !asked));
        }
    });
}
