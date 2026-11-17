//! Live edit: change the loaded song while the show is armed, after an explicit confirm.

use crate::link::Cmd;
use crate::session::Session;
use crate::theme::*;
use egui::{RichText, Ui};
use ptz_core::transport::Output;

pub fn show(ui: &mut Ui, s: &mut Session) {
    if s.status.transport.output == Output::Rehearse {
        empty(ui, |ui| {
            ui.label(RichText::new("The show is not live.").color(OFF_WHITE));
            note(ui, "Edit the song on the Timeline. This page is for changes after the show is armed.");
        });
        return;
    }
    let Some((id, name)) = s.song.as_ref().map(|song| (song.id.clone(), song.name.clone())) else {
        empty(ui, |ui| note(ui, "No song is open."));
        return;
    };
    if !s.status.live_edit {
        ask(ui, s, &id, &name);
        return;
    }
    let mut leave = false;
    toolbar(ui, |ui| {
        ui.label(RichText::new(format!("Editing {name} live. The rig follows this timeline.")).color(OFF_WHITE));
        if ui.button("Done").on_hover_text("Stop editing this live song").clicked() {
            leave = true;
        }
    });
    if leave {
        // Refusal is back before the next click. The server is told in the same breath.
        s.status.live_edit = false;
        s.link.send(Cmd::CancelLiveEdit(id));
        return;
    }
    s.live_surface = true;
    crate::timeline_ui::show(ui, s);
    s.live_surface = false;
}

fn ask(ui: &mut Ui, s: &mut Session, id: &str, name: &str) {
    empty(ui, |ui| {
        ui.label(RichText::new("You are about to edit the song that is live.").color(OFF_WHITE));
        note(ui, &format!("{name} is on the rig. Confirm to edit it and drop a preset on the timeline. The show stays armed."));
        ui.add_space(U);
        let why = blocked(s);
        let button = ui.add_enabled(why.is_none(), filled("Edit the live song", ACCENT));
        let button = match &why {
            Some(text) => button.on_disabled_hover_text(text.clone()),
            None => button.on_hover_text("Edit this song while the rig follows the timeline"),
        };
        if button.clicked() {
            s.link.send(Cmd::ConfirmLiveEdit(id.to_string()));
        }
    });
}

fn blocked(s: &Session) -> Option<String> {
    if s.lost.is_some() {
        Some("The server is not reachable".into())
    } else if !s.status.you_hold_lease {
        Some(match &s.status.lease {
            Some(lease) => format!("{} holds the edit lease. Take it over to edit", lease.holder),
            None => "Take the edit lease to edit".into(),
        })
    } else {
        None
    }
}
