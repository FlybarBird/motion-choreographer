//! Media browser, output meters and the reference monitor.

use crate::link::Cmd;
use crate::panels::note;
use crate::session::Session;
use crate::theme::*;
use crate::timeline_ui::MediaDrag;
use egui::{vec2, Align2, Color32, Rect, RichText, Sense, Stroke, Ui};
use ptz_core::api::{MediaInfo, MediaKind};

pub fn duration_label(seconds: f64) -> String {
    let whole = seconds.floor() as u64;
    format!("{}:{:04.1}", whole / 60, seconds - (whole / 60 * 60) as f64)
}

pub fn channels_label(channels: u32) -> String {
    match channels {
        1 => "mono".into(),
        2 => "stereo".into(),
        n => format!("{n} ch"),
    }
}

/// A file in the library: a row with a hairline under it, a tag in the colour its clips have on
/// the timeline, its name and what it holds.
fn media_card(ui: &mut Ui, info: &MediaInfo) -> egui::Response {
    let width = ui.available_width().max(120.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 11.0 * U), Sense::hover());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, 0.0, CONTROL);
    }
    p.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(RULE, LINE));
    let badge = Rect::from_min_size(rect.left_top() + vec2(PAD, 1.5 * U), vec2(8.0 * U, 8.0 * U));
    let (tag, color) = if info.kind == MediaKind::Video { ("VID", VIDEO_CLIP) } else { ("AUD", AUDIO_CLIP) };
    p.rect_filled(badge, RADIUS, mix(PANEL, color, 0.24));
    p.rect_filled(badge.with_max_x(badge.left() + 3.0), 0.0, color);
    p.text(badge.center() + vec2(1.5, 0.0), Align2::CENTER_CENTER, tag, semibold(MICRO), OFF_WHITE);
    let text = p.with_clip_rect(rect.shrink2(vec2(PAD, 0.0)));
    text.text(rect.left_top() + vec2(PAD + 10.0 * U, 1.5 * U), Align2::LEFT_TOP, &info.name, body(), OFF_WHITE);
    let mut detail = format!("{}   {}   {} Hz", duration_label(info.duration_s), channels_label(info.channels), info.sample_rate);
    if let Some(tc) = &info.start_tc {
        detail.push_str(&format!("   starts {tc}"));
    }
    text.text(rect.left_top() + vec2(PAD + 10.0 * U, 6.5 * U), Align2::LEFT_TOP, detail, micro(), MUTED);
    resp
}

pub fn media(ui: &mut Ui, s: &mut Session) {
    toolbar(ui, |ui| {
        let can = s.can_edit();
        let import = ui.add_enabled(can.is_ok(), egui::Button::new("Import..."));
        let import = match &can {
            Err(why) => import.on_disabled_hover_text(why.clone()),
            Ok(()) => import.on_hover_text("Add WAV, AIFF or MP3 audio, or a QuickTime or MP4 video"),
        };
        if import.clicked() {
            let picked = rfd::FileDialog::new()
                .set_title("Import media")
                .add_filter("Audio and video", &["wav", "aif", "aiff", "mp3", "mov", "mp4", "m4v"])
                .pick_files();
            for path in picked.unwrap_or_default() {
                s.link.send(Cmd::Upload(path));
            }
        }
        ui.label(RichText::new("or drop files on the window").color(MUTED).small());
    });
    if s.media.is_empty() {
        empty(ui, |ui| {
            note(ui, "No media has been imported.");
            note(ui, "Drop an audio file here to add it.");
        });
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let library = s.media.clone();
        s.probe_media.clear();
        for info in &library {
            ui.horizontal(|ui| {
                let id = egui::Id::new(("media_drag", &info.id));
                let inner = ui.dnd_drag_source(id, MediaDrag(info.clone()), |ui| media_card(ui, info));
                if !ui.ctx().is_being_dragged(id) {
                    s.probe_media.push((info.id.clone(), inner.response.rect));
                }
                let lane = if info.kind == MediaKind::Video { "Video" } else { "Audio" };
                inner.response.on_hover_text(format!("Drag onto the {lane} lane")).context_menu(|ui| {
                    if ui.button("Delete from library").clicked() {
                        s.link.send(Cmd::DeleteMedia(info.id.clone()));
                        ui.close();
                    }
                });
            });
        }
        ui.add_space(GAP);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            note(ui, "Drag a file onto the timeline. Drag a clip's edges to trim it.");
        });
    });
}

/// Meter position for a peak level: -48 dB at the left, full scale at the right.
pub fn meter_fraction(peak: f32) -> f32 {
    if peak <= 0.0 {
        return 0.0;
    }
    ((20.0 * peak.log10() + 48.0) / 48.0).clamp(0.0, 1.0)
}

/// Left and right output meters with the device name, for the transport bar.
pub fn meters(ui: &mut Ui, s: &Session) {
    let audio = &s.status.audio;
    let name = match &audio.output {
        Some(name) => name.clone(),
        None => "Silent".to_string(),
    };
    let hover = match (&audio.output, &audio.silent_reason) {
        (Some(name), _) => format!("Audio output: {name} at {} Hz", audio.device_rate),
        (None, Some(why)) => why.clone(),
        (None, None) => "No audio output".into(),
    };
    let label_w = 92.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(64.0 + GAP + label_w, CONTROL_LARGE), Sense::hover());
    let p = ui.painter().with_clip_rect(rect);
    for (row, (tag, peak)) in [("L", audio.peak_left), ("R", audio.peak_right)].into_iter().enumerate() {
        let y = rect.top() + U + row as f32 * 3.0 * U;
        p.text(egui::pos2(rect.left(), y + U), Align2::LEFT_CENTER, tag, mono(MICRO), MUTED);
        let bar = Rect::from_min_size(egui::pos2(rect.left() + 10.0, y), vec2(54.0, 2.0 * U));
        p.rect_filled(bar, 0.0, BASE);
        let lit = meter_fraction(peak);
        if lit > 0.0 {
            // Level is plain ink: it turns amber near the top and red at full scale, and green is left to the rig.
            let color = if peak >= 0.98 {
                RED
            } else if lit > 0.85 {
                AMBER
            } else {
                OFF_WHITE
            };
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * lit, bar.height())), 0.0, color);
        }
    }
    let color = if audio.output.is_some() { MUTED } else { AMBER };
    p.text(egui::pos2(rect.left() + 64.0 + GAP, rect.center().y), Align2::LEFT_CENTER, name, small(), color);
    resp.on_hover_text(hover);
}

/// How many frames ahead of the playhead are fetched while playing.
const PREFETCH: i64 = 12;

/// The video clip under `frame`, and which frame of its file that is.
pub fn picture_at(song: &ptz_core::song::Song, frame: i64) -> Option<(&ptz_core::song::MediaClip, i64)> {
    song.media
        .iter()
        .filter(|c| c.track == ptz_core::song::Track::Video)
        .find(|c| (c.start_frame..c.end_frame()).contains(&frame))
        .map(|c| (c, c.in_frame + frame - c.start_frame))
}

fn fit(inside: Rect, aspect: f32) -> Rect {
    let size = if inside.width() / inside.height() > aspect {
        vec2(inside.height() * aspect, inside.height())
    } else {
        vec2(inside.width(), inside.width() / aspect)
    };
    Rect::from_center_size(inside.center(), size)
}

/// The reference monitor: the frame of the reference video under the playhead.
pub fn reference(ui: &mut Ui, s: &mut Session) {
    let Some(song) = s.song.clone() else {
        empty(ui, |ui| note(ui, "No song is open."));
        return;
    };
    if !song.media.iter().any(|c| c.track == ptz_core::song::Track::Video) {
        empty(ui, |ui| {
            note(ui, "No reference video is on the timeline.");
            note(ui, "Import a QuickTime or MP4 file, then drag it from Media onto the Reference video lane.");
        });
        return;
    }
    let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let p = ui.painter_at(area);
    p.rect_filled(area, 0.0, Color32::BLACK);
    let frame = s.playhead().floor() as i64;
    let Some((clip, source)) = picture_at(&song, frame) else {
        s.reference_shown = None;
        p.text(area.center(), Align2::CENTER_CENTER, "No picture at the playhead", body(), MUTED);
        return;
    };
    let info = s.media.iter().find(|m| m.id == clip.media_id).cloned();
    if info.as_ref().is_none_or(|m| m.proxy_frames == 0) {
        p.text(area.center(), Align2::CENTER_CENTER, "This file has no picture", body(), MUTED);
        return;
    }
    let key = crate::frames::key(&clip.media_id, source, false);
    s.frames.want(&s.link, &key);
    if s.is_playing() {
        for ahead in 1..=PREFETCH {
            if source + ahead < clip.out_frame {
                s.frames.want(&s.link, &crate::frames::key(&clip.media_id, source + ahead, false));
            }
        }
    }
    if let Some(texture) = s.frames.texture(ui.ctx(), &key) {
        s.reference_shown = Some((clip.media_id.clone(), texture));
    }
    // Until the frame arrives, the last one shown from this file stays up.
    match s.reference_shown.as_ref().filter(|(media, _)| *media == clip.media_id) {
        Some((_, texture)) => {
            let size = texture.size_vec2();
            let at = fit(area.shrink(2.0), size.x / size.y.max(1.0));
            p.image(texture.id(), at, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            let strip = Rect::from_min_max(egui::pos2(at.left(), at.bottom() - 20.0), at.max);
            p.rect_filled(strip, 0.0, plate_fill());
            p.text(strip.left_center() + vec2(GAP, 0.0), Align2::LEFT_CENTER, &clip.name, small(), OFF_WHITE);
            let rate = s.project.rate;
            let code = info
                .as_ref()
                .and_then(|m| m.start_tc.as_deref())
                .and_then(|tc| ptz_core::time::Timecode::parse_at(tc, rate))
                .map(|start| ptz_core::time::Timecode::from_frames(start.to_frames(rate) + source, rate).to_string())
                .unwrap_or_else(|| format!("frame {source}"));
            p.text(strip.right_center() - vec2(GAP, 0.0), Align2::RIGHT_CENTER, code, mono(SMALL), OFF_WHITE);
        }
        None => {
            p.text(area.center(), Align2::CENTER_CENTER, "Loading the picture", body(), MUTED);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_meter_scale() {
        assert_eq!(duration_label(12.4), "0:12.4");
        assert_eq!(duration_label(185.04), "3:05.0");
        assert_eq!(channels_label(1), "mono");
        assert_eq!(channels_label(6), "6 ch");
        assert_eq!(meter_fraction(0.0), 0.0);
        assert_eq!(meter_fraction(1.0), 1.0);
        assert!((meter_fraction(0.5) - 0.8746).abs() < 0.001);
        assert_eq!(meter_fraction(0.001), 0.0);
    }
}
