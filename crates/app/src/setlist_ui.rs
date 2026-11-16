//! The set list tab: the songs of the show in running order, one click from each other.

use crate::panels::note;
use crate::session::Session;
use crate::theme::*;
use egui::{RichText, Ui};
use ptz_core::api::SongSummary;

/// A button that says why when it cannot be pressed.
fn gated(ui: &mut Ui, label: &str, can: &Result<(), String>, tip: &str) -> bool {
    let r = ui.add_enabled(can.is_ok(), egui::Button::new(label));
    match can {
        Err(why) => r.on_disabled_hover_text(why.clone()).clicked(),
        Ok(()) => r.on_hover_text(tip).clicked(),
    }
}

pub fn show(ui: &mut Ui, s: &mut Session) {
    let loaded = s.status.song.as_ref().map(|x| x.id.clone());
    let can_change = s.can_change_song();
    let can_edit =
        s.can_edit().map_err(|why| if why.starts_with("Disarm") { "Disarm to change the set list".to_string() } else { why });
    let mut project = s.project.clone();
    let mut load = None::<String>;

    // Arranging is a mode of the tab, so that in a show a row holds nothing but the song to load.
    let arrange_id = ui.id().with("arrange_set_list");
    let mut arranging = ui.data(|d| d.get_temp::<bool>(arrange_id)).unwrap_or(false);

    toolbar(ui, |ui| {
        for (label, by, tip) in [
            ("Previous", -1, "Load the song before this one in the set list  ( Page Up )"),
            ("Next", 1, "Load the next song in the set list  ( Page Down )"),
        ] {
            let to = project.set_neighbour(loaded.as_deref(), by).map(str::to_string);
            let can = match (&to, &can_change) {
                (_, Err(why)) => Err(why.clone()),
                (None, _) if project.set_list.is_empty() => Err("The set list is empty".to_string()),
                (None, _) => Err("This is the end of the set list".to_string()),
                (Some(id), Ok(())) => s.songs.iter().find(|x| &x.id == id).map(|x| s.can_load(x)).unwrap_or(Ok(())),
            };
            if gated(ui, label, &can, tip) {
                load = to;
            }
        }
        if ui
            .add(egui::Button::selectable(arranging, "Arrange"))
            .on_hover_text("Add songs to the set list, put them in order, or take them out")
            .clicked()
        {
            arranging = !arranging;
            ui.data_mut(|d| d.insert_temp(arrange_id, arranging));
        }
        let follow = ui.add_enabled(can_edit.is_ok(), egui::Checkbox::new(&mut project.follow_code, "Follow timecode"));
        match &can_edit {
            Err(why) => follow.on_disabled_hover_text(why.clone()),
            Ok(()) => follow.on_hover_text(
                "Locked to LTC, MTC or Art-Net with Play pressed, load whichever song the incoming code is in. Armed, the rig travels to it.",
            ),
        };
    });

    // Each song with its place in the list. A name the app has no song for yet is not shown.
    let listed: Vec<(usize, SongSummary)> = project
        .set_list
        .iter()
        .enumerate()
        .filter_map(|(at, id)| s.songs.iter().find(|x| &x.id == id).map(|x| (at, x.clone())))
        .collect();
    let last = project.set_list.len().saturating_sub(1);
    let others: Vec<SongSummary> = s.songs.iter().filter(|x| project.set_position(&x.id).is_none()).cloned().collect();
    // The rows start at the panel's margin, like the tools above them.
    let list = |ui: &mut Ui| {
        if listed.is_empty() {
            note(
                ui,
                if arranging {
                    "The set list is empty. Add the show's songs below, in running order."
                } else {
                    "The set list is empty. Arrange adds the show's songs to it."
                },
            );
        }
        egui::Grid::new("set_list").striped(true).spacing([PAD, U]).show(ui, |ui| {
            for (at, song) in listed.iter().map(|(at, song)| (*at, song)) {
                let open = loaded.as_deref() == Some(song.id.as_str());
                ui.label(RichText::new(format!("{:\u{2007}>2}", at + 1)).monospace().color(MUTED));
                if arranging {
                    // In front of the name, so a long name cannot push them out of the panel.
                    ui.horizontal(|ui| {
                        let up = if at == 0 { Err("Already first".to_string()) } else { can_edit.clone() };
                        if gated(ui, "Up", &up, "Move earlier in the set list") {
                            project.set_list.swap(at, at - 1);
                        }
                        let down = if at == last { Err("Already last".to_string()) } else { can_edit.clone() };
                        if gated(ui, "Down", &down, "Move later in the set list") {
                            project.set_list.swap(at, at + 1);
                        }
                        if gated(ui, "Remove", &can_edit, "Take out of the set list. The song itself is kept") {
                            project.set_list.retain(|id| id != &song.id);
                        }
                    });
                }
                let can = if open { Err("This song is loaded".to_string()) } else { s.can_load(song) };
                // The loaded song is the row that is switched on; green is kept for output that reaches the rig.
                let name = RichText::new(&song.name).font(bold(BODY));
                let row = ui.add_enabled(open || can.is_ok(), egui::Button::selectable(open, name));
                let row = match &can {
                    Err(why) if !open => row.on_disabled_hover_text(why.clone()),
                    _ => row.on_hover_text("Load this song"),
                };
                if row.clicked() && !open {
                    load = Some(song.id.clone());
                }
                if !arranging {
                    ui.label(RichText::new(&song.start_tc).monospace());
                    ui.label(match (song.revision, song.dirty) {
                        (_, true) => RichText::new("unsaved").color(AMBER),
                        (Some(n), false) => RichText::new(format!("revision {n}")).color(MUTED),
                        (None, false) => RichText::new("draft").color(AMBER),
                    });
                }
                ui.end_row();
            }
        });
        if arranging && !others.is_empty() {
            section(ui, "Not in the set list");
            egui::Grid::new("set_list_others").spacing([PAD, U]).show(ui, |ui| {
                for song in &others {
                    if gated(ui, "Add", &can_edit, "Add to the end of the set list") {
                        project.set_list.push(song.id.clone());
                    }
                    ui.label(&song.name);
                    ui.label(RichText::new(&song.start_tc).monospace().color(MUTED));
                    ui.end_row();
                }
            });
        }
    };
    egui::ScrollArea::both()
        .auto_shrink([false, false])
        .show(ui, |ui| egui::Frame::new().inner_margin(panel_margin()).show(ui, list));

    // A list that names a song the server no longer has is sent as it stands; the server drops the name.
    s.put_project(project);
    if let Some(id) = load {
        s.load_song(&id);
    }
}
