//! The dock tabs other than the timeline and the monitors.

use crate::link::Cmd;
use crate::session::Session;
use crate::theme::*;
use crate::timeline_ui::PresetDrag;
use egui::{vec2, RichText, Sense, Stroke, StrokeKind, Ui};
use ptz_core::compile::{suggested_duration, Issue};
use ptz_core::edit;
use ptz_core::pose::{Axis, Preset};
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

pub fn section(ui: &mut Ui, title: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(title.to_uppercase()).small().color(MUTED));
    ui.add_space(2.0);
}

pub fn note(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).color(MUTED));
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

fn preset_card(ui: &mut Ui, preset: &Preset, picture: Option<egui::TextureId>) -> egui::Response {
    let width = ui.available_width().clamp(120.0, 400.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 58.0), Sense::hover());
    let p = ui.painter_at(rect);
    let fill = if resp.hovered() { CONTROL } else { LANE };
    p.rect(rect, 6.0, fill, Stroke::new(1.0, LINE), StrokeKind::Inside);
    let thumb = egui::Rect::from_min_size(rect.min + vec2(5.0, 5.0), THUMB);
    crate::stage::paint_picture(&p, thumb, picture);
    p.rect_stroke(thumb, 2.0, Stroke::new(1.0, LINE), StrokeKind::Outside);
    let x = thumb.right() + 10.0;
    p.text(egui::pos2(x, rect.top() + 9.0), egui::Align2::LEFT_TOP, &preset.name, egui::FontId::proportional(13.5), OFF_WHITE);
    let detail = format!(
        "{}  {}  zoom {}  {}",
        Axis::Pan.format(preset.pose.pan),
        Axis::Tilt.format(preset.pose.tilt),
        Axis::Zoom.format(preset.pose.zoom),
        Axis::Elevation.format(preset.pose.elevation)
    );
    p.text(egui::pos2(x, rect.top() + 30.0), egui::Align2::LEFT_TOP, detail, egui::FontId::proportional(10.5), MUTED);
    if let Some(n) = preset.camera_number {
        p.text(
            rect.right_top() + vec2(-8.0, 8.0),
            egui::Align2::RIGHT_TOP,
            format!("cam {n}"),
            egui::FontId::proportional(10.5),
            MUTED,
        );
    }
    resp
}

pub fn presets(ui: &mut Ui, s: &mut Session) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
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
        let sync = ui.add_enabled(why_not.is_none(), egui::Button::new("Sync from rig").small());
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
        let store = ui.add_enabled(can.is_ok(), egui::Button::new("Store current pose").small());
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
            ui.add_space(8.0);
            ui.spinner();
            ui.label(RichText::new(progress).color(AMBER));
        });
    }
    ui.add_space(2.0);
    let presets = s.camera_presets();
    if presets.is_empty() {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
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
        ui.add_space(2.0);
        s.probe_presets.clear();
        for preset in &presets {
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                let id = egui::Id::new(("preset_drag", &preset.id));
                let picture = preset_picture(ui, s, preset).or_else(|| drawn_picture(ui, s, preset));
                let inner = ui.dnd_drag_source(id, PresetDrag(preset.clone()), |ui| preset_card(ui, preset, picture));
                if !ui.ctx().is_being_dragged(id) {
                    s.probe_presets.push((preset.id.clone(), inner.response.rect));
                }
                inner.response.on_hover_text("Drag onto the Camera move lane").context_menu(|ui| {
                    if ui.button("Delete preset").clicked() {
                        s.link.send(Cmd::DeletePreset(preset.id.clone()));
                        ui.close();
                    }
                });
            });
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            note(ui, "Drag a preset onto the timeline. The move starts where the camera already is.");
        });
    });
}

// -------------------------------------------------------------- Inspector

fn tc_row(ui: &mut Ui, s: &Session, label: &str, frame: i64) {
    ui.label(label);
    ui.label(RichText::new(s.timecode_at(frame)).monospace());
    ui.end_row();
}

fn issue_rows(ui: &mut Ui, issues: &[Issue]) {
    for issue in issues {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("!").color(AMBER).strong());
            ui.label(RichText::new(issue.message()).color(AMBER));
        });
    }
}

fn inspect_move(ui: &mut Ui, s: &mut Session, id: &str) {
    let Some(song) = s.song.clone() else { return };
    let Some(clip) = song.motion_clip(id).cloned() else { return };
    let fps = s.project.rate.fps();
    ui.label(RichText::new(crate::timeline::move_label(&song, id, &s.project)).strong().size(14.0));
    let issues: Vec<Issue> = s.issues_for(id).into_iter().cloned().collect();
    let missing = s.project.camera(&clip.camera).is_none();
    if s.project.cameras.len() > 1 || missing {
        ui.horizontal(|ui| {
            ui.label("Camera");
            let shown = s.project.label(&clip.camera);
            egui::ComboBox::from_id_salt("move_camera")
                .selected_text(shown)
                .width(190.0)
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
        issue_rows(ui, &issues);
        ui.add_space(4.0);
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
            egui::Grid::new("move_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                ui.label("From");
                let from_text = match from {
                    FromPose::Linked(_) => "Where the camera already is".to_string(),
                    FromPose::Pinned(p) => p.name.clone(),
                };
                egui::ComboBox::from_id_salt("from").selected_text(from_text).width(190.0).show_ui(ui, |ui| {
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

                ui.label("To");
                egui::ComboBox::from_id_salt("to").selected_text(to.name.clone()).width(190.0).show_ui(ui, |ui| {
                    for p in presets.clone() {
                        if ui.selectable_label(to.preset_id == p.id, &p.name).clicked() {
                            s.edit(|song, project| edit::set_to(song, project, id, &p));
                        }
                    }
                });
                ui.end_row();

                tc_row(ui, s, "Start", clip.start_frame);
                tc_row(ui, s, "End", clip.end_frame());

                ui.label("Duration");
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

                ui.label("Ease");
                egui::ComboBox::from_id_salt("ease").selected_text(ease.label()).width(190.0).show_ui(ui, |ui| {
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
            egui::Grid::new("keys_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                tc_row(ui, s, "Start", clip.start_frame);
                tc_row(ui, s, "End", clip.end_frame());
                ui.label("Duration");
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
        ui.add_space(10.0);
        if ui
            .button("Convert to keys")
            .on_hover_text("Replace the move with two keys per axis that can be shaped in the graph editor")
            .clicked()
        {
            s.edit(|song, project| edit::convert_to_keys(song, project, id));
        }
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
    ui.label(RichText::new(&clip.name).strong().size(14.0));
    section(ui, if clip.track == ptz_core::song::Track::Video { "Reference video" } else { "Audio" });
    egui::Grid::new("media_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        tc_row(ui, s, "Start", clip.start_frame);
        tc_row(ui, s, "End", clip.end_frame());
        ui.label("Plays");
        ui.label(format!("{:.2} s of {:.2} s", clip.duration() as f64 / fps, clip.media_frames as f64 / fps));
        ui.end_row();
        if let Some(info) = s.media.iter().find(|m| m.id == clip.media_id) {
            ui.label("File");
            ui.label(format!("{} ch, {} Hz", info.channels, info.sample_rate));
            ui.end_row();
        }
    });
    let mut muted = clip.muted;
    if ui.checkbox(&mut muted, "Mute in Program").changed() {
        s.edit(|song, project| edit::set_media_muted(song, project, id, muted));
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
        egui::Grid::new("file_tc_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label("Starts at");
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
            ui.label("Read LTC");
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
                    ui.label(RichText::new("kept out of Program").color(MUTED).small());
                }
            });
            ui.end_row();
        });
    });

    // The clip against its stamp.
    let Some(code_frame) = info.frame_in(song, rate) else { return };
    let sits = clip.start_frame - clip.in_frame;
    if sits == code_frame {
        ui.label(RichText::new("On its timecode").color(GREEN).small());
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
    egui::Grid::new("marker_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Name");
        if let Some(name) = text_field(ui, &format!("marker_name_{id}"), &marker.name, 180.0) {
            s.edit(|song, project| edit::rename_marker(song, project, id, &name));
        }
        ui.end_row();
        tc_row(ui, s, "At", marker.frame);
    });
}

fn inspect_song(ui: &mut Ui, s: &mut Session) {
    let Some(song) = s.song.clone() else {
        ui.add_space(12.0);
        note(ui, "No song is open.");
        return;
    };
    let fps = s.project.rate.fps();
    section(ui, "Song");
    egui::Grid::new("song_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Name");
        if let Some(name) = text_field(ui, "insp_song_name", &song.name, 180.0) {
            s.edit(|song, project| edit::set_song_name(song, project, &name));
        }
        ui.end_row();
        ui.label("Starts at");
        if let Some(tc) = text_field(ui, "insp_song_start", &song.start_tc, 110.0) {
            match Timecode::parse_at(&tc, s.project.rate) {
                Some(_) => {
                    s.edit(|song, project| edit::set_song_start(song, project, &tc));
                }
                None => s.toast(format!("{tc} is not a timecode at {} fps. Use HH:MM:SS:FF", s.project.rate.label()), true),
            }
        }
        ui.end_row();
        ui.label("Length");
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
            let key = (issue.clip_id().map(str::to_string), issue.message());
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
                ui.label(RichText::new(issue.message()).color(AMBER));
            });
        }
    }

    section(ui, "Project");
    let editable = s.can_edit().is_ok();
    egui::Grid::new("project_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
        ui.label("Frame rate");
        ui.add_enabled_ui(editable, |ui| {
            egui::ComboBox::from_id_salt("rate").selected_text(s.project.rate.label()).show_ui(ui, |ui| {
                for rate in ptz_core::FrameRate::ALL {
                    if ui.selectable_label(s.project.rate == rate, rate.label()).clicked() && rate != s.project.rate {
                        let mut p = s.project.clone();
                        p.rate = rate;
                        s.link.send(Cmd::PutProject(Box::new(p)));
                    }
                }
            });
        });
        ui.end_row();
    });
    crate::settings_ui::project(ui, s);
}

pub fn inspector(ui: &mut Ui, s: &mut Session) {
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
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
                    ui.label(RichText::new(format!("{} items selected", many.len())).strong());
                    note(ui, "Drag to move them together. Delete removes them. Cmd-D copies them after the selection.");
                    let issues: Vec<Issue> =
                        many.iter().flat_map(|id| s.issues_for(id).into_iter().cloned().collect::<Vec<_>>()).collect();
                    if !issues.is_empty() {
                        section(ui, "Issues");
                        issue_rows(ui, &issues);
                    }
                }
            }
        });
    });
}

// -------------------------------------------------------------------- Log

/// A log record in words. With several cameras, a record about one of them starts with its number and name.
fn log_line(project: &ptz_core::Project, kind: &str, data: &serde_json::Value) -> String {
    let line = log_text(kind, data);
    match data.get("camera").and_then(|v| v.as_str()) {
        Some(camera) if project.cameras.len() > 1 => format!("{}: {line}", project.label(camera)),
        _ => line,
    }
}

fn log_text(kind: &str, data: &serde_json::Value) -> String {
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
                .map(|a| format!("{} {} ({:+.2})", a.label().to_lowercase(), a.format(value("pose", *a)), value("error", *a)))
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
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        note(ui, "The server's run log. Each record is chained to the one before it, so edits to the file are detected.");
    });
    if s.log.is_empty() {
        ui.add_space(10.0);
        ui.vertical_centered(|ui| note(ui, "Nothing has been logged since the server started."));
        return;
    }
    let row_h = 20.0;
    egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show_rows(
        ui,
        row_h,
        s.log.len(),
        |ui, rows| {
            for i in rows {
                let r = &s.log[i];
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    let time = r.time.get(11..19).unwrap_or(&r.time);
                    ui.label(RichText::new(format!("{:>5}", r.seq)).monospace().color(MUTED));
                    ui.label(RichText::new(time).monospace().color(MUTED));
                    let color = match r.kind.as_str() {
                        "estop" | "fault" | "camera_held" | "estop_halted" | "estop_still_moving" => RED,
                        "limit" | "lease" | "refused" | "issue_accepted" | "acceptance_withdrawn" => AMBER,
                        "arm" => GREEN,
                        "transport" => OFF_WHITE,
                        _ => BLUE,
                    };
                    ui.label(RichText::new(format!("{:<13}", r.kind)).monospace().color(color));
                    ui.label(RichText::new(log_line(&s.project, &r.kind, &r.data)).monospace());
                });
            }
        },
    );
}
