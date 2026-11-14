//! The dock tabs other than the timeline and the monitors.

use crate::link::Cmd;
use crate::monitor;
use crate::session::Session;
use crate::theme::*;
use crate::timeline_ui::PresetDrag;
use egui::{vec2, Color32, RichText, Sense, Stroke, StrokeKind, Ui};
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

fn preset_card(ui: &mut Ui, preset: &Preset, selected: bool) -> egui::Response {
    let width = ui.available_width().clamp(120.0, 400.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 58.0), Sense::hover());
    let p = ui.painter_at(rect);
    let fill = if resp.hovered() { Color32::from_rgb(0x34, 0x36, 0x46) } else { LANE };
    p.rect(rect, 6.0, fill, Stroke::new(1.0, if selected { BLUE } else { LINE }), StrokeKind::Inside);
    let thumb = egui::Rect::from_min_size(rect.min + vec2(5.0, 5.0), vec2(48.0 * 16.0 / 9.0, 48.0));
    monitor::paint_stage(&p, thumb, &preset.pose);
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
        if ui.small_button("Sync from rig").on_hover_text("Read the preset list and poses from the camera").clicked() {
            s.link.send(Cmd::SyncPresets);
        }
        let can = s.can_edit();
        let store = ui.add_enabled(can.is_ok(), egui::Button::new("Store current pose").small());
        let store = match &can {
            Err(why) => store.on_disabled_hover_text(why.clone()),
            Ok(()) => store.on_hover_text("Save the pose in the monitor as a new preset"),
        };
        if store.clicked() {
            let (pose, _) = s.monitor_pose();
            let name = format!("Preset {}", s.presets.len() + 1);
            s.link.send(Cmd::StorePreset(ptz_core::api::StorePreset { name, pose: Some(pose) }));
        }
    });
    ui.add_space(2.0);
    if s.presets.is_empty() {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            note(ui, "No presets yet.");
            note(ui, "Sync from rig reads them from the camera.");
        });
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(2.0);
        let presets = s.presets.clone();
        s.probe_presets.clear();
        for preset in &presets {
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                let id = egui::Id::new(("preset_drag", &preset.id));
                let inner = ui.dnd_drag_source(id, PresetDrag(preset.clone()), |ui| preset_card(ui, preset, false));
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
                    for p in s.presets.clone() {
                        let on = matches!(from, FromPose::Pinned(r) if r.preset_id == p.id);
                        if ui.selectable_label(on, &p.name).clicked() {
                            s.edit(|song, project| edit::set_from(song, project, id, FromPose::Pinned(PresetRef::of(&p))));
                        }
                    }
                });
                ui.end_row();

                ui.label("To");
                egui::ComboBox::from_id_salt("to").selected_text(to.name.clone()).width(190.0).show_ui(ui, |ui| {
                    for p in s.presets.clone() {
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
            ui.label("File timecode");
            match (&info.start_tc, &info.start_tc_source) {
                (Some(tc), Some(src)) => ui.label(format!("{tc}  ({src})")),
                (Some(tc), None) => ui.label(tc),
                _ => ui.label(RichText::new("none found").color(MUTED)),
            };
            ui.end_row();
        }
    });
    let mut muted = clip.muted;
    if ui.checkbox(&mut muted, "Mute in Program").changed() {
        s.edit(|song, project| edit::set_media_muted(song, project, id, muted));
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

fn log_line(kind: &str, data: &serde_json::Value) -> String {
    let text = |k: &str| data.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
    match kind {
        "transport" => {
            let action = text("action");
            let by = text("by");
            let frame = data.get("frame").and_then(|v| v.as_i64()).map(|f| format!(" at frame {f}")).unwrap_or_default();
            format!("{action}{frame}  ({by})")
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
                        "estop" | "fault" => RED,
                        "limit" | "lease" => AMBER,
                        "transport" => OFF_WHITE,
                        _ => BLUE,
                    };
                    ui.label(RichText::new(format!("{:<10}", r.kind)).monospace().color(color));
                    ui.label(RichText::new(log_line(&r.kind, &r.data)).monospace());
                });
            }
        },
    );
}
