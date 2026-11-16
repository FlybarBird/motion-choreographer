//! The dock tabs other than the timeline and the monitors.

use crate::link::Cmd;
use crate::session::Session;
use crate::theme::*;
use crate::timeline_ui::PresetDrag;
use egui::{vec2, RichText, Sense, Stroke, StrokeKind, Ui};
use ptz_core::compile::{suggested_duration, Issue};
use ptz_core::edit;
use ptz_core::pose::{Axis, Preset, Units};
use ptz_core::song::{ClipBody, Ease, FromPose, PresetRef};
use ptz_core::time::Timecode;

/// A single-line text box that reports the new text once, when editing ends.
pub fn text_field(ui: &mut Ui, salt: &str, current: &str, width: f32) -> Option<String> {
    let id = ui.make_persistent_id(salt);
    let mut buf = ui.data_mut(|d| d.get_temp::<String>(id)).unwrap_or_else(|| current.to_string());
    let resp = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(width).id(id.with("edit")));
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

/// The heading of a section, drawn as the theme draws every one.
pub fn section(ui: &mut Ui, title: &str) {
    crate::theme::section(ui, title);
}

pub fn note(ui: &mut Ui, text: &str) {
    crate::theme::note(ui, text);
}

// ---------------------------------------------------------------- Presets

/// The camera's picture of a preset, asked for once and uploaded as a texture when it arrives.
fn preset_picture(ui: &Ui, s: &mut Session, preset: &Preset) -> Option<egui::TextureId> {
    use crate::session::PresetPicture;
    if !preset.has_thumbnail {
        return None;
    }
    match s.preset_pictures.get(&preset.id) {
        None => {
            s.preset_pictures.insert(preset.id.clone(), PresetPicture::Asked);
            s.link.send(Cmd::FetchPresetThumb(preset.id.clone()));
            None
        }
        Some(PresetPicture::Asked) => None,
        Some(PresetPicture::Shown(texture)) => Some(texture.id()),
        Some(PresetPicture::Decoded(image)) => {
            let texture = ui.ctx().load_texture(format!("preset:{}", preset.id), image.clone(), egui::TextureOptions::LINEAR);
            let id = texture.id();
            s.preset_pictures.insert(preset.id.clone(), PresetPicture::Shown(texture));
            Some(id)
        }
    }
}

/// The app's own drawing of a preset: its camera's view of the stage at the preset's pose,
/// with the other cameras at rest so the picture does not change while they move.
fn drawn_picture(ui: &Ui, s: &mut Session, preset: &Preset) -> Option<egui::TextureId> {
    let index = s.project.camera_index(&preset.camera)?;
    let mut poses = vec![ptz_core::pose::Pose::home(); s.project.cameras.len()];
    poses[index] = preset.pose;
    let size = crate::stage::pixel_size(egui::Rect::from_min_size(egui::Pos2::ZERO, THUMB), ui.ctx().pixels_per_point(), 256.0);
    crate::stage::camera_picture(ui.ctx(), s, &format!("preset:{}", preset.id), index, &poses, size, false)
}

const THUMB: egui::Vec2 = vec2(48.0 * 16.0 / 9.0, 48.0);

/// Room kept at the end of a preset's row for the button that opens its menu.
const PRESET_MENU_W: f32 = 8.0 * U;

/// What can be done to a stored preset: on its row's button, and on a right-click of the preset.
fn preset_menu(ui: &mut Ui, s: &mut Session, preset: &Preset) {
    let can = s.status.you_hold_lease && s.lost.is_none();
    let no_lease = "Take the edit lease to change presets";
    ui.label(RichText::new(&preset.name).strong());
    let update = ui
        .add_enabled(can, egui::Button::new("Update to the current pose"))
        .on_hover_text(
            "Store where the rig is now in this preset. Nothing moves. Moves already on the timeline keep the old pose until they are updated",
        )
        .on_disabled_hover_text(no_lease);
    if update.clicked() {
        s.link.send(Cmd::UpdatePreset(preset.id.clone(), Default::default()));
        ui.close();
    }
    ui.separator();
    let units = s.project.units;
    let limits = s.project.axes_of(&preset.camera).elevation;
    // What is typed is kept until it is set; a preset whose height has changed starts again from that.
    let id = ui.make_persistent_id(("preset_height", &preset.id, preset.pose.elevation.to_bits()));
    let mut mm = ui.data_mut(|d| d.get_temp::<f64>(id)).unwrap_or(preset.pose.elevation);
    ui.horizontal(|ui| {
        field(ui, "Height");
        ui.add_enabled(can, crate::units::height_box(&mut mm, units).range(limits.min..=limits.max))
            .on_hover_text("The column height this preset goes to. Double-click to type it")
            .on_disabled_hover_text(no_lease);
        let set = ui
            .add_enabled(can, egui::Button::new("Set height"))
            .on_hover_text("Store this column height in the preset. Pan, tilt, zoom and focus stay as they are")
            .on_disabled_hover_text(no_lease);
        if set.clicked() {
            s.link.send(Cmd::UpdatePreset(preset.id.clone(), ptz_core::api::UpdatePreset { pose: None, elevation: Some(mm) }));
            ui.close();
        }
    });
    ui.data_mut(|d| d.insert_temp(id, mm));
    ui.separator();
    if ui.button("Delete preset").clicked() {
        s.link.send(Cmd::DeletePreset(preset.id.clone()));
        ui.close();
    }
}

fn preset_card(ui: &mut Ui, preset: &Preset, units: Units, picture: Option<egui::TextureId>) -> egui::Response {
    let width = (ui.available_width() - PRESET_MENU_W).clamp(120.0, 400.0);
    // A row, not a box: its picture, its name and its pose, with a hairline under it.
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 16.0 * U), Sense::hover());
    let p = ui.painter_at(rect);
    if resp.hovered() {
        p.rect_filled(rect, 0.0, CONTROL);
    }
    let thumb = egui::Rect::from_min_size(rect.min + vec2(PAD, 2.0 * U), THUMB);
    crate::stage::paint_picture(&p, thumb, picture);
    p.rect_stroke(thumb, 0.0, Stroke::new(RULE, CONTROL_LINE), StrokeKind::Outside);
    let x = thumb.right() + PAD;
    p.text(egui::pos2(x, rect.top() + 3.0 * U), egui::Align2::LEFT_TOP, &preset.name, body(), OFF_WHITE);
    let detail = format!(
        "{}  {}  zoom {}  {}",
        Axis::Pan.format(preset.pose.pan),
        Axis::Tilt.format(preset.pose.tilt),
        Axis::Zoom.format(preset.pose.zoom),
        Axis::Elevation.format_in(preset.pose.elevation, units)
    );
    p.text(egui::pos2(x, rect.top() + 8.0 * U), egui::Align2::LEFT_TOP, detail, micro(), MUTED);
    if let Some(n) = preset.camera_number {
        p.text(rect.right_top() + vec2(-GAP, 3.0 * U), egui::Align2::RIGHT_TOP, format!("cam {n}"), micro(), MUTED);
    }
    resp
}

pub fn presets(ui: &mut Ui, s: &mut Session) {
    toolbar(ui, |ui| {
        let hardware = s.cam_status().hardware;
        let t = s.status.transport;
        let why_not = if s.cam_status().preset_sync.is_some() {
            Some("The camera is being read now")
        } else if !hardware {
            None
        } else if !s.status.you_hold_lease {
            Some("Take the edit lease to read presets from the camera")
        } else if t.output != ptz_core::transport::Output::Rehearse {
            Some("Disarm first: reading presets moves the camera")
        } else if t.run != ptz_core::transport::Run::Stopped {
            Some("Stop first: reading presets moves the camera")
        } else {
            None
        };
        let sync = ui.add_enabled(why_not.is_none(), egui::Button::new("Sync from rig"));
        let sync = match why_not {
            Some(why) => sync.on_disabled_hover_text(why),
            None if hardware => {
                sync.on_hover_text("Recall each preset on the camera and read its pose and picture. The camera moves")
            }
            None => sync.on_hover_text("Read the preset list and poses from the rig"),
        };
        if sync.clicked() {
            if hardware {
                s.confirm_sync = true;
            } else {
                s.link.send(Cmd::SyncPresets(s.cam_id().to_string()));
            }
        }
        let can = s.can_edit();
        let store = ui.add_enabled(can.is_ok(), egui::Button::new("Store current pose"));
        let store = match &can {
            Err(why) => store.on_disabled_hover_text(why.clone()),
            Ok(()) => store.on_hover_text("Save the pose in the monitor as a new preset"),
        };
        if store.clicked() {
            let (pose, _) = s.monitor_pose();
            let name = format!("Preset {}", s.camera_presets().len() + 1);
            let camera = Some(s.cam_id().to_string());
            s.link.send(Cmd::StorePreset(ptz_core::api::StorePreset { name, pose: Some(pose), camera }));
        }
    });
    if let Some(progress) = s.cam_status().preset_sync.clone() {
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.spinner();
            ui.label(RichText::new(progress).color(AMBER));
        });
    }
    let presets = s.camera_presets();
    if presets.is_empty() {
        empty(ui, |ui| {
            if s.project.cameras.len() > 1 {
                note(ui, &format!("No presets yet for {}.", s.cam().name));
            } else {
                note(ui, "No presets yet.");
            }
            note(ui, "Sync from rig reads them from the camera.");
        });
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        s.probe_presets.clear();
        for preset in &presets {
            let row = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let id = egui::Id::new(("preset_drag", &preset.id));
                let picture = preset_picture(ui, s, preset).or_else(|| drawn_picture(ui, s, preset));
                let units = s.project.units;
                let inner = ui.dnd_drag_source(id, PresetDrag(preset.clone()), |ui| preset_card(ui, preset, units, picture));
                if !ui.ctx().is_being_dragged(id) {
                    s.probe_presets.push((preset.id.clone(), inner.response.rect));
                }
                inner.response.on_hover_text("Drag onto the Camera move lane").context_menu(|ui| preset_menu(ui, s, preset));
                let dots = RichText::new(icon::DOTS_THREE_VERTICAL).font(icon_font(TITLE));
                ui.scope(|ui| {
                    // The menu's button is quiet: it shows its plate only under the pointer.
                    ui.visuals_mut().widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                    ui.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
                    ui.menu_button(dots, |ui| preset_menu(ui, s, preset))
                        .response
                        .on_hover_text(format!("{}: update to the current pose, set its height, or delete it", preset.name));
                });
            });
            let rule = row.response.rect;
            ui.painter().hline(ui.max_rect().x_range(), rule.bottom() - 0.5, Stroke::new(RULE, LINE));
        }
        ui.add_space(GAP);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            note(ui, "Drag a preset onto the timeline. The move starts where the camera already is.");
        });
    });
}

// -------------------------------------------------------------- Inspector

fn tc_row(ui: &mut Ui, s: &Session, label: &str, frame: i64) {
    field(ui, label);
    ui.label(RichText::new(s.timecode_at(frame)).monospace());
    ui.end_row();
}

fn issue_rows(ui: &mut Ui, issues: &[Issue], units: Units) {
    for issue in issues {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("!").color(AMBER).strong());
            ui.label(RichText::new(issue.message_in(units)).color(AMBER));
        });
    }
}

fn inspect_move(ui: &mut Ui, s: &mut Session, id: &str) {
    let Some(song) = s.song.clone() else { return };
    let Some(clip) = song.motion_clip(id).cloned() else { return };
    let fps = s.project.rate.fps();
    // A list is as wide as the panel has room for beside its label, up to its usual width.
    let list = (ui.available_width() - 20.0 * U).clamp(28.0 * U, 190.0);
    ui.label(RichText::new(crate::timeline::move_label(&song, id, &s.project)).font(semibold(BODY)));
    let issues: Vec<Issue> = s.issues_for(id).into_iter().cloned().collect();
    let missing = s.project.camera(&clip.camera).is_none();
    if s.project.cameras.len() > 1 || missing {
        ui.horizontal(|ui| {
            field(ui, "Camera");
            let shown = s.project.label(&clip.camera);
            egui::ComboBox::from_id_salt("move_camera")
                .selected_text(shown)
                .width(list)
                .show_ui(ui, |ui| {
                    for camera in s.project.cameras.clone() {
                        let label = s.project.label(&camera.id);
                        if ui.selectable_label(camera.id == clip.camera, label).clicked() && camera.id != clip.camera {
                            let ids = [id.to_string()];
                            s.edit(|song, project| edit::set_camera(song, project, &ids, &camera.id));
                            s.select_camera(&camera.id);
                        }
                    }
                })
                .response
                .on_hover_text("The camera that makes this move. Dragging the move to another camera's lane does the same");
        });
    }
    let presets: Vec<Preset> = s.presets.iter().filter(|p| p.camera == clip.camera).cloned().collect();

    if !issues.is_empty() {
        section(ui, "Issues");
        issue_rows(ui, &issues, s.project.units);
        ui.horizontal_wrapped(|ui| {
            let overrun = issues.iter().any(|i| i.acceptable());
            if clip.is_preset_move()
                && overrun
                && ui.button("Use auto time").on_hover_text("Lengthen the move to the fastest time the rig can do").clicked()
            {
                s.edit(|song, project| edit::set_auto_duration(song, project, id));
            }
            let all_acceptable = issues.iter().all(|i| i.acceptable());
            if clip.accepted.is_none() {
                let b = ui.add_enabled(all_acceptable, egui::Button::new("Accept as programmed"));
                let b = if all_acceptable {
                    b.on_hover_text("Allow Arm with this overrun. The rig will lag behind the timeline here")
                } else {
                    b.on_disabled_hover_text("Range, jump and keep-out problems cannot be accepted. Change the move")
                };
                if b.clicked() {
                    let by = s.station.clone();
                    let at = now_text();
                    s.edit(|song, project| edit::accept_issues(song, project, id, &by, &at));
                }
            } else if ui.button("Withdraw acceptance").clicked() {
                s.edit(|song, project| edit::clear_acceptance(song, project, id));
            }
        });
        if let Some(a) = &clip.accepted {
            ui.label(RichText::new(format!("Accepted by {} at {}", a.by, a.at)).small().color(MUTED));
        }
    }

    match &clip.body {
        ClipBody::PresetMove { auto_duration, from, to, ease } => {
            section(ui, "Preset move");
            egui::Grid::new("move_grid").num_columns(2).spacing([PAD, GAP]).show(ui, |ui| {
                field(ui, "From");
                let from_text = match from {
                    FromPose::Linked(_) => "Where the camera already is".to_string(),
                    FromPose::Pinned(p) => p.name.clone(),
                };
                egui::ComboBox::from_id_salt("from").selected_text(from_text).width(list).show_ui(ui, |ui| {
                    if ui.selectable_label(from.is_linked(), "Where the camera already is").clicked() {
                        s.edit(|song, project| edit::set_from(song, project, id, FromPose::linked()));
                    }
                    for p in presets.clone() {
                        let on = matches!(from, FromPose::Pinned(r) if r.preset_id == p.id);
                        if ui.selectable_label(on, &p.name).clicked() {
                            s.edit(|song, project| edit::set_from(song, project, id, FromPose::Pinned(PresetRef::of(&p))));
                        }
                    }
                });
                ui.end_row();

                field(ui, "To");
                egui::ComboBox::from_id_salt("to").selected_text(to.name.clone()).width(list).show_ui(ui, |ui| {
                    for p in presets.clone() {
                        if ui.selectable_label(to.preset_id == p.id, &p.name).clicked() {
                            s.edit(|song, project| edit::set_to(song, project, id, &p));
                        }
                    }
                });
                ui.end_row();

                // A move keeps its own copy of the pose, so a preset stored again later does not reach it by itself.
                let newer = |r: &PresetRef| presets.iter().find(|p| p.id == r.preset_id && PresetRef::of(p) != *r).cloned();
                let stale_from = match from {
                    FromPose::Pinned(r) => newer(r),
                    FromPose::Linked(_) => None,
                };
                let stale_to = newer(to);
                if stale_from.is_some() || stale_to.is_some() {
                    ui.label("");
                    ui.horizontal(|ui| {
                        let which = stale_to.as_ref().or(stale_from.as_ref()).map(|p| p.name.clone()).unwrap_or_default();
                        ui.label(RichText::new(format!("{which} has changed")).color(AMBER)).on_hover_text(
                            "The preset was stored again after this move was made. The move still uses the old pose",
                        );
                        if ui.button("Update").on_hover_text("Use the preset as it is now").clicked() {
                            s.edit(|song, project| {
                                let mut next = song.clone();
                                if let Some(p) = &stale_from {
                                    next = edit::set_from(&next, project, id, FromPose::Pinned(PresetRef::of(p)))?;
                                }
                                if let Some(p) = &stale_to {
                                    next = edit::set_to(&next, project, id, p)?;
                                }
                                Ok(next)
                            });
                        }
                    });
                    ui.end_row();
                }

                tc_row(ui, s, "Start", clip.start_frame);
                tc_row(ui, s, "End", clip.end_frame());

                field(ui, "Duration");
                ui.horizontal(|ui| {
                    if ui.selectable_label(*auto_duration, "Auto").on_hover_text("As fast as the rig's limits allow").clicked()
                        && !auto_duration
                    {
                        s.edit(|song, project| edit::set_auto_duration(song, project, id));
                    }
                    if ui.selectable_label(!auto_duration, "Custom").clicked() && *auto_duration {
                        let frames = clip.duration_frames;
                        s.edit(|song, project| edit::set_custom_duration(song, project, id, frames));
                    }
                    let mut secs = clip.duration_frames as f64 / fps;
                    let dv = ui
                        .add(egui::DragValue::new(&mut secs).speed(0.05).range(1.0 / fps..=600.0).suffix(" s").fixed_decimals(2));
                    if dv.changed() {
                        let frames = (secs * fps).round().max(1.0) as i64;
                        s.edit_coalesced(&format!("dur:{id}"), |song, project| {
                            edit::set_custom_duration(song, project, id, frames)
                        });
                    }
                });
                ui.end_row();

                if let Some(auto) = suggested_duration(&song, &clip, &s.project) {
                    ui.label("");
                    ui.label(RichText::new(format!("Auto time is {:.2} s", auto as f64 / fps)).color(MUTED).small());
                    ui.end_row();
                }

                field(ui, "Ease");
                egui::ComboBox::from_id_salt("ease").selected_text(ease.label()).width(list).show_ui(ui, |ui| {
                    for e in Ease::ALL {
                        if ui.selectable_label(*ease == e, e.label()).clicked() {
                            s.edit(|song, project| edit::set_ease(song, project, id, e));
                        }
                    }
                });
                ui.end_row();
            });
        }
        ClipBody::Keys { curves } => {
            section(ui, if clip.recorded_pass.is_some() { "Recorded keys" } else { "Keys" });
            egui::Grid::new("keys_grid").num_columns(2).spacing([PAD, GAP]).show(ui, |ui| {
                tc_row(ui, s, "Start", clip.start_frame);
                tc_row(ui, s, "End", clip.end_frame());
                field(ui, "Duration");
                ui.label(format!("{:.2} s", clip.duration_frames as f64 / fps));
                ui.end_row();
                for (axis, keys) in curves {
                    ui.label(RichText::new(axis.label()).color(axis_color(*axis)));
                    ui.label(format!("{} keys", keys.len()));
                    ui.end_row();
                }
            });
            if let Some(pass) = &clip.recorded_pass {
                section(ui, "Recorded pass");
                note(ui, "Fewer keys are easier to edit; more keys follow the take closer.");
                ui.horizontal(|ui| {
                    for (label, scale) in [("Fewer keys", 3.0), ("Default", 1.0), ("More keys", 0.35)] {
                        if ui.button(label).clicked() {
                            s.link.send(Cmd::Refit { clip: id.to_string(), scale });
                        }
                    }
                });
                smooth_button(ui, s, "Smooth this camera's moves", Some(clip.camera.clone()));
                ui.label(RichText::new(format!("Raw samples: {pass}")).small().color(MUTED));
            }
        }
    }

    section(ui, "Axes this move drives");
    ui.horizontal_wrapped(|ui| {
        for axis in Axis::ALL {
            let mut on = clip.axes.contains(&axis);
            let enabled = clip.is_preset_move();
            if ui
                .add_enabled(enabled, egui::Checkbox::new(&mut on, RichText::new(axis.label()).color(axis_color(axis))))
                .changed()
            {
                let mut axes = clip.axes.clone();
                if on {
                    axes.push(axis);
                } else {
                    axes.retain(|a| *a != axis);
                }
                axes.sort_by_key(|a| a.index());
                s.edit(|song, project| edit::set_axes(song, project, id, &axes));
            }
        }
    });

    if clip.is_preset_move() {
        ui.add_space(U);
        if ui
            .button("Convert to keys")
            .on_hover_text("Replace the move with two keys per axis that can be shaped in the graph editor")
            .clicked()
        {
            s.edit(|song, project| edit::convert_to_keys(song, project, id));
        }
    }
}

/// Smooth recorded moves to the rigs' limits: one camera's, or every camera's in the song.
fn smooth_button(ui: &mut Ui, s: &mut Session, label: &str, camera: Option<String>) {
    let can = s.can_edit();
    let b = ui.add_enabled(can.is_ok(), egui::Button::new(label));
    let b = match &can {
        Err(why) => b.on_disabled_hover_text(why.clone()),
        Ok(()) => b.on_hover_text(
            "Smooth each recorded move that is over a speed or acceleration limit until the rig can make it, and accept \
             what is still over. The limits are not changed, the raw samples are kept, and one undo puts the song back",
        ),
    };
    if b.clicked() {
        s.link.send(Cmd::Smooth { camera });
    }
}

fn now_text() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let (h, m, sec) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let days = secs / 86400;
    // Civil date from days since 1970-01-01.
    let z = days as i64 + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(mo <= 2);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{sec:02}Z")
}

fn inspect_media(ui: &mut Ui, s: &mut Session, id: &str) {
    let Some(song) = s.song.clone() else { return };
    let Some(clip) = song.media_clip(id).cloned() else { return };
    let fps = s.project.rate.fps();
    ui.label(RichText::new(&clip.name).font(semibold(BODY)));
    section(ui, if clip.track == ptz_core::song::Track::Video { "Reference video" } else { "Audio" });
    egui::Grid::new("media_grid").num_columns(2).spacing([PAD, GAP]).show(ui, |ui| {
        tc_row(ui, s, "Start", clip.start_frame);
        tc_row(ui, s, "End", clip.end_frame());
        field(ui, "Plays");
        ui.label(format!("{:.2} s of {:.2} s", clip.duration() as f64 / fps, clip.media_frames as f64 / fps));
        ui.end_row();
        if let Some(info) = s.media.iter().find(|m| m.id == clip.media_id) {
            field(ui, "File");
            ui.label(format!("{} ch, {} Hz", info.channels, info.sample_rate));
            ui.end_row();
        }
    });
    let mut muted = clip.muted;
    if ui.checkbox(&mut muted, "Mute in Program").changed() {
        s.edit(|song, project| edit::set_media_muted(song, project, id, muted));
    }
    if matches!(s.status.lock.source.as_deref(), Some("ltc" | "mtc" | "artnet")) {
        ui.label(
            RichText::new(
                "Not played while the playhead follows timecode from outside: the sound comes from where the timecode does.",
            )
            .color(MUTED)
            .small(),
        );
    }
    if let Some(info) = s.media.iter().find(|m| m.id == clip.media_id).cloned() {
        file_timecode(ui, s, &song, &clip, &info);
        lock_to_file(ui, s, &clip, &info);
    }
}

/// Where a stamp came from, in the operator's words.
pub fn stamp_source_label(source: &str) -> String {
    match source {
        "bext" => "from the broadcast WAV".into(),
        "quicktime" => "from the video's timecode track".into(),
        "typed" => "typed in".into(),
        other => match other.strip_prefix("ltc channel ") {
            Some(n) => format!("read from LTC on channel {n}"),
            None => other.to_string(),
        },
    }
}

/// The file's own start timecode: shown, typed, or read from an LTC channel; and whether the clip sits on it.
fn file_timecode(
    ui: &mut Ui,
    s: &mut Session,
    song: &ptz_core::song::Song,
    clip: &ptz_core::song::MediaClip,
    info: &ptz_core::api::MediaInfo,
) {
    let rate = s.project.rate;
    let editable = s.can_edit();
    section(ui, "File timecode");
    ui.add_enabled_ui(editable.is_ok(), |ui| {
        egui::Grid::new("file_tc_grid").num_columns(2).spacing([PAD, GAP]).show(ui, |ui| {
            field(ui, "Starts at");
            ui.horizontal(|ui| {
                let current = info.start_tc.clone().unwrap_or_default();
                if let Some(typed) = text_field(ui, &format!("file_tc_{}", info.id), &current, 96.0) {
                    let typed = typed.trim().to_string();
                    if typed.is_empty() || Timecode::parse_at(&typed, rate).is_some() {
                        s.link.send(Cmd::SetMediaTc { id: info.id.clone(), start_tc: typed });
                    } else {
                        s.toast(format!("{typed} is not a timecode at {} fps. Use HH:MM:SS:FF", rate.label()), true);
                    }
                }
                match &info.start_tc_source {
                    Some(source) => ui.label(RichText::new(stamp_source_label(source)).color(MUTED).small()),
                    None => ui.label(RichText::new("none in the file: type one, or lock by position").color(MUTED).small()),
                };
            });
            ui.end_row();
            field(ui, "Read LTC");
            ui.horizontal(|ui| {
                let picked = info.ltc_channel.map(|c| format!("Channel {}", c + 1)).unwrap_or_else(|| "from channel...".into());
                egui::ComboBox::from_id_salt(("ltc_channel", &info.id)).selected_text(picked).width(118.0).show_ui(ui, |ui| {
                    for n in 1..=info.channels.max(1) {
                        if ui.selectable_label(info.ltc_channel == Some(n - 1), format!("Channel {n}")).clicked() {
                            s.link.send(Cmd::SetMediaTc { id: info.id.clone(), start_tc: format!("ltc:{n}") });
                        }
                    }
                });
                if info.ltc_channel.is_some() {
                    ui.label(RichText::new("sync only, never played").color(MUTED).small());
                }
            });
            ui.end_row();
        });
        if info.ltc_channel.is_some() {
            ui.label(RichText::new("The file's sound plays on both the left and the right output.").color(MUTED).small());
        }
    });

    // The clip against its stamp.
    let Some(code_frame) = info.frame_in(song, rate) else { return };
    let sits = clip.start_frame - clip.in_frame;
    if sits == code_frame {
        // Said in plain ink: green is kept for output that reaches the rig.
        ui.label(RichText::new("On its timecode").small());
        return;
    }
    let file_start = info.start_tc.clone().unwrap_or_default();
    let fits = code_frame + clip.in_frame >= 0 && code_frame + clip.in_frame < song.length_frames;
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new("!").color(AMBER).strong());
        let off = sits - code_frame;
        let which = if off > 0 { "late" } else { "early" };
        let text = if fits {
            format!("Sits {} frames {which} against its timecode", off.abs())
        } else {
            format!("Its timecode, {file_start}, is outside this song")
        };
        ui.label(RichText::new(text).color(AMBER));
    });
    ui.add_enabled_ui(editable.is_ok(), |ui| {
        if fits {
            if ui.button("Move to its timecode").clicked() {
                let to = code_frame + clip.in_frame;
                let id = clip.id.clone();
                s.edit(|song, project| edit::move_clip(song, project, &id, to));
            }
        } else if ui
            .button(format!("Set the song start to {file_start}"))
            .on_hover_text("The song starts on the file's first frame. Everything else keeps its place and takes new timecodes")
            .clicked()
        {
            let (id, to) = (clip.id.clone(), clip.in_frame);
            s.edit(|song, project| {
                let moved = edit::set_song_start(song, project, &file_start)?;
                edit::move_clip(&moved, project, &id, to)
            });
        }
    });
}

/// Lock the playhead to this file, or let go of it.
fn lock_to_file(ui: &mut Ui, s: &mut Session, clip: &ptz_core::song::MediaClip, info: &ptz_core::api::MediaInfo) {
    use ptz_core::transport::Action;
    section(ui, "Lock");
    let source = format!("file:{}", clip.id);
    let locked_here = s.status.lock.source.as_deref() == Some(source.as_str());
    let g = s.status.guidance.clone();
    if locked_here {
        ui.label(RichText::new("The playhead follows this file. Play to code rolls it.").color(MUTED).small());
        let unlock = ui.add_enabled(g.allows(Action::Unlock), egui::Button::new("Unlock"));
        let unlock = match g.reason(Action::Unlock) {
            Some(why) => unlock.on_disabled_hover_text(why),
            None => unlock,
        };
        if unlock.clicked() {
            s.act(Action::Unlock);
        }
        return;
    }
    let how = if info.start_tc.is_some() {
        "The file's timecode becomes the show's clock: exact, with no drift."
    } else {
        "The file has no timecode, so it locks from its first sample where the clip sits."
    };
    ui.label(RichText::new(how).color(MUTED).small());
    let lock = ui.add_enabled(g.allows(Action::Lock) && s.lost.is_none(), egui::Button::new("Lock to this file"));
    let lock = match g.reason(Action::Lock) {
        Some(why) => lock.on_disabled_hover_text(why),
        None => lock,
    };
    if lock.clicked() {
        s.link.send(Cmd::Act(Action::Lock, ptz_core::api::TransportRequest { source: Some(source), ..Default::default() }));
    }
}

fn inspect_marker(ui: &mut Ui, s: &mut Session, id: &str) {
    let Some(song) = s.song.clone() else { return };
    let Some(marker) = song.markers.iter().find(|m| m.id == id).cloned() else { return };
    section(ui, "Marker");
    egui::Grid::new("marker_grid").num_columns(2).spacing([PAD, GAP]).show(ui, |ui| {
        field(ui, "Name");
        if let Some(name) = text_field(ui, &format!("marker_name_{id}"), &marker.name, 180.0) {
            s.edit(|song, project| edit::rename_marker(song, project, id, &name));
        }
        ui.end_row();
        tc_row(ui, s, "At", marker.frame);
    });
}

fn inspect_song(ui: &mut Ui, s: &mut Session) {
    let Some(song) = s.song.clone() else {
        ui.add_space(PAD);
        note(ui, "No song is open.");
        return;
    };
    let fps = s.project.rate.fps();
    section(ui, "Song");
    egui::Grid::new("song_grid").num_columns(2).spacing([PAD, GAP]).show(ui, |ui| {
        field(ui, "Name");
        if let Some(name) = text_field(ui, "insp_song_name", &song.name, 180.0) {
            s.edit(|song, project| edit::set_song_name(song, project, &name));
        }
        ui.end_row();
        field(ui, "Starts at");
        if let Some(tc) = text_field(ui, "insp_song_start", &song.start_tc, 110.0) {
            match Timecode::parse_at(&tc, s.project.rate) {
                Some(_) => {
                    s.edit(|song, project| edit::set_song_start(song, project, &tc));
                }
                None => s.toast(format!("{tc} is not a timecode at {} fps. Use HH:MM:SS:FF", s.project.rate.label()), true),
            }
        }
        ui.end_row();
        field(ui, "Length");
        let mut secs = song.length_frames as f64 / fps;
        let floor = (song.content_end() as f64 / fps).max(1.0);
        if ui.add(egui::DragValue::new(&mut secs).speed(0.5).range(floor..=7200.0).suffix(" s").fixed_decimals(1)).changed() {
            let frames = (secs * fps).round() as i64;
            s.edit_coalesced("song_length", |song, project| edit::set_song_length(song, project, frames));
        }
        ui.end_row();
        tc_row(ui, s, "Ends at", song.length_frames);
    });

    if !s.compiled.issues.is_empty() {
        section(ui, "Issues in this song");
        let issues = s.compiled.issues.clone();
        let mut seen = std::collections::BTreeSet::new();
        for issue in &issues {
            let key = (issue.clip_id().map(str::to_string), issue.message_in(s.project.units));
            if !seen.insert(key) {
                continue;
            }
            ui.horizontal_wrapped(|ui| {
                let at = s.timecode_at(issue.frame());
                if ui.link(RichText::new(at).monospace().color(AMBER)).on_hover_text("Go there").clicked() {
                    s.seek(issue.frame());
                    if let Some(id) = issue.clip_id() {
                        s.timeline.selection.clear();
                        s.timeline.selection.insert(id.to_string());
                    }
                }
                ui.label(RichText::new(issue.message_in(s.project.units)).color(AMBER));
            });
        }
    }

    if song.motion.iter().any(|c| c.recorded_pass.is_some()) {
        section(ui, "Recorded moves");
        note(ui, "A take is as jumpy as the readings it was made from. Smoothing fits it to what each rig can do.");
        smooth_button(ui, s, "Smooth recorded moves", None);
        if let Some((said, left)) = &s.smoothed {
            ui.label(RichText::new(said).small().color(if *left { AMBER } else { MUTED }));
        }
    }

    section(ui, "The whole show");
    note(ui, "Cameras, rigs, lenses, the stage, audio output and timecode inputs are in Show settings.");
    if icon_button(ui, true, icon::GEAR_SIX, "Open Show settings", Look::Plain, egui::vec2(0.0, CONTROL_H)).clicked() {
        s.settings = Some(crate::settings_ui::Page::default());
    }
}

pub fn inspector(ui: &mut Ui, s: &mut Session) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(panel_margin()).show(ui, |ui| {
            let ids = s.selected_ids();
            let Some(song) = s.song.clone() else {
                inspect_song(ui, s);
                return;
            };
            match ids.as_slice() {
                [] => inspect_song(ui, s),
                [id] if song.motion_clip(id).is_some() => inspect_move(ui, s, id),
                [id] if song.media_clip(id).is_some() => inspect_media(ui, s, id),
                [id] if song.markers.iter().any(|m| &m.id == id) => inspect_marker(ui, s, id),
                [_] => inspect_song(ui, s),
                many => {
                    ui.label(RichText::new(format!("{} items selected", many.len())).font(semibold(BODY)));
                    note(ui, "Drag to move them together. Delete removes them. Cmd-D copies them after the selection.");
                    let issues: Vec<Issue> =
                        many.iter().flat_map(|id| s.issues_for(id).into_iter().cloned().collect::<Vec<_>>()).collect();
                    if !issues.is_empty() {
                        section(ui, "Issues");
                        issue_rows(ui, &issues, s.project.units);
                    }
                }
            }
        });
    });
}

// -------------------------------------------------------------------- Log

/// A log record in words. With several cameras, a record about one of them starts with its number and name.
fn log_line(project: &ptz_core::Project, kind: &str, data: &serde_json::Value) -> String {
    let line = log_text(kind, data, project.units);
    match data.get("camera").and_then(|v| v.as_str()) {
        Some(camera) if project.cameras.len() > 1 => format!("{}: {line}", project.label(camera)),
        _ => line,
    }
}

fn log_text(kind: &str, data: &serde_json::Value, units: Units) -> String {
    let text = |k: &str| data.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match kind {
        "camera_held" => {
            let rest = data.get("still_running").and_then(|v| v.as_u64()).unwrap_or(0);
            match rest {
                0 => "held where it stopped".to_string(),
                1 => "held where it stopped; 1 other camera carries on".to_string(),
                n => format!("held where it stopped; {n} other cameras carry on"),
            }
        }
        "cameras_changed" => {
            let names = |k: &str| -> Vec<String> {
                data.get(k)
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
                    .unwrap_or_default()
            };
            let (added, removed) = (names("added"), names("removed"));
            let mut parts = Vec::new();
            if !added.is_empty() {
                parts.push(format!("added {}", added.join(", ")));
            }
            if !removed.is_empty() {
                parts.push(format!("removed {}", removed.join(", ")));
            }
            if parts.is_empty() {
                parts.push("reordered".to_string());
            }
            let count = data.get("cameras").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            format!("{}; the show has {count}", parts.join(", "))
        }
        "transport" => {
            let action = text("action");
            let by = text("by");
            let frame = data.get("frame").and_then(|v| v.as_i64()).map(|f| format!(" at frame {f}")).unwrap_or_default();
            format!("{action}{frame}  ({by})")
        }
        "refused" => format!("{} refused: {}  ({})", text("action"), text("reason"), text("by")),
        "arm" => {
            let revision = data.get("revision").and_then(|v| v.as_u64()).unwrap_or(0);
            let sha: String = text("sha256").chars().take(12).collect();
            let accepted = data.get("accepted").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            let accepted = match accepted {
                0 => String::new(),
                1 => ", 1 accepted issue".to_string(),
                n => format!(", {n} accepted issues"),
            };
            format!("{} revision {revision} ({sha}){accepted}  ({})", text("song"), text("by"))
        }
        "fault" => text("reason"),
        "estop_halted" => {
            let ms = data.get("after_ms").and_then(|v| v.as_u64()).unwrap_or(0);
            format!("the rig stood still {ms} ms after E-Stop")
        }
        "estop_still_moving" => "the rig is still moving; stop sent again".to_string(),
        "adapter" => {
            let up = data.get("connected").and_then(|v| v.as_bool()).unwrap_or(false);
            format!("{} {}  {}", text("name"), if up { "connected" } else { "disconnected" }, text("detail"))
        }
        "pose" => {
            let frame = data.get("frame").and_then(|v| v.as_i64()).unwrap_or(0);
            let value = |group: &str, axis: Axis| {
                let key = axis.label().to_lowercase();
                data.get(group).and_then(|g| g.get(&key)).and_then(|v| v.as_f64()).unwrap_or(0.0)
            };
            let axes: Vec<String> = Axis::ALL
                .iter()
                .map(|a| {
                    format!("{} {} ({:+.2})", a.label().to_lowercase(), a.format_in(value("pose", *a), units), value("error", *a))
                })
                .collect();
            format!("frame {frame}   {}", axes.join("   "))
        }
        "chase" => {
            let jumped = if data.get("jumped").and_then(|v| v.as_bool()).unwrap_or(false) { ", code jumped" } else { "" };
            format!("{} {}{jumped}", text("source"), text("state"))
        }
        "issue_accepted" => {
            let issues = data.get("issues").and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            format!("{} accepted {issues} issue(s) on {} as programmed", text("by"), text("clip"))
        }
        "record_smooth" => {
            let count = |k: &str| data.get(k).and_then(|v| v.as_array()).map(|a| a.len()).unwrap_or(0);
            let left = data.get("left").and_then(|v| v.as_u64()).unwrap_or(0);
            format!(
                "{} smoothed {} recorded move(s) and accepted {}; {left} issue(s) left  ({})",
                text("song"),
                count("smoothed"),
                count("accepted"),
                text("by")
            )
        }
        _ => {
            let mut parts = Vec::new();
            if let Some(obj) = data.as_object() {
                for (k, v) in obj {
                    let v = match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    if v.len() <= 80 {
                        parts.push(format!("{k} {v}"));
                    }
                }
            }
            parts.join("   ")
        }
    }
}

pub fn log(ui: &mut Ui, s: &mut Session) {
    toolbar(ui, |ui| {
        ui.label(
            RichText::new(
                "The server's run log. Each record is chained to the one before it, so edits to the file are detected.",
            )
            .color(MUTED)
            .small(),
        );
    });
    if s.log.is_empty() {
        empty(ui, |ui| note(ui, "Nothing has been logged since the server started."));
        return;
    }
    let row_h = 5.0 * U;
    ui.spacing_mut().item_spacing.y = 0.0;
    egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show_rows(
        ui,
        row_h,
        s.log.len(),
        |ui, rows| {
            for i in rows {
                let r = &s.log[i];
                ui.horizontal(|ui| {
                    ui.add_space(PAD);
                    let time = r.time.get(11..19).unwrap_or(&r.time);
                    ui.label(RichText::new(format!("{:>5}", r.seq)).monospace().color(MUTED));
                    ui.label(RichText::new(time).monospace().color(MUTED));
                    let color = match r.kind.as_str() {
                        "estop" | "fault" | "camera_held" | "estop_halted" | "estop_still_moving" => RED,
                        "limit" | "lease" | "refused" | "issue_accepted" | "acceptance_withdrawn" => AMBER,
                        "arm" => GREEN,
                        "transport" => OFF_WHITE,
                        _ => MUTED,
                    };
                    ui.label(RichText::new(format!("{:<13}", r.kind)).monospace().color(color));
                    ui.label(RichText::new(log_line(&s.project, &r.kind, &r.data)).monospace());
                });
            }
        },
    );
}
