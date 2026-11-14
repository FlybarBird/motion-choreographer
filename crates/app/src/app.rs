//! The window: connect screen, fixed song header and transport bar, and the dock.

use crate::link::{self, Cmd, Event, Link};
use crate::panels::{self, text_field};
use crate::session::Session;
use crate::theme::*;
use crate::{monitor, timeline_ui};
use egui::{vec2, Align, Key, Layout, RichText, Stroke, Ui};
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
    Stage,
    Multiview,
}

impl Tab {
    pub const ALL: [Tab; 11] = [
        Tab::Presets,
        Tab::Media,
        Tab::Program,
        Tab::Stage,
        Tab::Multiview,
        Tab::Reference,
        Tab::Inspector,
        Tab::Rig,
        Tab::Timeline,
        Tab::Graph,
        Tab::Log,
    ];

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
            Tab::Stage => "Stage",
            Tab::Multiview => "Multiview",
        }
    }
}

pub fn default_layout() -> DockState<Tab> {
    let mut state = DockState::new(vec![Tab::Program, Tab::Stage, Tab::Multiview, Tab::Reference]);
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
    /// Servers this station has reached, newest first.
    pub recent: Vec<String>,
    pub layout: Option<DockState<Tab>>,
}

const RECENT_SERVERS: usize = 5;

/// `url` moved to the front of the recent list.
pub fn remember_server(recent: &mut Vec<String>, url: &str) {
    let url = url.trim();
    if url.is_empty() {
        return;
    }
    recent.retain(|r| r != url);
    recent.insert(0, url.to_string());
    recent.truncate(RECENT_SERVERS);
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

/// What the operator is told when an E-Stop pressed here has not been taken by the server.
pub const STOP_NOT_SENT: &str = "E-STOP has not reached the server";
pub const STOP_NOT_SENT_DO: &str = "The rig may still be moving. Use the stop control on the rig itself, or cut its power.";
pub const STOP_STILL_TRYING: &str = "The stop is still being sent.";
pub const STOP_GAVE_UP: &str = "The stop is no longer being sent. Click E-STOP to send it again.";

/// Arm is never a quiet switch: the rig travels to the pose at the playhead before anything plays.
pub const ARM_LABEL: &str = "Arm (moves rig to playhead)";

/// Every dialog the window can open. Each one is modal: the rest of the window is dimmed and takes no
/// click while it is open. E-STOP alone is kept above them all (see [`e_stop`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prompt {
    NewSong,
    LoadSong,
    SaveAs,
    Revert,
    DeleteSong,
    SyncPresets,
    TakeOver,
    RenameMarker,
    StampedDrop,
    StopNotSent,
}

impl Prompt {
    #[cfg(test)]
    pub const ALL: [Prompt; 10] = [
        Prompt::NewSong,
        Prompt::LoadSong,
        Prompt::SaveAs,
        Prompt::Revert,
        Prompt::DeleteSong,
        Prompt::SyncPresets,
        Prompt::TakeOver,
        Prompt::RenameMarker,
        Prompt::StampedDrop,
        Prompt::StopNotSent,
    ];

    /// The one way a dialog is put on the screen.
    fn show(self, ctx: &egui::Context, add: impl FnOnce(&mut Ui)) {
        egui::Modal::new(egui::Id::new(("prompt", self as u8))).show(ctx, add);
    }
}

/// Is a dialog open over the window? This is the one place that knows: it asks egui for the modal that
/// was on the screen when this frame's keys and clicks were made, so it holds for every dialog there is
/// and for any added later. While it is true the keyboard does nothing to the show and no jog is held.
pub fn modal_open(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.top_modal_layer().is_some())
}

/// The marker a double-click on the timeline asked to rename, and the name as typed so far.
pub fn marker_rename_id() -> egui::Id {
    egui::Id::new("rename_marker")
}

#[derive(Default)]
pub(crate) enum Dialog {
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
    pub(crate) dialog: Dialog,
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
            Tab::Stage => crate::stage_ui::show(ui, s),
            Tab::Multiview => crate::multiview::show(ui, s),
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
fn action_button(ui: &mut Ui, s: &mut Session, action: Action, glyph: &str, label: &str, look: Look, min_w: f32) -> bool {
    let g = &s.status.guidance;
    let enabled = s.offers(action);
    // Space is the transport's one key. Its buttons take a click, and never have the keyboard.
    let resp = click_only_button(ui, enabled, glyph, label, look, vec2(min_w, 30.0));
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

/// E-STOP is this size in every state and at every window size.
const ESTOP_SIZE: egui::Vec2 = vec2(108.0, 34.0);
/// A row of buttons in the transport bar is as high as its tallest control.
const BUTTON_ROW: f32 = ESTOP_SIZE.y;
/// The hint is left out where there is not even room for the mark that says it was cut short.
const HINT_LEAST: f32 = 16.0;

/// How the left of the transport bar is laid out. Its first row ends where the meters and the stop
/// control begin; what does not fit there goes onto rows under it, which have the whole width of the bar.
#[derive(Clone, Copy)]
struct Flow {
    /// The width of the bar.
    width: f32,
    /// What the bar may paint on.
    clip: egui::Rect,
}

impl Flow {
    /// Something of this size comes next: it starts a new row unless this one has room for it.
    fn make_room(&self, ui: &mut Ui, size: egui::Vec2) {
        let row_begun = ui.cursor().left() > ui.max_rect().left() + 0.5;
        if row_begun && size.x > ui.available_size_before_wrap().x {
            ui.end_row();
            ui.set_max_width(self.width);
            ui.set_clip_rect(self.clip);
            ui.set_row_height(size.y);
        }
    }

    /// Controls that stay together: when the row has no room for all of them, all of them go
    /// to the next row. They are given the width they were laid out at last.
    fn together(&self, ui: &mut Ui, key: &str, height: f32, add: impl FnOnce(&mut Ui)) {
        let id = egui::Id::new(("transport_group", key));
        let known = ui.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
        self.make_room(ui, vec2(known, height));
        let laid = ui.allocate_ui_with_layout(vec2(known, height), Layout::left_to_right(Align::Center), add);
        let width = laid.response.rect.width();
        if (width - known).abs() > 0.5 {
            ui.data_mut(|d| d.insert_temp(id, width));
            // They were given room for another width: lay the bar out again before it is shown.
            ui.ctx().request_discard("the transport bar changed");
        }
    }

    /// A sentence, whole: on the next row when this one has no room for it, and over several
    /// lines when it is longer than the bar is wide.
    fn sentence(&self, ui: &mut Ui, text: RichText) {
        let galley =
            egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Wrap), self.width, egui::TextStyle::Body);
        let size = vec2(galley.size().x.min(self.width), galley.size().y);
        self.make_room(ui, size);
        ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| ui.add(egui::Label::new(galley)));
    }

    /// The one-line hint is the first thing to give way. It has what is left of its row and is cut short
    /// there, with the whole of it under the pointer; it never opens a row, so the bar does not grow for it.
    fn hint(&self, ui: &mut Ui, text: &str) {
        let left = ui.available_size_before_wrap().x;
        if text.is_empty() || left < HINT_LEAST {
            return;
        }
        let text = RichText::new(text).color(MUTED);
        let whole = egui::WidgetText::from(text.clone()).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        );
        let size = vec2(whole.size().x.min(left), whole.size().y);
        ui.allocate_ui_with_layout(size, Layout::left_to_right(Align::Center), |ui| ui.add(egui::Label::new(text).truncate()));
    }
}

fn e_stop_has_keyboard_id() -> egui::Id {
    egui::Id::new("e_stop_has_keyboard")
}

/// E-STOP has a layer of its own above every dialog, menu and tip the window can show, over the place kept
/// for it in the transport bar. Whatever else is open it is drawn whole, in its own colour, and it takes
/// the click: a dialog stops clicks only on what lies beneath it, and nothing is laid out above this.
fn e_stop(ui: &mut Ui, s: &mut Session) {
    let (place, _) = ui.allocate_exact_size(ESTOP_SIZE, egui::Sense::hover());
    let id = egui::Id::new("e_stop");
    let ctx = ui.ctx().clone();
    // The first time a layer is laid out egui only measures it. That pass is done over at once, so that
    // there is no frame in which the button is not there.
    if ctx.memory(|m| m.area_rect(id).is_none()) {
        ctx.request_discard("E-STOP is on show from the first frame");
    }
    let pressed = egui::Area::new(id)
        .order(egui::Order::TOP)
        .fixed_pos(place.min)
        .fade_in(false)
        .sense(egui::Sense::hover())
        .show(&ctx, |ui| {
            // Unlike the rest of the transport bar it can be reached with Tab and pressed with Space or
            // Enter: there is no shortcut key for it, but no way of stopping is taken away either.
            let button = icon_button(ui, true, icon::WARNING_OCTAGON, "E-STOP", Look::Solid(RED), ESTOP_SIZE);
            let has_keyboard = button.has_focus();
            ui.data_mut(|d| d.insert_temp(e_stop_has_keyboard_id(), has_keyboard));
            button.on_hover_text("Stop all motion now. Works from any station, with or without the lease").clicked()
        })
        .inner;
    if pressed {
        s.act(Action::Estop);
    }
}

/// The icon that stands for a transport action wherever it is offered.
fn action_glyph(action: Action) -> &'static str {
    match action {
        Action::Play => icon::PLAY,
        Action::Stop => icon::STOP,
        Action::Cue => icon::SKIP_BACK,
        Action::Arm => icon::POWER,
        Action::Disarm => icon::PROHIBIT,
        Action::Lock => icon::LOCK_SIMPLE,
        Action::Unlock => icon::LOCK_SIMPLE_OPEN,
        Action::RecordOn | Action::RecordOff => icon::RECORD,
        Action::Estop => icon::WARNING_OCTAGON,
        Action::ResetEstop => icon::ARROW_COUNTER_CLOCKWISE,
        _ => icon::PLAY,
    }
}

impl Shell {
    pub fn new(session: Session, layout: Option<DockState<Tab>>) -> Shell {
        let dock = layout.map(with_every_tab).unwrap_or_else(default_layout);
        Shell { session, dock, dialog: Dialog::None, layout_changed: false }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        self.session.poll();
        self.shortcuts(ui);
        self.dropped_files(ui);

        egui::Panel::top("song_header")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(PANEL_DARK).inner_margin(egui::Margin::symmetric(12, 6)))
            .show(ui, |ui| self.header(ui));
        egui::Panel::top("transport_bar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(12, 8)))
            .show(ui, |ui| self.transport(ui));
        egui::Panel::top("camera_bar")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(BASE).inner_margin(egui::Margin { left: 12, right: 12, top: 6, bottom: 2 }))
            .show(ui, |ui| crate::camera_ui::bar(ui, &mut self.session));
        if let Some(lost) = self.session.lost.clone() {
            egui::Panel::top("lost_banner")
                .show_separator_line(false)
                .frame(egui::Frame::new().fill(RED).inner_margin(egui::Margin::symmetric(12, 6)))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{}  {lost}. The show keeps running on the server; this window is not in control.",
                            icon::PLUGS
                        ))
                        .font(semibold(13.0))
                        .color(egui::Color32::WHITE),
                    );
                });
        }

        let before = serde_json::to_string(&self.dock).unwrap_or_default();
        self.session.rig.keyboard = false;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BASE).inner_margin(egui::Margin::same(4))).show(ui, |ui| {
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
        // Whatever was or was not drawn above, this is where a jog is sent, changed or stopped.
        crate::rig_ui::settle(ui.ctx(), &mut self.session);

        if let Some(path) = self.session.stage_import_wanted.take() {
            self.import_stage(path);
        }
        if self.session.world.is_busy(&self.session.project.stage) || self.session.stage_view.is_dragging_a_camera() {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
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
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new(icon::VIDEO_CAMERA).font(icon_font(18.0)).color(ACCENT));
            ui.label(RichText::new("PTZ Programmer").font(semibold(13.5)));
            ui.add_space(8.0);
            let parked = s.status.transport.run == Run::Stopped && s.status.transport.output == Output::Rehearse;
            let need_lease = can_edit.clone().err().filter(|_| !s.status.you_hold_lease || s.lost.is_some());
            let gate = |ui: &mut Ui, enabled: bool, glyph: &str, label: &str, why: Option<String>, tip: &str| -> bool {
                let r = icon_button(ui, enabled, glyph, label, Look::Quiet, vec2(0.0, 26.0));
                let r = match (&why, enabled) {
                    (Some(w), false) => r.on_disabled_hover_text(w.clone()),
                    _ => r.on_hover_text(tip),
                };
                r.clicked()
            };
            let park_why = need_lease.clone().or_else(|| (!parked).then(|| "Stop and disarm to change songs".to_string()));
            if gate(ui, park_why.is_none(), icon::PLUS, "New", park_why.clone(), "Start a new song") {
                let n = s.songs.len() + 1;
                self.dialog = Dialog::New { name: format!("Song {n}"), start_tc: next_start(s) };
            }
            if gate(ui, park_why.is_none(), icon::FOLDER_OPEN, "Load", park_why.clone(), "Open another song") {
                s.link.send(Cmd::RefreshSongs);
                self.dialog = Dialog::Load;
            }
            let dirty = st.as_ref().is_some_and(|x| x.dirty);
            let has = song_id.is_some();
            let save_why = need_lease
                .clone()
                .or_else(|| (!has).then(|| "No song is open".to_string()))
                .or_else(|| (!dirty).then(|| "Nothing to save".to_string()));
            if gate(ui, save_why.is_none(), icon::FLOPPY_DISK, "Save", save_why, "Save the draft  ( Cmd-S )") {
                if let Some(id) = &song_id {
                    s.link.send(Cmd::Save(id.clone()));
                }
            }
            let any_why = need_lease.clone().or_else(|| (!has).then(|| "No song is open".to_string()));
            if gate(ui, any_why.is_none(), icon::COPY, "Save As", any_why.clone(), "Save a copy under a new name and open it") {
                let name = s.song.as_ref().map(|x| format!("{} copy", x.name)).unwrap_or_default();
                self.dialog = Dialog::SaveAs { name };
            }
            let revert_why = can_edit
                .clone()
                .err()
                .or_else(|| (!has).then(|| "No song is open".to_string()))
                .or_else(|| (!dirty).then(|| "No changes since the last Save".to_string()));
            if gate(ui, revert_why.is_none(), icon::ARROW_COUNTER_CLOCKWISE, "Revert", revert_why, "Go back to the last Save") {
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
                icon::SEAL_CHECK,
                "Commit",
                commit_why,
                "Freeze this draft as the next revision. Only committed revisions can be armed",
            ) {
                if let Some(id) = &song_id {
                    s.link.send(Cmd::Commit(id.clone()));
                }
            }
            ui.add_space(8.0);
            ui.spacing_mut().item_spacing.x = 8.0;

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
                        chip(ui, &rev, if st.committed_clean { GREEN } else { MUTED }, false);
                        if st.dirty {
                            chip(ui, "unsaved", AMBER, false).on_hover_text(
                                "The draft has changes since the last Save. They are kept on the server either way",
                            );
                        }
                    }
                    let n = s.compiled.issues.iter().filter_map(|i| i.clip_id()).collect::<std::collections::BTreeSet<_>>().len()
                        + s.compiled.issues.iter().filter(|i| i.clip_id().is_none()).count().min(1);
                    if n > 0 {
                        let blocking = s.song.as_ref().map(|song| s.compiled.blocking_count(song)).unwrap_or(0);
                        let text = format!("{n} issue{}", if n == 1 { "" } else { "s" });
                        ui.label(RichText::new(icon::WARNING).font(icon_font(15.0)).color(AMBER));
                        let r = ui.add(
                            egui::Label::new(RichText::new(text).color(AMBER).font(semibold(13.0))).sense(egui::Sense::click()),
                        );
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
                let view = ui.button("View");
                // E-STOP is just beneath and is drawn above everything: the menu opens beside it, not across it.
                let clear = ui.max_rect().right() - ESTOP_SIZE.x - 6.0;
                let beside =
                    egui::Rect::from_x_y_ranges(view.rect.left().min(clear)..=view.rect.right().min(clear), view.rect.y_range());
                egui::Popup::menu(&view).anchor(beside).align(egui::RectAlign::BOTTOM_END).align_alternatives(&[]).show(|ui| {
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
                    chip(ui, "You hold the edit lease", GREEN, false);
                } else {
                    let (label, text, force) = match &s.status.lease {
                        Some(l) => ("Take over", format!("Watching. {} holds the edit lease", l.holder), true),
                        None => ("Take the lease", "Watching. Nobody holds the edit lease".to_string(), false),
                    };
                    if ui.add(filled(label, ACCENT)).clicked() {
                        if force {
                            s.confirm_takeover = true;
                        } else {
                            s.link.send(Cmd::TakeLease { name: s.station.clone(), force });
                        }
                    }
                    ui.label(RichText::new(icon::EYE).font(icon_font(15.0)).color(AMBER));
                    ui.label(RichText::new(text).color(AMBER));
                }
                ui.label(RichText::new(&s.link.base).color(MUTED).small());
            });
        });
    }

    fn transport(&mut self, ui: &mut Ui) {
        let s = &mut self.session;
        let t = s.status.transport;
        let flow = Flow { width: ui.available_width(), clip: ui.clip_rect() };
        ui.horizontal(|ui| {
            // The stop control is laid out first and takes its place at the right. What is left of the
            // row is all the rest may use: it goes onto further rows instead of running on under the button.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if t.estop {
                    let reset = action_glyph(Action::ResetEstop);
                    action_button(ui, s, Action::ResetEstop, reset, "Reset E-Stop", Look::Solid(AMBER), 130.0);
                    chip(ui, "E-STOP LATCHED", RED, true);
                } else {
                    e_stop(ui, s);
                }
                ui.add_space(4.0);
                crate::media_ui::meters(ui, s);
                ui.add_space(4.0);

                let room = ui.available_rect_before_wrap();
                ui.with_layout(Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
                    // Should anything still be wider than the room it has, it is cut off there, not drawn over the meters and the stop.
                    ui.shrink_clip_rect(egui::Rect::everything_left_of(room.right()));
                    let frame = s.playhead().floor() as i64;
                    let ink = if t.estop {
                        RED
                    } else if s.status.hardware_live {
                        GREEN
                    } else {
                        OFF_WHITE
                    };
                    // The main button: whatever the server says is the next step.
                    let g = s.status.guidance.clone();
                    let primary = g.primary.filter(|a| *a != Action::Estop && *a != Action::ResetEstop);
                    let live = t.output != Output::Rehearse;
                    flow.together(ui, "run", BUTTON_ROW, |ui| {
                        egui::Frame::new()
                            .fill(BASE)
                            .stroke(Stroke::new(1.0, LINE))
                            .corner_radius(7)
                            .inner_margin(egui::Margin::symmetric(12, 3))
                            .show(ui, |ui| ui.label(RichText::new(s.timecode_at(frame)).monospace().size(22.0).color(ink)));
                        ui.add_space(4.0);
                        match primary {
                            Some(action) => {
                                let look = match action {
                                    Action::Play if live => Look::Solid(GREEN),
                                    Action::Stop => Look::Solid(OFF_WHITE),
                                    _ => Look::Solid(ACCENT),
                                };
                                action_button(ui, s, action, action_glyph(action), &g.primary_label, look, 124.0);
                            }
                            None => {
                                icon_button(ui, false, icon::PLAY, "Rehearse", Look::Plain, vec2(124.0, 30.0))
                                    .on_disabled_hover_text(g.hint.clone());
                            }
                        }
                        if primary != Some(Action::Stop) {
                            action_button(ui, s, Action::Stop, icon::STOP, "Stop", Look::Plain, 0.0);
                        }
                        action_button(ui, s, Action::Cue, icon::SKIP_BACK, "Cue", Look::Plain, 0.0);
                    });
                    ui.add_space(6.0);

                    flow.together(ui, "modes", BUTTON_ROW, |ui| {
                        if t.locked() {
                            action_button(ui, s, Action::Unlock, icon::LOCK_SIMPLE_OPEN, "Unlock", Look::Plain, 0.0);
                        } else {
                            crate::lock_ui::lock_menu(ui, s);
                        }
                        if t.output == Output::Rehearse {
                            action_button(ui, s, Action::Arm, icon::POWER, ARM_LABEL, Look::Outline(GREEN), 0.0);
                        } else {
                            action_button(ui, s, Action::Disarm, icon::PROHIBIT, "Disarm", Look::Outline(AMBER), 0.0);
                        }
                        if t.record {
                            action_button(ui, s, Action::RecordOff, icon::RECORD, "Record off", Look::Solid(RED), 0.0);
                        } else {
                            action_button(ui, s, Action::RecordOn, icon::RECORD, "Record", Look::Outline(RED), 0.0);
                        }
                    });
                    ui.add_space(6.0);

                    flow.together(ui, "state", chip_height(ui), |ui| crate::lock_ui::state_chips(ui, s, flow.width));
                    // What is wrong is said in full, on as many lines as it takes.
                    if let Some(problem) = &s.status.lock.problem {
                        flow.sentence(ui, RichText::new(problem).color(AMBER));
                    }
                    if let Some(fault) = &s.status.telemetry.fault {
                        flow.sentence(ui, RichText::new(fault).color(RED).strong());
                    }
                    flow.hint(ui, &g.hint);
                });
            });
        });
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        let s = &mut self.session;
        // A dialog has the keyboard to itself: nothing pressed while one is open reaches the show
        // behind it. (There is no key for E-Stop; its button is above every dialog.)
        if modal_open(&ctx) {
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
        if with_cmd(Key::S) {
            if let Some(song) = &s.song {
                s.link.send(Cmd::Save(song.id.clone()));
            }
        }
        // Nothing below edits or runs the show while a text field has the keyboard. Cmd-Z there
        // is the field's own undo: it does not reach past the field into the song.
        if ctx.text_edit_focused() {
            return;
        }
        if let Some(m) = chord(Key::Z).filter(|m| m.command) {
            if m.shift {
                s.redo();
            } else {
                s.undo();
            }
        }
        if with_cmd(Key::D) {
            timeline_ui::duplicate_selection(s);
        }
        crate::camera_ui::shortcuts(&ctx, s);
        if pressed(Key::Escape) {
            s.timeline.cancel();
        }
        // With the keyboard on E-STOP, Space presses E-STOP and does nothing else.
        let on_e_stop = ctx.data(|d| d.get_temp::<bool>(e_stop_has_keyboard_id())).unwrap_or(false);
        if pressed(Key::Space) && !on_e_stop {
            // Otherwise Space is the transport's key and nothing else's. A button that Tab left the
            // keyboard on is not pressed as well: it gives the keyboard up before it can take the key.
            if !s.rig.keyboard {
                ctx.memory_mut(|m| {
                    if let Some(id) = m.focused() {
                        m.surrender_focus(id);
                    }
                });
            }
            let g = &s.status.guidance;
            // Space runs the transport. Releasing the E-Stop latch takes a click on its own button.
            // A key does what its button would: nothing, while the button is off.
            if let Some(action) = g.primary.filter(|a| !matches!(a, Action::Estop | Action::ResetEstop)) {
                if s.offers(action) {
                    s.act(action);
                }
            }
        }
        if pressed(Key::Home) && s.offers(Action::Cue) {
            s.act(Action::Cue);
        }
        let step = if shift { 10 } else { 1 };
        // While the rig controller has the keyboard, the arrows drive the rig.
        let can_seek = s.offers(Action::Seek) && !s.rig.keyboard;
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
            if crate::stage::is_model(&path) {
                self.import_stage(path);
            } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mtl")) {
                // A material file dropped with its model is read from beside the model, not uploaded as media.
            } else {
                self.session.link.send(Cmd::Upload(path));
            }
        }
    }

    /// Start importing a stage model and bring the Stage view forward to watch it arrive.
    pub fn import_stage(&mut self, path: PathBuf) {
        if let Err(why) = self.session.can_edit() {
            self.session.toast(
                why.replace("to edit the song", "to import a stage model").replace("to edit", "to import a stage model"),
                true,
            );
            return;
        }
        self.session.link.send(Cmd::ImportStage(path));
        self.session.stage_view.fit_requested = true;
        self.show_tab(Tab::Stage);
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let s = &mut self.session;
        let mut close = false;
        let mut next: Option<Dialog> = None;
        match &mut self.dialog {
            Dialog::None => {}
            Dialog::New { name, start_tc } => {
                Prompt::NewSong.show(ctx, |ui| {
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
                        if ui.add_enabled(ok, filled("Create", ACCENT)).clicked() {
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
                Prompt::LoadSong.show(ctx, |ui| {
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
                Prompt::SaveAs.show(ctx, |ui| {
                    ui.set_width(340.0);
                    ui.heading("Save As");
                    ui.label(RichText::new("Makes a copy with its own revisions and opens it.").color(MUTED));
                    ui.add_space(6.0);
                    ui.add(egui::TextEdit::singleline(name).desired_width(300.0));
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add_enabled(!name.trim().is_empty(), filled("Save copy", ACCENT)).clicked() {
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
                Prompt::Revert.show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.heading("Revert to the last Save?");
                    ui.label("Every change since the last Save is dropped. This cannot be undone.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(filled("Revert", AMBER)).clicked() {
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
                Prompt::DeleteSong.show(ctx, |ui| {
                    ui.set_width(360.0);
                    ui.heading(format!("Delete {name}?"));
                    ui.label("The draft and every committed revision are removed from the server.");
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.add(filled("Delete", RED)).clicked() {
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
            Prompt::SyncPresets.show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading("Read presets from the camera?");
                ui.label("The camera is recalled to each of its presets in turn, so that its pose and picture can be read. It will move, at its own speed, through every preset it holds.");
                ui.label(RichText::new("The pedestal does not move. E-Stop stops the camera and abandons the sync.").color(MUTED));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(filled("Move the camera and sync", AMBER)).clicked() {
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
        if s.confirm_takeover {
            let holder = s.status.lease.as_ref().map(|l| l.holder.clone()).unwrap_or_default();
            Prompt::TakeOver.show(ctx, |ui| {
                ui.set_width(380.0);
                ui.heading(format!("Take the edit lease from {holder}?"));
                ui.label(format!("{holder} stops being able to edit or run the show at once, and is told this station took over. The song stays as it is."));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(filled(format!("Take it from {holder}"), AMBER)).clicked() {
                        s.link.send(Cmd::TakeLease { name: s.station.clone(), force: true });
                        s.confirm_takeover = false;
                    }
                    if ui.button("Cancel").clicked() {
                        s.confirm_takeover = false;
                    }
                });
            });
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                s.confirm_takeover = false;
            }
        }
        if let Some(d) = next {
            self.dialog = d;
        } else if close || (!matches!(self.dialog, Dialog::None) && ctx.input(|i| i.key_pressed(Key::Escape))) {
            self.dialog = Dialog::None;
        }

        // Marker rename, opened by a double-click on the timeline.
        let rename_id = marker_rename_id();
        if let Some((id, mut name)) = ctx.data(|d| d.get_temp::<(String, String)>(rename_id)) {
            let mut done = false;
            let mut apply = false;
            Prompt::RenameMarker.show(ctx, |ui| {
                ui.set_width(300.0);
                ui.heading("Rename marker");
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(260.0));
                r.request_focus();
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    apply = true;
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add(filled("Rename", ACCENT)).clicked() {
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
        self.stop_not_sent_dialog(ctx);
    }

    /// E-Stop was pressed here and the server has not taken it. This is put up over everything else,
    /// and stays until the operator closes it or the server reports the stop latched.
    fn stop_not_sent_dialog(&mut self, ctx: &egui::Context) {
        let s = &mut self.session;
        let Some((why, trying)) = s.stop_not_sent.clone() else { return };
        let mut close = false;
        Prompt::StopNotSent.show(ctx, |ui| {
            ui.set_width(440.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(icon::WARNING_OCTAGON).font(icon_font(22.0)).color(RED));
                ui.heading(RichText::new(STOP_NOT_SENT).color(RED));
            });
            ui.add_space(4.0);
            ui.label(RichText::new(STOP_NOT_SENT_DO).strong());
            ui.add_space(4.0);
            ui.label(RichText::new(format!("What went wrong: {}", why.trim_end_matches('.'))).color(MUTED));
            ui.label(if trying { STOP_STILL_TRYING } else { STOP_GAVE_UP });
            ui.add_space(8.0);
            if ui.button("Close").clicked() {
                close = true;
            }
        });
        if close || ctx.input(|i| i.key_pressed(Key::Escape)) {
            s.stop_not_sent = None;
        }
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
        Prompt::StampedDrop.show(ctx, |ui| {
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
                if ui.add(filled(format!("Set the song start to {file_start}"), ACCENT)).clicked() {
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

/// A layout saved by an earlier version lacks the tabs added since: they join the Program monitor's group,
/// and the arrangement the operator made is kept.
fn with_every_tab(mut state: DockState<Tab>) -> DockState<Tab> {
    for tab in Tab::ALL {
        if state.find_tab(&tab).is_some() {
            continue;
        }
        let beside = state.find_tab(&Tab::Program).or_else(|| state.iter_all_tabs().next().map(|(path, _)| path));
        let Some(path) = beside else { return default_layout() };
        let Some(leaf) = state[path.surface][path.node].get_leaf_mut() else { return default_layout() };
        // Appending brings the new tab to the front; the one the operator was looking at stays in front.
        let active = leaf.active;
        leaf.append_tab(tab);
        leaf.active = active;
    }
    state
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
    /// Whether the show server is installed on this computer, and when that was last looked at.
    have_server: Option<(std::time::Instant, bool)>,
}

impl PtzApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> PtzApp {
        apply(&cc.egui_ctx);
        let ctx = cc.egui_ctx.clone();
        let repaint: Arc<dyn Fn() + Send + Sync> = Arc::new(move || ctx.request_repaint());
        let settings = Settings::load();
        let mut app = PtzApp {
            settings,
            screen: Screen::Connect { error: None },
            repaint,
            last_layout_save: std::time::Instant::now(),
            have_server: None,
        };
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
        // The app can be installed without the server. Looked for again now and then, in case it is installed meanwhile.
        if self.have_server.is_none_or(|(at, _)| at.elapsed() > std::time::Duration::from_secs(2)) {
            self.have_server = Some((std::time::Instant::now(), link::local_server().is_some()));
        }
        let have_server = self.have_server.is_some_and(|(_, found)| found);
        let mut go = false;
        let mut start_local = false;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BASE)).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space((ui.available_height() * 0.18).max(20.0));
                ui.label(RichText::new("PTZ Programmer").size(30.0).strong());
                ui.label(RichText::new("Timeline programming and playout for the FR7 and its pedestal").color(MUTED));
                ui.add_space(18.0);
                egui::Frame::new().fill(PANEL).corner_radius(10).inner_margin(egui::Margin::same(18)).show(ui, |ui| {
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
                        let others: Vec<String> =
                            self.settings.recent.iter().filter(|r| r.as_str() != self.settings.server.trim()).cloned().collect();
                        if !others.is_empty() {
                            ui.label(RichText::new("Recent").color(MUTED));
                            ui.vertical(|ui| {
                                for url in others {
                                    let link = ui.add_enabled(!busy, egui::Button::new(&url).frame(false));
                                    if link.on_hover_text("Use this server").clicked() {
                                        self.settings.server = url;
                                    }
                                }
                            });
                            ui.end_row();
                        }
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
                            if ui.add_enabled(ok, filled("Connect", ACCENT).min_size(vec2(110.0, 32.0))).clicked() {
                                go = true;
                            }
                            if ui
                                .add_enabled(
                                    have_server,
                                    egui::Button::new("Start a server on this computer").min_size(vec2(0.0, 32.0)),
                                )
                                .on_hover_text("Runs ptz-server here on the show folder in your home folder, then connects to it. It drives the rig that project names, the simulator on a new folder, and keeps running after the app quits")
                                .on_disabled_hover_text(link::NO_LOCAL_SERVER)
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
                        let server = self.settings.server.clone();
                        remember_server(&mut self.settings.recent, &server);
                        self.settings.save();
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
