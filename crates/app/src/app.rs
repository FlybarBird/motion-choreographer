//! The window: connect screen, fixed song header and transport bar, and the dock.

use crate::link::{self, Cmd, Event, Link};
use crate::panels::{self, text_field};
use crate::session::Session;
use crate::theme::*;
use crate::{monitor, timeline_ui};
use egui::{vec2, Align, Color32, Key, Layout, RichText, Stroke, Ui};
use egui_dock::{DockArea, DockState, NodeIndex};
use ptz_core::edit;
use ptz_core::time::Timecode;
use ptz_core::transport::{Action, Output, Run};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Tab {
    Presets,
    Media,
    Program,
    Reference,
    Inspector,
    Rig,
    Timeline,
    Graph,
    Log,
}

impl Tab {
    pub const ALL: [Tab; 9] =
        [Tab::Presets, Tab::Media, Tab::Program, Tab::Reference, Tab::Inspector, Tab::Rig, Tab::Timeline, Tab::Graph, Tab::Log];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Presets => "Presets",
            Tab::Media => "Media",
            Tab::Program => "Program monitor",
            Tab::Reference => "Reference monitor",
            Tab::Inspector => "Inspector",
            Tab::Rig => "Rig controller",
            Tab::Timeline => "Timeline",
            Tab::Graph => "Graph editor",
            Tab::Log => "Log",
        }
    }
}

pub fn default_layout() -> DockState<Tab> {
    let mut state = DockState::new(vec![Tab::Program, Tab::Reference]);
    let surface = state.main_surface_mut();
    let [top, _] = surface.split_below(NodeIndex::root(), 0.5, vec![Tab::Timeline, Tab::Graph, Tab::Log]);
    let [centre, _] = surface.split_left(top, 0.2, vec![Tab::Presets, Tab::Media]);
    let [_, _] = surface.split_right(centre, 0.7, vec![Tab::Inspector, Tab::Rig]);
    state
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub server: String,
    pub station: String,
    pub access_key: String,
    pub layout: Option<DockState<Tab>>,
}

fn settings_path() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("PTZ_APP_DIR") {
        return Some(PathBuf::from(dir).join("settings.json"));
    }
    dirs::config_dir().map(|d| d.join("PTZ Programmer").join("settings.json"))
}

impl Settings {
    pub fn load() -> Settings {
        let mut s: Settings = settings_path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        if s.server.trim().is_empty() {
            s.server = format!("http://127.0.0.1:{}", ptz_core::api::DEFAULT_PORT);
        }
        if s.station.trim().is_empty() {
            s.station = std::env::var("USER").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| "Operator".into());
        }
        s
    }

    pub fn save(&self) {
        let Some(path) = settings_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }
}

/// Arm is never a quiet switch: the rig travels to the pose at the playhead before anything plays.
pub const ARM_LABEL: &str = "Arm (moves rig to playhead)";

#[derive(Default)]
enum Dialog {
    #[default]
    None,
    New {
        name: String,
        start_tc: String,
    },
    Load,
    SaveAs {
        name: String,
    },
    Revert,
    DeleteSong {
        id: String,
        name: String,
    },
}

/// Everything shown once connected. Hosted by the real window and by the UI tests.
pub struct Shell {
    pub session: Session,
    pub dock: DockState<Tab>,
    dialog: Dialog,
    pub layout_changed: bool,
}

impl Shell {
    /// Bring a tab to the front of its group.
    pub fn show_tab(&mut self, tab: Tab) {
        if let Some(path) = self.dock.find_tab(&tab) {
            let _ = self.dock.set_active_tab(path);
        }
    }
}

struct Viewer<'a> {
    session: &'a mut Session,
}

impl egui_dock::TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("dock_tab", *tab))
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        let s = &mut *self.session;
        match tab {
            Tab::Presets => panels::presets(ui, s),
            Tab::Media => crate::media_ui::media(ui, s),
            Tab::Program => monitor::show(ui, s),
            Tab::Reference => crate::media_ui::reference(ui, s),
            Tab::Inspector => panels::inspector(ui, s),
            Tab::Rig => crate::rig_ui::show(ui, s),
            Tab::Timeline => timeline_ui::show(ui, s),
            Tab::Graph => crate::graph::show(ui, s),
            Tab::Log => panels::log(ui, s),
        }
    }

    fn is_closeable(&self, _tab: &Tab) -> bool {
        false
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        [false, matches!(tab, Tab::Timeline)]
    }

    fn allowed_in_windows(&self, _tab: &mut Tab) -> bool {
        false
    }
}

/// A transport button: enabled when the server says so, with the server's reason when not.
fn action_button(
    ui: &mut Ui,
    s: &mut Session,
    action: Action,
    label: &str,
    fill: Option<(Color32, Color32)>,
    min_w: f32,
) -> bool {
    let g = &s.status.guidance;
    let enabled = g.allows(action) && s.lost.is_none();
    let text = RichText::new(label).size(13.5);
    let button = match fill {
        Some((bg, ink)) if enabled => egui::Button::new(text.color(ink).strong()).fill(bg),
        _ => egui::Button::new(text),
    };
    let resp = ui.add_enabled(enabled, button.min_size(vec2(min_w, 30.0)));
    let reason = if s.lost.is_some() { Some("The server is not reachable") } else { g.reason(action) };
    let resp = match reason {
        Some(r) if !enabled => resp.on_disabled_hover_text(r),
        _ => resp,
    };
    if resp.clicked() {
        s.act(action);
        return true;
    }
    false
}

impl Shell {
    pub fn new(session: Session, layout: Option<DockState<Tab>>) -> Shell {
        let dock = layout.filter(layout_is_complete).unwrap_or_else(default_layout);
        Shell { session, dock, dialog: Dialog::None, layout_changed: false }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        self.session.poll();
        self.shortcuts(ui);
        self.dropped_files(ui);

        egui::Panel::top("song_header")
            .frame(egui::Frame::new().fill(NAVY).inner_margin(egui::Margin::symmetric(10, 7)))
            .show(ui, |ui| self.header(ui));
        egui::Panel::top("transport_bar")
            .frame(egui::Frame::new().fill(PANEL_DARK).inner_margin(egui::Margin::symmetric(10, 7)))
            .show(ui, |ui| self.transport(ui));
        egui::Panel::top("camera_bar")
            .frame(egui::Frame::new().fill(CHARCOAL).inner_margin(egui::Margin::symmetric(10, 4)))
            .show(ui, |ui| crate::camera_ui::bar(ui, &mut self.session));
        if let Some(lost) = self.session.lost.clone() {
            egui::Panel::top("lost_banner")
                .frame(egui::Frame::new().fill(RED).inner_margin(egui::Margin::symmetric(10, 5)))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!("{lost}. The show keeps running on the server; this window is not in control."))
                            .strong(),
                    );
                });
        }

        let before = serde_json::to_string(&self.dock).unwrap_or_default();
        self.session.rig.keyboard = false;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(NAVY)).show(ui, |ui| {
            let style = dock_style(ui.style());
            let mut viewer = Viewer { session: &mut self.session };
            DockArea::new(&mut self.dock)
                .style(style)
                .show_close_buttons(false)
                .show_add_buttons(false)
                .show_leaf_close_all_buttons(false)
                .show_leaf_collapse_buttons(false)
                .show_inside(ui, &mut viewer);
        });
        if serde_json::to_string(&self.dock).unwrap_or_default() != before {
            self.layout_changed = true;
        }

        self.dialogs(ui.ctx());
        self.toasts(ui.ctx());
    }

    fn header(&mut self, ui: &mut Ui) {
        let s = &mut self.session;
        let can_edit = s.can_edit();
        let song_id = s.song.as_ref().map(|x| x.id.clone());
        let st = s.status.song.clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new("PTZ Programmer").strong().color(BLUE));
            ui.separator();
            let parked = s.status.transport.run == Run::Stopped && s.status.transport.output == Output::Rehearse;
            let need_lease = can_edit.clone().err().filter(|_| !s.status.you_hold_lease || s.lost.is_some());
            let gate = |ui: &mut Ui, enabled: bool, label: &str, why: Option<String>, tip: &str| -> bool {
                let r = ui.add_enabled(enabled, egui::Button::new(label));
                let r = match (&why, enabled) {
                    (Some(w), false) => r.on_disabled_hover_text(w.clone()),
                    _ => r.on_hover_text(tip),
                };
                r.clicked()
            };
            let park_why = need_lease.clone().or_else(|| (!parked).then(|| "Stop and disarm to change songs".to_string()));
            if gate(ui, park_why.is_none(), "New", park_why.clone(), "Start a new song") {
                let n = s.songs.len() + 1;
                self.dialog = Dialog::New { name: format!("Song {n}"), start_tc: next_start(s) };
            }
            if gate(ui, park_why.is_none(), "Load", park_why.clone(), "Open another song") {
                s.link.send(Cmd::RefreshSongs);
                self.dialog = Dialog::Load;
            }
            let dirty = st.as_ref().is_some_and(|x| x.dirty);
            let has = song_id.is_some();
            let save_why = need_lease
                .clone()
                .or_else(|| (!has).then(|| "No song is open".to_string()))
                .or_else(|| (!dirty).then(|| "Nothing to save".to_string()));
            if gate(ui, save_why.is_none(), "Save", save_why, "Save the draft  ( Cmd-S )") {
                if let Some(id) = &song_id {
                    s.link.send(Cmd::Save(id.clone()));
                }
            }
            let any_why = need_lease.clone().or_else(|| (!has).then(|| "No song is open".to_string()));
            if gate(ui, any_why.is_none(), "Save As", any_why.clone(), "Save a copy under a new name and open it") {
                let name = s.song.as_ref().map(|x| format!("{} copy", x.name)).unwrap_or_default();
                self.dialog = Dialog::SaveAs { name };
            }
            let revert_why = can_edit
                .clone()
                .err()
                .or_else(|| (!has).then(|| "No song is open".to_string()))
                .or_else(|| (!dirty).then(|| "No changes since the last Save".to_string()));
            if gate(ui, revert_why.is_none(), "Revert", revert_why, "Go back to the last Save") {
                self.dialog = Dialog::Revert;
            }
            let clean = st.as_ref().is_some_and(|x| x.committed_clean);
            let commit_why = can_edit
                .clone()
                .err()
                .or_else(|| (!has).then(|| "No song is open".to_string()))
                .or_else(|| dirty.then(|| "Save the song before committing".to_string()))
                .or_else(|| clean.then(|| "This is already the committed revision".to_string()));
            if gate(
                ui,
                commit_why.is_none(),
                "Commit",
                commit_why,
                "Freeze this draft as the next revision. Only committed revisions can be armed",
            ) {
                if let Some(id) = &song_id {
                    s.link.send(Cmd::Commit(id.clone()));
                }
            }
            ui.separator();

            match &s.song {
                Some(song) => {
                    let (name, start) = (song.name.clone(), song.start_tc.clone());
                    ui.add_enabled_ui(can_edit.is_ok(), |ui| {
                        if let Some(new) = text_field(ui, "hdr_song_name", &name, 170.0) {
                            if !new.trim().is_empty() {
                                s.edit(|song, project| edit::set_song_name(song, project, new.trim()));
                            }
                        }
                        ui.label(RichText::new("starts").color(MUTED));
                        if let Some(tc) = text_field(ui, "hdr_song_start", &start, 92.0) {
                            match Timecode::parse_at(&tc, s.project.rate) {
                                Some(_) => {
                                    s.edit(|song, project| edit::set_song_start(song, project, &tc));
                                }
                                None => s.toast(
                                    format!("{tc} is not a timecode at {} fps. Use HH:MM:SS:FF", s.project.rate.label()),
                                    true,
                                ),
                            }
                        }
                    });
                    if let Some(st) = &st {
                        let rev = match st.revision {
                            Some(n) if st.committed_clean => format!("revision {n}"),
                            Some(n) => format!("revision {n} + edits"),
                            None => "never committed".to_string(),
                        };
                        ui.label(RichText::new(rev).color(if st.committed_clean { GREEN } else { MUTED }));
                        if st.dirty {
                            ui.label(RichText::new("unsaved").color(AMBER)).on_hover_text(
                                "The draft has changes since the last Save. They are kept on the server either way",
                            );
                        }
                    }
                    let n = s.compiled.issues.iter().filter_map(|i| i.clip_id()).collect::<std::collections::BTreeSet<_>>().len()
                        + s.compiled.issues.iter().filter(|i| i.clip_id().is_none()).count().min(1);
                    if n > 0 {
                        let blocking = s.song.as_ref().map(|song| s.compiled.blocking_count(song)).unwrap_or(0);
                        let text = format!("{n} issue{}", if n == 1 { "" } else { "s" });
                        let r = ui.add(egui::Label::new(RichText::new(text).color(AMBER).strong()).sense(egui::Sense::click()));
                        let tip = if blocking > 0 {
                            "Arm is blocked until these are fixed or accepted. Click to go to the first"
                        } else {
                            "All accepted. Click to go to the first"
                        };
                        if r.on_hover_text(tip).clicked() {
                            if let Some(first) = s.compiled.issues.first().cloned() {
                                s.seek(first.frame());
                                s.timeline.selection.clear();
                                if let Some(id) = first.clip_id() {
                                    s.timeline.selection.insert(id.to_string());
                                }
                            }
                        }
                    }
                }
                None => {
                    ui.label(RichText::new("No song is open. New starts one; Load opens one.").color(MUTED));
                }
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.menu_button("View", |ui| {
                    if ui.button("Reset layout").clicked() {
                        self.dock = default_layout();
                        self.layout_changed = true;
                        ui.close();
                    }
                    if ui.button("Fit timeline").clicked() {
                        s.fit_requested = true;
                        ui.close();
                    }
                });
                if s.status.you_hold_lease {
                    if ui.small_button("Release").on_hover_text("Give up the edit lease so another station can take it").clicked()
                    {
                        s.link.send(Cmd::ReleaseLease);
                    }
                    ui.label(RichText::new("You hold the edit lease").color(GREEN));
                } else {
                    let (label, text, force) = match &s.status.lease {
                        Some(l) => ("Take over", format!("Watching. {} holds the edit lease", l.holder), true),
                        None => ("Take the lease", "Watching. Nobody holds the edit lease".to_string(), false),
                    };
                    if ui.button(label).clicked() {
                        s.link.send(Cmd::TakeLease { name: s.station.clone(), force });
                    }
                    ui.label(RichText::new(text).color(AMBER));
                }
                ui.label(RichText::new(&s.link.base).color(MUTED).small());
            });
        });
    }

    fn transport(&mut self, ui: &mut Ui) {
        let s = &mut self.session;
        let t = s.status.transport;
        ui.horizontal(|ui| {
            // E-Stop is always first to be laid out on the right, and always enabled.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if t.estop {
                    action_button(ui, s, Action::ResetEstop, "Reset E-Stop", Some((AMBER, NAVY)), 120.0);
                    ui.label(RichText::new("E-STOP LATCHED").color(RED).strong());
                } else {
                    let b = egui::Button::new(RichText::new("E-STOP").color(OFF_WHITE).strong().size(14.0))
                        .fill(RED)
                        .stroke(Stroke::new(1.5, OFF_WHITE))
                        .min_size(vec2(96.0, 32.0));
                    if ui.add(b).on_hover_text("Stop all motion now. Works from any station, with or without the lease").clicked()
                    {
                        s.act(Action::Estop);
                    }
                }
                ui.separator();
                crate::media_ui::meters(ui, s);
                ui.separator();

                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let frame = s.playhead().floor() as i64;
                    ui.label(RichText::new(s.timecode_at(frame)).monospace().size(22.0).color(if s.status.hardware_live {
                        GREEN
                    } else {
                        OFF_WHITE
                    }));
                    ui.add_space(6.0);

                    // The main button: whatever the server says is the next step.
                    let g = s.status.guidance.clone();
                    let primary = g.primary.filter(|a| *a != Action::Estop && *a != Action::ResetEstop);
                    let live = t.output != Output::Rehearse;
                    match primary {
                        Some(action) => {
                            let fill = match action {
                                Action::Play if live => (GREEN, NAVY),
                                Action::Stop => (OFF_WHITE, NAVY),
                                _ => (BLUE, OFF_WHITE),
                            };
                            action_button(ui, s, action, &g.primary_label, Some(fill), 118.0);
                        }
                        None => {
                            ui.add_enabled(
                                false,
                                egui::Button::new(RichText::new("Rehearse").size(13.5)).min_size(vec2(118.0, 30.0)),
                            )
                            .on_disabled_hover_text(g.hint.clone());
                        }
                    }
                    if primary != Some(Action::Stop) {
                        action_button(ui, s, Action::Stop, "Stop", None, 0.0);
                    }
                    action_button(ui, s, Action::Cue, "Cue", None, 0.0);
                    ui.separator();

                    if t.locked() {
                        action_button(ui, s, Action::Unlock, "Unlock", None, 0.0);
                    } else {
                        crate::lock_ui::lock_menu(ui, s);
                    }
                    if t.output == Output::Rehearse {
                        action_button(ui, s, Action::Arm, ARM_LABEL, Some((GREEN, NAVY)), 0.0);
                    } else {
                        action_button(ui, s, Action::Disarm, "Disarm", Some((AMBER, NAVY)), 0.0);
                    }
                    if t.record {
                        action_button(ui, s, Action::RecordOff, "Record off", Some((RED, OFF_WHITE)), 0.0);
                    } else {
                        action_button(ui, s, Action::RecordOn, "Record", None, 0.0);
                    }
                    ui.separator();
                    crate::lock_ui::state_chips(ui, s);
                    ui.add(egui::Label::new(RichText::new(&g.hint).color(MUTED)).truncate());
                });
            });
        });
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let s = &mut self.session;
        if !matches!(self.dialog, Dialog::None) {
            return;
        }
        let shift = ctx.input(|i| i.modifiers.shift);
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        // The modifiers that came with the key, not those held when the frame ends:
        // a quick Cmd-Z can be pressed and let go inside one frame.
        let chord = |k: Key| {
            ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, modifiers, .. } if *key == k => Some(*modifiers),
                    _ => None,
                })
            })
        };
        let with_cmd = |k: Key| chord(k).is_some_and(|m| m.command);
        let alone = |k: Key| chord(k).is_some_and(|m| !m.command);
        if let Some(m) = chord(Key::Z).filter(|m| m.command) {
            if m.shift {
                s.redo();
            } else {
                s.undo();
            }
        }
        if with_cmd(Key::S) {
            if let Some(song) = &s.song {
                s.link.send(Cmd::Save(song.id.clone()));
            }
        }
        if ctx.text_edit_focused() {
            return;
        }
        if with_cmd(Key::D) {
            timeline_ui::duplicate_selection(s);
        }
        crate::camera_ui::shortcuts(&ctx, s);
        if pressed(Key::Escape) {
            s.timeline.cancel();
        }
        if pressed(Key::Space) {
            let g = &s.status.guidance;
            if let Some(action) = g.primary.filter(|a| *a != Action::Estop) {
                if g.allows(action) {
                    s.act(action);
                }
            }
        }
        if pressed(Key::Home) && s.status.guidance.allows(Action::Cue) {
            s.act(Action::Cue);
        }
        let step = if shift { 10 } else { 1 };
        // While the rig controller has the keyboard, the arrows drive the rig.
        let can_seek = s.status.guidance.allows(Action::Seek) && !s.rig.keyboard;
        if pressed(Key::ArrowLeft) && can_seek {
            let f = s.playhead().round() as i64 - step;
            s.seek(f);
        }
        if pressed(Key::ArrowRight) && can_seek {
            let f = s.playhead().round() as i64 + step;
            s.seek(f);
        }
        if pressed(Key::End) && can_seek {
            if let Some(len) = s.song.as_ref().map(|x| x.length_frames) {
                s.seek(len);
            }
        }
        if pressed(Key::Backslash) {
            s.fit_requested = true;
        }
        if alone(Key::M) {
            timeline_ui::add_marker(s);
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            timeline_ui::delete_selection(s);
        }
        if alone(Key::K) {
            crate::graph::add_key_at_playhead(s);
        }
    }

    fn dropped_files(&mut self, ui: &mut Ui) {
        let files: Vec<PathBuf> = ui.ctx().input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        for path in files {
            self.session.link.send(Cmd::Upload(path));
        }
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let s = &mut self.session;
        let mut close = false;
        let mut next: Option<Dialog> = None;
        match &mut self.dialog {
            Dialog::None => {}
            Dialog::New { name, start_tc } => {
                egui::Modal::new(egui::Id::new("dlg_new")).show(ctx, |ui| {
                    ui.set_width(340.0);
                    ui.heading("New song");
                    ui.add_space(6.0);
                    egui::Grid::new("new_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        ui.label("Name");
                        ui.add(egui::TextEdit::singleline(name).desired_width(220.0));
                        ui.end_row();
                        ui.label("Starts at");
                        ui.add(egui::TextEdit::singleline(start_tc).desired_width(120.0));
                        ui.end_row();
                    });
                    let valid = Timecode::parse_at(start_tc, s.project.rate).is_some();
                    if !valid {
                        ui.label(RichText::new(format!("Use HH:MM:SS:FF at {} fps", s.project.rate.label())).color(AMBER));
                    } else {
                        ui.label(RichText::new("The timecode at which this song's first frame plays.").color(MUTED));
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let ok = valid && !name.trim().is_empty();
                        if ui.add_enabled(ok, egui::Button::new("Create").fill(BLUE)).clicked() {
                            s.link.send(Cmd::NewSong { name: name.trim().to_string(), start_tc: Some(start_tc.clone()) });
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::Load => {
                egui::Modal::new(egui::Id::new("dlg_load")).show(ctx, |ui| {
                    ui.set_width(520.0);
                    ui.heading("Load a song");
                    ui.add_space(6.0);
                    if s.songs.is_empty() {
                        ui.label(RichText::new("There are no songs on this server yet. Use New to start one.").color(MUTED));
                    }
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        egui::Grid::new("load_grid").num_columns(5).striped(true).spacing([14.0, 8.0]).show(ui, |ui| {
                            for song in s.songs.clone() {
                                ui.label(RichText::new(&song.name).strong());
                                ui.label(RichText::new(&song.start_tc).monospace());
                                ui.label(match song.revision {
                                    Some(n) => format!("revision {n}"),
                                    None => "draft".into(),
                                });
                                if song.loaded {
                                    ui.label(RichText::new("open").color(GREEN));
                                } else if song.dirty {
                                    ui.label(RichText::new("unsaved").color(AMBER));
                                } else {
                                    ui.label("");
                                }
                                ui.horizontal(|ui| {
                                    if ui.add_enabled(!song.loaded, egui::Button::new("Load")).clicked() {
                                        s.link.send(Cmd::LoadSong(song.id.clone()));
                                        close = true;
                                    }
                                    if ui.add_enabled(!song.loaded, egui::Button::new("Delete")).clicked() {
                                        next = Some(Dialog::DeleteSong { id: song.id.clone(), name: song.name.clone() });
                                    }
                                });
                                ui.end_row();
                            }
                        });
                    });
                    ui.add_space(8.0);
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            }
            Dialog::SaveAs { name } => {
                egui::Modal::new(egui::Id::new("dlg_saveas")).show(ctx, |ui| {
                    ui.set_width(340.0);
                    ui.heading("Save As");
                    ui.label(RichText::new("Makes a copy with its own revisions and opens it.").color(MUTED));
                    ui.add_space(6.0);
                    ui.add(egui::TextEdit::singleline(name).desired_width(300.0));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!name.trim().is_empty(), egui::Button::new("Save copy").fill(BLUE)).clicked() {
                            if let Some(song) = &s.song {
                                s.link.send(Cmd::CopySong { id: song.id.clone(), name: name.trim().to_string() });
                            }
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::Revert => {
                egui::Modal::new(egui::Id::new("dlg_revert")).show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.heading("Revert to the last Save?");
                    ui.label("Every change since the last Save is dropped. This cannot be undone.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Revert").fill(AMBER)).clicked() {
                            if let Some(song) = &s.song {
                                s.link.send(Cmd::Revert(song.id.clone()));
                            }
                            close = true;
                        }
                        if ui.button("Keep my changes").clicked() {
                            close = true;
                        }
                    });
                });
            }
            Dialog::DeleteSong { id, name } => {
                egui::Modal::new(egui::Id::new("dlg_delete")).show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.heading(format!("Delete {name}?"));
                    ui.label("The draft and every committed revision are removed from the server.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Delete").fill(RED)).clicked() {
                            s.link.send(Cmd::DeleteSong(id.clone()));
                            next = Some(Dialog::Load);
                        }
                        if ui.button("Cancel").clicked() {
                            next = Some(Dialog::Load);
                        }
                    });
                });
            }
        }
        if s.confirm_sync {
            egui::Modal::new(egui::Id::new("dlg_sync")).show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading("Read presets from the camera?");
                ui.label("The camera is recalled to each of its presets in turn, so that its pose and picture can be read. It will move, at its own speed, through every preset it holds.");
                ui.label(RichText::new("The pedestal does not move. E-Stop stops the camera and abandons the sync.").color(MUTED));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("Move the camera and sync").fill(AMBER)).clicked() {
                        s.link.send(Cmd::SyncPresets(s.cam_id().to_string()));
                        s.confirm_sync = false;
                    }
                    if ui.button("Cancel").clicked() {
                        s.confirm_sync = false;
                    }
                });
            });
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                s.confirm_sync = false;
            }
        }
        if let Some(d) = next {
            self.dialog = d;
        } else if close || (!matches!(self.dialog, Dialog::None) && ctx.input(|i| i.key_pressed(Key::Escape))) {
            self.dialog = Dialog::None;
        }

        // Marker rename, opened by a double-click on the timeline.
        let rename_id = egui::Id::new("rename_marker");
        if let Some((id, mut name)) = ctx.data(|d| d.get_temp::<(String, String)>(rename_id)) {
            let mut done = false;
            let mut apply = false;
            egui::Modal::new(egui::Id::new("dlg_marker")).show(ctx, |ui| {
                ui.set_width(300.0);
                ui.heading("Rename marker");
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(260.0));
                r.request_focus();
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    apply = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new("Rename").fill(BLUE)).clicked() {
                        apply = true;
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            });
            if apply {
                let n = name.trim().to_string();
                if !n.is_empty() {
                    s.edit(|song, project| edit::rename_marker(song, project, &id, &n));
                }
                done = true;
            }
            if done || ctx.input(|i| i.key_pressed(Key::Escape)) {
                ctx.data_mut(|d| d.remove::<(String, String)>(rename_id));
            } else {
                ctx.data_mut(|d| d.insert_temp(rename_id, (id, name)));
            }
        }

        self.stamped_drop_dialog(ctx);
    }

    /// A stamped file was dropped on a song its timecode does not fall inside.
    fn stamped_drop_dialog(&mut self, ctx: &egui::Context) {
        let s = &mut self.session;
        let (Some(drop), Some(song)) = (s.stamped_drop.clone(), s.song.clone()) else {
            s.stamped_drop = None;
            return;
        };
        let info = &drop.media;
        let file_start = info.start_tc.clone().unwrap_or_default();
        let track = if info.kind == ptz_core::api::MediaKind::Video {
            ptz_core::song::Track::Video
        } else {
            ptz_core::song::Track::Audio
        };
        let mut choice: Option<bool> = None;
        let mut close = false;
        egui::Modal::new(egui::Id::new("dlg_stamped_drop")).show(ctx, |ui| {
            ui.set_width(430.0);
            ui.heading(format!("{} starts at {file_start}", info.name));
            ui.add_space(4.0);
            ui.label(format!(
                "That is outside {}, which runs from {} to {}.",
                song.name,
                s.timecode_at(0),
                s.timecode_at(song.length_frames)
            ));
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Setting the song start puts the file on its own timecode at the first frame. \
                     Clips already on the timeline keep their places and take new timecodes.",
                )
                .color(MUTED),
            );
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(format!("Set the song start to {file_start}")).fill(BLUE)).clicked() {
                    choice = Some(true);
                }
                if ui.button("Place it where I dropped it").clicked() {
                    choice = Some(false);
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if let Some(restart) = choice {
            let at = if restart { 0 } else { drop.at_frame };
            let placed = s.edit(|song, project| {
                let base = if restart { edit::set_song_start(song, project, &file_start)? } else { song.clone() };
                edit::add_media_clip(&base, project, &info.id, &info.name, track, info.duration_frames, at).map(|(song, _)| song)
            });
            if placed && !restart {
                s.toast(format!("{} is off its timecode. The Inspector can move it back", info.name), false);
            }
            close = true;
        }
        if close || ctx.input(|i| i.key_pressed(Key::Escape)) {
            s.stamped_drop = None;
        }
    }

    fn toasts(&mut self, ctx: &egui::Context) {
        if self.session.toasts.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-14.0, -14.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                ui.with_layout(Layout::bottom_up(Align::RIGHT), |ui| {
                    for toast in self.session.toasts.iter().rev() {
                        let edge = if toast.warn { AMBER } else { BLUE };
                        egui::Frame::new()
                            .fill(PANEL_DARK)
                            .stroke(Stroke::new(1.0, edge))
                            .corner_radius(6)
                            .inner_margin(egui::Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.label(RichText::new(&toast.text).color(if toast.warn { AMBER } else { OFF_WHITE }));
                            });
                    }
                });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

fn layout_is_complete(state: &DockState<Tab>) -> bool {
    Tab::ALL.iter().all(|t| state.find_tab(t).is_some())
}

/// A start timecode for the next song: ten minutes after the latest one.
fn next_start(s: &Session) -> String {
    let rate = s.project.rate;
    let latest = s.songs.iter().filter_map(|x| Timecode::parse_at(&x.start_tc, rate)).map(|tc| tc.to_frames(rate)).max();
    match latest {
        Some(frames) => {
            let ten_min = (600.0 * rate.fps()).round() as i64;
            let step = (frames / ten_min + 1) * ten_min;
            Timecode::from_frames(step % rate.frames_per_day(), rate).to_string()
        }
        None => "01:00:00:00".into(),
    }
}

// ------------------------------------------------------------------ window

enum Screen {
    Connect { error: Option<String> },
    Connecting(Link),
    Main(Box<Shell>),
}

pub struct PtzApp {
    settings: Settings,
    screen: Screen,
    repaint: Arc<dyn Fn() + Send + Sync>,
    last_layout_save: std::time::Instant,
}

impl PtzApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> PtzApp {
        apply(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let settings = Settings::load();
        let mut app =
            PtzApp { settings, screen: Screen::Connect { error: None }, repaint, last_layout_save: std::time::Instant::now() };
        app.connect();
        app
    }

    fn connect(&mut self) {
        let key = self.settings.access_key.trim();
        let link =
            link::connect(&self.settings.server, (!key.is_empty()).then_some(key), &self.settings.station, self.repaint.clone());
        self.screen = Screen::Connecting(link);
    }

    fn connect_screen(&mut self, ui: &mut Ui, error: Option<String>, busy: bool) {
        let mut go = false;
        let mut start_local = false;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(NAVY)).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.18).max(20.0));
                ui.label(RichText::new("PTZ Programmer").size(30.0).strong());
                ui.label(RichText::new("Timeline programming and playout for the FR7 and its pedestal").color(MUTED));
                ui.add_space(18.0);
                egui::Frame::new().fill(CHARCOAL).corner_radius(10).inner_margin(egui::Margin::same(18)).show(ui, |ui| {
                    ui.set_width(430.0);
                    egui::Grid::new("connect_grid").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                        ui.label("Show server");
                        let r = ui.add_enabled(
                            !busy,
                            egui::TextEdit::singleline(&mut self.settings.server)
                                .desired_width(290.0)
                                .hint_text("http://127.0.0.1:8787"),
                        );
                        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                            go = true;
                        }
                        ui.end_row();
                        ui.label("Station name");
                        ui.add_enabled(!busy, egui::TextEdit::singleline(&mut self.settings.station).desired_width(290.0))
                            .on_hover_text("Shown to other stations when this one holds the edit lease");
                        ui.end_row();
                        ui.label("Access key");
                        ui.add_enabled(
                            !busy,
                            egui::TextEdit::singleline(&mut self.settings.access_key)
                                .password(true)
                                .desired_width(290.0)
                                .hint_text("only for a remote server"),
                        );
                        ui.end_row();
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if busy {
                            ui.spinner();
                            ui.label("Connecting");
                        } else {
                            let ok = !self.settings.server.trim().is_empty() && !self.settings.station.trim().is_empty();
                            if ui
                                .add_enabled(
                                    ok,
                                    egui::Button::new(RichText::new("Connect").strong()).fill(BLUE).min_size(vec2(110.0, 32.0)),
                                )
                                .clicked()
                            {
                                go = true;
                            }
                            if ui
                                .add(egui::Button::new("Start a server on this computer").min_size(vec2(0.0, 32.0)))
                                .on_hover_text("Runs ptz-server here with the simulator rig, then connects to it")
                                .clicked()
                            {
                                start_local = true;
                            }
                        }
                    });
                    if let Some(e) = &error {
                        ui.add_space(8.0);
                        ui.label(RichText::new(e).color(AMBER));
                    }
                });
            });
        });
        if start_local {
            match link::start_local_server() {
                Ok(url) => {
                    self.settings.server = url;
                    // Give the server a moment to bind before the first request.
                    std::thread::sleep(std::time::Duration::from_millis(700));
                    go = true;
                }
                Err(e) => self.screen = Screen::Connect { error: Some(e) },
            }
        }
        if go {
            self.settings.save();
            self.connect();
        }
    }
}

impl eframe::App for PtzApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let mut next: Option<Screen> = None;
        match &mut self.screen {
            Screen::Connect { error } => {
                let error = error.clone();
                self.connect_screen(ui, error, false);
            }
            Screen::Connecting(link) => {
                let mut result = None;
                if let Ok(event) = link.rx.try_recv() {
                    match event {
                        Event::Connected(hello) => result = Some(Ok(hello)),
                        Event::ConnectFailed(msg) => result = Some(Err(msg)),
                        _ => {}
                    }
                }
                match result {
                    Some(Ok(hello)) => {
                        let Screen::Connecting(link) = std::mem::replace(&mut self.screen, Screen::Connect { error: None })
                        else {
                            unreachable!()
                        };
                        let session = Session::new(link, &self.settings.station, *hello);
                        let shell = Shell::new(session, self.settings.layout.take());
                        next = Some(Screen::Main(Box::new(shell)));
                    }
                    Some(Err(msg)) => {
                        let text = format!("Could not reach {}: {msg}", self.settings.server);
                        next = Some(Screen::Connect { error: Some(text) });
                    }
                    None => self.connect_screen(ui, None, true),
                }
            }
            Screen::Main(shell) => {
                shell.ui(ui);
                if shell.layout_changed && self.last_layout_save.elapsed().as_secs_f32() > 1.0 {
                    shell.layout_changed = false;
                    self.last_layout_save = std::time::Instant::now();
                    self.settings.layout = Some(shell.dock.clone());
                    self.settings.save();
                    self.settings.layout = None;
                }
            }
        }
        if let Some(screen) = next {
            self.screen = screen;
            ui.ctx().request_repaint();
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Screen::Main(shell) = &self.screen {
            self.settings.layout = Some(shell.dock.clone());
            self.settings.save();
        }
    }
}
