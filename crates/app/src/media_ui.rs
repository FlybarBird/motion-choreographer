//! Media browser, output meters and the reference monitor.

use crate::link::Cmd;
use crate::panels::note;
use crate::session::Session;
use crate::theme::*;
use crate::timeline_ui::MediaDrag;
use egui::{vec2, Align2, FontId, Rect, RichText, Sense, Stroke, StrokeKind, Ui};
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

fn media_card(ui: &mut Ui, info: &MediaInfo) -> egui::Response {
    let width = (ui.available_width() - 8.0).max(120.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 44.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 5.0, mix(CHARCOAL, OFF_WHITE, 0.06));
    p.rect_stroke(rect, 5.0, Stroke::new(1.0, mix(CHARCOAL, OFF_WHITE, 0.16)), StrokeKind::Inside);
    let badge = Rect::from_min_size(rect.left_top() + vec2(6.0, 6.0), vec2(32.0, 32.0));
    let (tag, color) = if info.kind == MediaKind::Video { ("VID", BLUE) } else { ("AUD", GREEN) };
    p.rect_filled(badge, 4.0, with_alpha(color, 40));
    p.text(badge.center(), Align2::CENTER_CENTER, tag, FontId::proportional(10.0), color);
    let text = p.with_clip_rect(rect.shrink(4.0));
    text.text(rect.left_top() + vec2(46.0, 6.0), Align2::LEFT_TOP, &info.name, FontId::proportional(12.5), OFF_WHITE);
    let mut detail = format!("{}   {}   {} Hz", duration_label(info.duration_s), channels_label(info.channels), info.sample_rate);
    if let Some(tc) = &info.start_tc {
        detail.push_str(&format!("   starts {tc}"));
    }
    text.text(rect.left_top() + vec2(46.0, 25.0), Align2::LEFT_TOP, detail, FontId::proportional(10.5), MUTED);
    resp
}

pub fn media(ui: &mut Ui, s: &mut Session) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(6.0);
        let can = s.can_edit();
        let import = ui.add_enabled(can.is_ok(), egui::Button::new("Import...").small());
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
    ui.add_space(2.0);
    if s.media.is_empty() {
        ui.add_space(12.0);
        ui.vertical_centered(|ui| {
            note(ui, "No media has been imported.");
            note(ui, "Drop an audio file here to add it.");
        });
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.add_space(2.0);
        let library = s.media.clone();
        s.probe_media.clear();
        for info in &library {
            ui.horizontal(|ui| {
                ui.add_space(6.0);
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
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add_space(8.0);
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
    let (rect, resp) = ui.allocate_exact_size(vec2(64.0 + 8.0 + label_w, 30.0), Sense::hover());
    let p = ui.painter().with_clip_rect(rect);
    for (row, (tag, peak)) in [("L", audio.peak_left), ("R", audio.peak_right)].into_iter().enumerate() {
        let y = rect.top() + 4.0 + row as f32 * 12.0;
        p.text(egui::pos2(rect.left(), y + 5.0), Align2::LEFT_CENTER, tag, FontId::monospace(9.5), MUTED);
        let bar = Rect::from_min_size(egui::pos2(rect.left() + 10.0, y), vec2(54.0, 10.0));
        p.rect_filled(bar, 2.0, mix(CHARCOAL, NAVY, 0.6));
        let lit = meter_fraction(peak);
        if lit > 0.0 {
            let color = if peak >= 0.98 {
                RED
            } else if lit > 0.85 {
                AMBER
            } else {
                GREEN
            };
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * lit, bar.height())), 2.0, color);
        }
    }
    let color = if audio.output.is_some() { OFF_WHITE } else { AMBER };
    p.text(egui::pos2(rect.left() + 72.0, rect.center().y), Align2::LEFT_CENTER, name, FontId::proportional(11.0), color);
    resp.on_hover_text(hover);
}

pub fn reference(ui: &mut Ui, _s: &mut Session) {
    ui.add_space(12.0);
    ui.vertical_centered(|ui| note(ui, "No reference video is on the timeline."));
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
