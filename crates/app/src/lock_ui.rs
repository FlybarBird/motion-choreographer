//! Lock source menu and the transport state chips.

use crate::link::Cmd;
use crate::session::Session;
use crate::theme::*;
use egui::{Color32, RichText, Stroke, Ui};
use ptz_core::api::TransportRequest;
use ptz_core::chase::ChaseState;
use ptz_core::transport::{Action, Output, Run, TimeMode};

/// What a lock source id reads as: `file:<clip>` becomes the clip's name.
pub fn source_name(s: &Session, source: &str) -> String {
    match source {
        "ltc" => "LTC input".into(),
        "mtc" => "MTC input".into(),
        "artnet" => "Art-Net timecode".into(),
        other => {
            let clip = other.strip_prefix("file:").unwrap_or(other);
            s.song.as_ref().and_then(|song| song.media_clip(clip)).map(|c| c.name.clone()).unwrap_or_else(|| other.to_string())
        }
    }
}

pub fn lock_menu(ui: &mut Ui, s: &mut Session) {
    let g = &s.status.guidance;
    let enabled = g.allows(Action::Lock) && s.lost.is_none();
    let reason = g.reason(Action::Lock).map(str::to_string);
    let mut pick: Option<String> = None;
    let resp = ui.add_enabled_ui(enabled, |ui| {
        ui.menu_button(RichText::new("Lock").size(13.5), |ui| {
            ui.label(RichText::new("Follow timecode from").color(MUTED).small());
            for (id, label) in [("ltc", "LTC input"), ("mtc", "MTC input"), ("artnet", "Art-Net timecode")] {
                if ui.button(label).clicked() {
                    pick = Some(id.to_string());
                    ui.close();
                }
            }
            ui.separator();
            ui.label(RichText::new("A file on the timeline").color(MUTED).small());
            let clips: Vec<(String, String, bool)> = s
                .song
                .as_ref()
                .map(|song| {
                    song.media
                        .iter()
                        .map(|c| {
                            let has_tc = s.media.iter().any(|m| m.id == c.media_id && m.start_tc.is_some());
                            (c.id.clone(), c.name.clone(), has_tc)
                        })
                        .collect()
                })
                .unwrap_or_default();
            if clips.is_empty() {
                ui.label(RichText::new("Drop an audio or video file on the timeline first").color(MUTED));
            }
            for (id, name, has_tc) in clips {
                let label = if has_tc { format!("{name}  (its own timecode)") } else { format!("{name}  (by position)") };
                if ui.button(label).clicked() {
                    pick = Some(format!("file:{id}"));
                    ui.close();
                }
            }
        })
        .response
    });
    if let (false, Some(r)) = (enabled, reason) {
        resp.response.on_disabled_hover_text(r);
    }
    if let Some(source) = pick {
        s.link.send(Cmd::Act(Action::Lock, TransportRequest { source: Some(source), ..Default::default() }));
    }
}

fn chip(ui: &mut Ui, text: &str, fill: Color32, ink: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, mix(fill, OFF_WHITE, 0.25)))
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| ui.label(RichText::new(text).color(ink).small().strong()))
        .response
}

/// The five transport fields, each as its own chip.
pub fn state_chips(ui: &mut Ui, s: &Session) {
    let t = s.status.transport;
    match t.output {
        Output::Rehearse => chip(ui, "REHEARSE", LANE, OFF_WHITE).on_hover_text("Nothing is sent to the rig"),
        Output::Positioning => chip(ui, "ARMING", AMBER, NAVY).on_hover_text("The rig is moving to the pose at the playhead"),
        Output::Armed => chip(ui, "ARMED", GREEN, NAVY).on_hover_text("Play moves the rig"),
    };
    match t.time {
        TimeMode::FreeRun => chip(ui, "FREE RUN", LANE, OFF_WHITE).on_hover_text("The server's own clock drives the playhead"),
        _ => {
            let lock = &s.status.lock;
            let name = lock.source.as_deref().map(|src| source_name(s, src)).unwrap_or_else(|| "code".into());
            let (word, fill, ink) = match (lock.chase, t.run) {
                (_, Run::WaitingForCode) => ("WAITING FOR", AMBER, NAVY),
                (ChaseState::Locked, _) => ("LOCKED TO", BLUE, OFF_WHITE),
                (ChaseState::Freewheel, _) => ("FREEWHEEL", AMBER, NAVY),
                _ => ("LOCK:", LANE, OFF_WHITE),
            };
            let r = chip(ui, &format!("{word} {name}"), fill, ink);
            let mut tip = match &lock.last_code {
                Some(code) => format!("Last code {code}"),
                None => "No code has arrived yet".to_string(),
            };
            if let Some(rate) = &lock.announced_rate {
                tip.push_str(&format!(", {rate} fps"));
            }
            if let Some(p) = &lock.problem {
                tip = p.clone();
            }
            r.on_hover_text(tip)
        }
    };
    if t.record {
        chip(ui, "REC", RED, OFF_WHITE).on_hover_text("Play writes the rig's movement onto the record axes");
    }
    if let Some(p) = &s.status.lock.problem {
        ui.label(RichText::new(p).color(AMBER));
    }
    if let Some(f) = &s.status.telemetry.fault {
        ui.label(RichText::new(f).color(RED).strong());
    }
}
