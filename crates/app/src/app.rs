//! The window: connect screen, fixed song header and transport bar, and the dock.

use crate::link::{self, Cmd, Event, Link};
use crate::panels::{self, text_field};
use crate::session::Session;
use crate::theme::*;
use crate::update::{self, Step, Updater};
use crate::{monitor, timeline_ui};
use egui::{vec2, Align, FontId, Key, Layout, RichText, Stroke, Ui};
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
    SetList,
}

impl Tab {
    pub const ALL: [Tab; 12] = [
        Tab::Presets,
        Tab::SetList,
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
            Tab::SetList => "Set list",
        }
    }
}

/// The panels under the transport strip: the timeline and what shares its place.
pub const LOWER: [Tab; 3] = [Tab::Timeline, Tab::Graph, Tab::Log];

/// The panels over the transport strip: the browsers at the left, the stage viewer and the program
/// monitor side by side in the middle, the inspector at the right.
pub fn default_layout() -> DockState<Tab> {
    let mut state = DockState::new(vec![Tab::Program, Tab::Multiview, Tab::Reference]);
    let surface = state.main_surface_mut();
    let [centre, _] = surface.split_left(NodeIndex::root(), 0.2, vec![Tab::Presets, Tab::SetList, Tab::Media]);
    let [centre, _] = surface.split_right(centre, 0.7, vec![Tab::Inspector, Tab::Rig]);
    let [_, _] = surface.split_left(centre, 0.5, vec![Tab::Stage]);
    state
}

/// The panels under the transport strip.
pub fn lower_layout() -> DockState<Tab> {
    DockState::new(LOWER.to_vec())
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
    /// The panels under the transport strip.
    pub lower_layout: Option<DockState<Tab>>,
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
    Update,
}

impl Prompt {
    #[cfg(test)]
    pub const ALL: [Prompt; 11] = [
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
        Prompt::Update,
    ];

    /// The one way a dialog is put on the screen.
    fn show(self, ctx: &egui::Context, add: impl FnOnce(&mut Ui)) {
        egui::Modal::new(egui::Id::new(("prompt", self as u8))).frame(dialog_frame()).backdrop_color(BACKDROP).show(ctx, add);
    }
}

/// Is a dialog open over the window? This is the one place that knows: it asks egui for the modal that
/// was on the screen when this frame's keys and clicks were made, so it holds for every dialog there is
/// and for any added later. While it is true the keyboard does nothing to the show and no jog is held.
pub fn modal_open(ctx: &egui::Context) -> bool {
    ctx.memory(|m| m.top_modal_layer().is_some())
}

/// Is the server at this address on this computer?
pub fn on_this_computer(server: &str) -> bool {
    let host = server.trim().trim_start_matches("http://").trim_start_matches("https://");
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|own| host.starts_with(own))
}

/// The update dialog, while it is open: what there is to install, how the install is going, and what
/// went wrong. `busy` is why the app may not close just now. With `server` the show server on this
/// computer is restarted when the app reopens, so that it is the new version too.
pub(crate) fn update_dialog(ctx: &egui::Context, updater: &Updater, busy: Option<&str>, server: bool) {
    // An install that was asked for and has finished reopens the app at once, if the show still allows it.
    if updater.take_reopen() && busy.is_none() && update::can_reopen() {
        reopen(ctx, updater, server);
    }
    if !updater.is_open() {
        return;
    }
    let step = updater.step();
    let mut close = false;
    Prompt::Update.show(ctx, |ui| {
        ui.set_width(420.0);
        let this = format!("This is version {}.", update::VERSION);
        let quiet = |ui: &mut Ui, text: &str| {
            ui.label(RichText::new(text).color(MUTED));
        };
        let buttons = |ui: &mut Ui, add: &mut dyn FnMut(&mut Ui), last: &str, close: &mut bool| {
            ui.add_space(PAD);
            ui.horizontal(|ui| {
                add(ui);
                if ui.button(last).clicked() {
                    *close = true;
                }
            });
        };
        match &step {
            Step::Off(why) => {
                dialog_title(ui, "Updates");
                ui.label(&this);
                quiet(ui, why);
                buttons(ui, &mut |_| {}, "Close", &mut close);
            }
            Step::Idle | Step::Checking => {
                dialog_title(ui, "Updates");
                ui.label(&this);
                ui.horizontal(|ui| {
                    ui.spinner();
                    quiet(ui, "Looking for a newer one");
                });
                buttons(ui, &mut |_| {}, "Close", &mut close);
            }
            Step::Newest => {
                dialog_title(ui, "Updates");
                ui.label(format!("{this} It is the newest."));
                buttons(
                    ui,
                    &mut |ui| {
                        if ui.button("Look again").clicked() {
                            updater.check();
                        }
                    },
                    "Close",
                    &mut close,
                );
            }
            Step::NotChecked(why) => {
                dialog_title(ui, "Updates");
                ui.label(&this);
                ui.label(RichText::new(why).color(AMBER));
                buttons(
                    ui,
                    &mut |ui| {
                        if ui.add(filled("Look again", ACCENT)).clicked() {
                            updater.check();
                        }
                    },
                    "Close",
                    &mut close,
                );
            }
            Step::Available(release) | Step::Failed(release, _) => {
                let failed = matches!(step, Step::Failed(..));
                if let Step::Failed(_, why) = &step {
                    dialog_title(ui, format!("Version {} was not installed", release.version));
                    ui.label(RichText::new(why).color(AMBER));
                } else {
                    dialog_title(ui, format!("Version {} is available", release.version));
                    ui.label(&this);
                    if !release.notes.trim().is_empty() {
                        ui.add_space(U);
                        ui.label(release.notes.trim());
                    }
                }
                ui.add_space(U);
                quiet(
                    ui,
                    if server {
                        "The Mac asks for its password. Then the app and the show server on this computer close, and reopen as the new version."
                    } else {
                        "The Mac asks for its password. Then the app closes, and reopens as the new version."
                    },
                );
                buttons(
                    ui,
                    &mut |ui| {
                        let label = if failed { "Try again" } else { "Install and reopen" };
                        let install = ui.add_enabled(busy.is_none(), filled(label, ACCENT));
                        if install.on_disabled_hover_text(busy.unwrap_or_default()).clicked() {
                            updater.install();
                        }
                    },
                    "Later",
                    &mut close,
                );
                if let Some(busy) = busy {
                    ui.label(RichText::new(busy).color(AMBER));
                }
            }
            Step::Downloading(release, got) => {
                dialog_title(ui, format!("Downloading version {}", release.version));
                let size = release.installer.size.max(1);
                ui.add(egui::ProgressBar::new((*got as f32 / size as f32).min(1.0)).desired_width(ui.available_width()));
                quiet(ui, &format!("{:.1} of {:.1} MB", *got as f64 / 1e6, size as f64 / 1e6));
                buttons(ui, &mut |_| {}, "Hide", &mut close);
            }
            Step::Installing(release) => {
                dialog_title(ui, format!("Installing version {}", release.version));
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("The Mac asks for its password, then installs it.");
                });
                buttons(ui, &mut |_| {}, "Hide", &mut close);
            }
            Step::Installed(release) => {
                dialog_title(ui, format!("Version {} is installed", release.version));
                if update::can_reopen() {
                    ui.label("It runs once the app has been closed and opened again.");
                    buttons(
                        ui,
                        &mut |ui| {
                            let again = ui.add_enabled(busy.is_none(), filled("Reopen now", ACCENT));
                            if again.on_disabled_hover_text(busy.unwrap_or_default()).clicked() {
                                reopen(ctx, updater, server);
                            }
                        },
                        "Later",
                        &mut close,
                    );
                    if let Some(busy) = busy {
                        ui.label(RichText::new(busy).color(AMBER));
                    }
                } else {
                    ui.label("Quit the app and open it again to use it.");
                    buttons(ui, &mut |_| {}, "Close", &mut close);
                }
            }
        }
    });
    if close || ctx.input(|i| i.key_pressed(Key::Escape)) {
        updater.open(false);
    }
}

/// Close the app and have it opened again, as the version that was just installed.
fn reopen(ctx: &egui::Context, updater: &Updater, server: bool) {
    if update::reopen_after_exit(server).is_ok() {
        updater.open(false);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
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
    /// The panels over the transport strip.
    pub dock: DockState<Tab>,
    /// The panels under it: the timeline, the graph editor and the log.
    pub lower: DockState<Tab>,
    pub(crate) dialog: Dialog,
    pub layout_changed: bool,
    /// Where the transport strip was drawn last.
    pub transport_rect: egui::Rect,
    /// Looks for a newer version of the app. The window's own; one that never looks, in the tests.
    pub updater: Updater,
    /// The newer version the operator has been told of.
    update_told: Option<String>,
}

impl Shell {
    /// Bring a tab to the front of its group.
    pub fn show_tab(&mut self, tab: Tab) {
        for dock in [&mut self.dock, &mut self.lower] {
            if let Some(path) = dock.find_tab(&tab) {
                let _ = dock.set_active_tab(path);
            }
        }
    }
}

struct Viewer<'a> {
    session: &'a mut Session,
    /// The tab in front of each group, which the tab strip marks.
    front: Vec<Tab>,
}

impl egui_dock::TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("dock_tab", *tab))
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab_title(tab.title())
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
            Tab::SetList => crate::setlist_ui::show(ui, s),
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

    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        if self.front.contains(tab) {
            tab_in_front(response);
        }
    }
}

/// A transport button: enabled when the server says so, with the server's reason when not.
fn action_button(ui: &mut Ui, s: &mut Session, action: Action, glyph: &str, label: &str, look: Look, min_w: f32) -> bool {
    let g = &s.status.guidance;
    let enabled = s.offers(action);
    // Space is the transport's one key. Its buttons take a click, and never have the keyboard.
    let resp = click_only_button(ui, enabled, glyph, label, look, vec2(min_w, CONTROL_LARGE));
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

/// E-STOP is this size in every state and at every window size: wider and taller than any other control.
pub(crate) const ESTOP_SIZE: egui::Vec2 = vec2(30.0 * U, 9.0 * U);
/// The yellow ground it stands on shows this far round it.
const ESTOP_GROUND: egui::Vec2 = vec2(ESTOP_SIZE.x + 2.0 * U, ESTOP_SIZE.y + 2.0 * U);
/// The width of Reset E-Stop, beside the latch.
const RESET_W: f32 = 130.0;
/// A row of buttons in the transport bar is as high as its tallest control.
const BUTTON_ROW: f32 = ESTOP_GROUND.y;
/// The hint is left out where there is not even room for the mark that says it was cut short.
const HINT_LEAST: f32 = 16.0;

/// How high the transport strip is with its controls on one row.
const TRANSPORT_H: f32 = ESTOP_GROUND.y + 2.0 * (U + 2.0);

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
/// for it in the top right corner. Whatever else is open it is drawn whole, in its own colour, and it takes
/// the click: a dialog stops clicks only on what lies beneath it, and nothing is laid out above this.
fn e_stop(ctx: &egui::Context, s: &mut Session, place: egui::Rect) {
    let id = egui::Id::new("e_stop");
    let ctx = ctx.clone();
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
            let (ground, _) = ui.allocate_exact_size(ESTOP_GROUND, egui::Sense::hover());
            let button = stop_button(ui, icon::WARNING_OCTAGON, "E-STOP", ground, ESTOP_SIZE);
            let has_keyboard = button.has_focus();
            ui.data_mut(|d| d.insert_temp(e_stop_has_keyboard_id(), has_keyboard));
            button.on_hover_text("Stop all motion now. Works from any station, with or without the lease").clicked()
        })
        .inner;
    if pressed {
        s.act(Action::Estop);
    }
}

/// Space: the transport's key and nothing else's.
fn transport_key(ctx: &egui::Context, s: &mut Session) {
    // With the keyboard on E-STOP, Space presses E-STOP and does nothing else.
    if ctx.data(|d| d.get_temp::<bool>(e_stop_has_keyboard_id())).unwrap_or(false) {
        return;
    }
    // A button that Tab left the keyboard on is not pressed as well: it gives the keyboard up
    // before it can take the key.
    if !s.rig.keyboard {
        ctx.memory_mut(|m| {
            if let Some(id) = m.focused() {
                m.surrender_focus(id);
            }
        });
    }
    // Space runs the transport. Releasing the E-Stop latch takes a click on its own button.
    // A key does what its button would: nothing, while the button is off.
    if let Some(action) = s.status.guidance.primary.filter(|a| !matches!(a, Action::Estop | Action::ResetEstop)) {
        if s.offers(action) {
            s.act(action);
        }
    }
}

/// The colour of the rule under the transport bar: the machine's state, red as well while a rig has
/// faulted, and a plain hairline in rehearsal. The chips beside the buttons say the same in words.
fn machine_rule(s: &Session) -> egui::Color32 {
    if s.status.telemetry.fault.is_some() {
        RED
    } else {
        machine_ink(&s.status.transport, s.status.hardware_live).unwrap_or(LINE)
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
        let dock = layout.map(|l| with_every_tab(without_lower(l))).unwrap_or_else(default_layout);
        Shell {
            session,
            dock,
            lower: lower_layout(),
            dialog: Dialog::None,
            layout_changed: false,
            transport_rect: egui::Rect::NOTHING,
            updater: Updater::off(),
            update_told: None,
        }
    }

    /// Why the app may not close to install an update just now.
    fn too_busy_to_update(&self) -> Option<&'static str> {
        let s = &self.session;
        let t = &s.status.transport;
        let parked = t.run == Run::Stopped && t.output == Output::Rehearse && !t.record;
        // A window that has lost the server is not running the show, and closing it changes nothing.
        (s.lost.is_none() && !parked).then_some("Stop the show and disarm it first: the app closes to install the update.")
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        self.session.poll();
        if let Some(version) = self.updater.offered().filter(|v| self.update_told.as_ref() != Some(v)) {
            self.session.toast(format!("Version {version} is available: View, then Update"), false);
            self.update_told = Some(version);
        }
        self.shortcuts(ui);
        self.dropped_files(ui);

        // Two bars across the top: the song and the cameras. The machine's strip, with the clock, the
        // transport and E-STOP, runs across the window between the viewers and the timeline.
        let bar = |fill, y: f32| egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(PAD as i8, y as i8));
        // The song bar runs the width of the window. The camera bar under it leaves its right end to
        // E-STOP, which stands there on a layer of its own: above everything, so that nothing in the
        // panels, however far it runs, is ever laid out over it.
        egui::Panel::top("song_header").frame(bar(PANEL_DARK, U + 2.0)).show(ui, |ui| self.header(ui));
        let corner = self.corner_width();
        let cameras = egui::Panel::top("camera_bar")
            .frame(bar(PANEL_DARK, U))
            .show(ui, |ui| {
                ui.set_min_height(ESTOP_GROUND.y);
                let mut room = ui.available_rect_before_wrap();
                room.max.x -= corner;
                ui.scope_builder(egui::UiBuilder::new().max_rect(room).layout(*ui.layout()), |ui| {
                    crate::camera_ui::bar(ui, &mut self.session)
                });
            })
            .response
            .rect;
        self.stop_corner(ui, cameras);
        if let Some(lost) = self.session.lost.clone() {
            egui::Panel::top("lost_banner").show_separator_line(false).frame(bar(RED, U + 2.0)).show(ui, |ui| {
                ui.label(
                    RichText::new(format!(
                        "{}  {lost}. The show keeps running on the server; this window is not in control.",
                        icon::PLUGS
                    ))
                    .font(semibold(BODY))
                    .color(egui::Color32::WHITE),
                );
            });
        }

        let before =
            (serde_json::to_string(&self.dock).unwrap_or_default(), serde_json::to_string(&self.lower).unwrap_or_default());
        // The rig controller is not drawn while Show settings is open, so it does not have the keyboard.
        self.session.rig.keyboard = false;
        let settings = self.session.settings;
        // Over the strip, the viewers; under it, the timeline, as tall as the viewers to start with. They
        // are drawn top to bottom, as they read. Show settings takes the place of both; the strip stays,
        // at the foot of the window.
        let strip = |ui: &mut Ui, shell: &mut Shell, panel: egui::Panel| {
            let transport =
                panel.show_separator_line(false).frame(bar(PANEL, U + 2.0)).show(ui, |ui| shell.transport(ui)).response.rect;
            shell.transport_rect = transport;
            // A rule over the strip and one under it, in the machine's state, right across the window.
            let rule = machine_rule(&shell.session);
            let top = egui::Rect::from_min_max(transport.min, egui::pos2(transport.right(), transport.top() + RULE_STRONG));
            let bottom = egui::Rect::from_min_max(egui::pos2(transport.left(), transport.bottom() - RULE_STRONG), transport.max);
            ui.painter().rect_filled(top, 0.0, rule);
            ui.painter().rect_filled(bottom, 0.0, rule);
        };
        if let Some(mut page) = settings {
            strip(ui, self, egui::Panel::bottom("transport_bar"));
            crate::settings_ui::area(ui, &mut self.session, &mut page);
            self.session.settings = Some(page);
        } else {
            let half = ((ui.available_height() - TRANSPORT_H) / 2.0).max(120.0);
            egui::Panel::top("upper_dock")
                .resizable(true)
                .show_separator_line(false)
                .default_size(half)
                .min_size(120.0)
                .frame(egui::Frame::new().fill(BASE))
                .show(ui, |ui| self.panels(ui));
            strip(ui, self, egui::Panel::top("transport_bar"));
            egui::CentralPanel::default().frame(egui::Frame::new().fill(BASE)).show(ui, |ui| self.lower_panels(ui));
        }
        if (serde_json::to_string(&self.dock).unwrap_or_default(), serde_json::to_string(&self.lower).unwrap_or_default())
            != before
        {
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

    /// The tab in front of each group, over and under the strip, which the tab strips mark.
    fn front(&self) -> Vec<Tab> {
        [&self.dock, &self.lower]
            .into_iter()
            .flat_map(|d| d.iter_surfaces().flat_map(|surface| surface.iter_nodes()))
            .filter_map(|node| node.get_leaf())
            .filter_map(|leaf| leaf.tabs.get(leaf.active.0).copied())
            .collect()
    }

    fn panels(&mut self, ui: &mut Ui) {
        let front = self.front();
        let style = dock_style(ui.style());
        let mut viewer = Viewer { session: &mut self.session, front };
        DockArea::new(&mut self.dock)
            .id(egui::Id::new("upper_dock"))
            .style(style)
            .show_close_buttons(false)
            .show_add_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut viewer);
    }

    fn lower_panels(&mut self, ui: &mut Ui) {
        let front = self.front();
        let style = dock_style(ui.style());
        let mut viewer = Viewer { session: &mut self.session, front };
        DockArea::new(&mut self.lower)
            .id(egui::Id::new("lower_dock"))
            .style(style)
            .show_close_buttons(false)
            .show_add_buttons(false)
            .show_leaf_close_all_buttons(false)
            .show_leaf_collapse_buttons(false)
            .show_inside(ui, &mut viewer);
    }

    fn header(&mut self, ui: &mut Ui) {
        let updater = self.updater.clone();
        let s = &mut self.session;
        let can_edit = s.can_edit();
        let song_id = s.song.as_ref().map(|x| x.id.clone());
        let st = s.status.song.clone();
        ui.horizontal_wrapped(|ui| {
            let row = ui.cursor().top();
            ui.spacing_mut().item_spacing.x = U;
            // The mark and the name, as the brand sets them in a title bar.
            crate::brand::mark_widget(ui, 5.0 * U);
            ui.label(RichText::new(crate::brand::NAME).font(semibold(BODY)));
            ui.add_space(U);
            ui.separator();
            let parked = s.status.transport.run == Run::Stopped && s.status.transport.output == Output::Rehearse;
            let need_lease = can_edit.clone().err().filter(|_| !s.status.you_hold_lease || s.lost.is_some());
            let gate = |ui: &mut Ui, enabled: bool, glyph: &str, label: &str, why: Option<String>, tip: &str| -> bool {
                let r = icon_button(ui, enabled, glyph, label, Look::Quiet, vec2(0.0, CONTROL_H));
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
            let load_why = s.can_change_song().err();
            if gate(ui, load_why.is_none(), icon::FOLDER_OPEN, "Load", load_why, "Open another song") {
                s.link.send(Cmd::RefreshSongs);
                self.dialog = Dialog::Load;
            }
            let dirty = st.as_ref().is_some_and(|x| x.dirty);
            let has = song_id.is_some();
            let save_why = need_lease
                .clone()
                .or_else(|| (!has).then(|| "No song is open".to_string()))
                .or_else(|| (!dirty).then(|| "Nothing to save".to_string()));
            if gate(ui, save_why.is_none(), icon::FLOPPY_DISK, "Save", save_why, "Save the draft  ( Cmd-S or Ctrl-S )") {
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
            ui.separator();
            ui.add_space(U);
            ui.spacing_mut().item_spacing.x = GAP;

            // The song's own fields go to a second row when the window is too narrow to hold them beside
            // what stays at the right. Both are given the width they were laid out at last.
            let rooms = egui::Id::new("header_rooms");
            let (song_room, lease_room) = ui.data(|d| d.get_temp::<(f32, f32)>(rooms)).unwrap_or_default();
            let mut first_row_ends = None;
            if song_room + lease_room > ui.available_size_before_wrap().x {
                first_row_ends = Some(ui.cursor().left());
                ui.end_row();
                ui.set_row_height(CONTROL_H);
            }
            let song_from = ui.cursor().left();

            // The timecode arriving from outside, as it arrives, while the playhead is locked to it.
            if let Some(source) = s.status.lock.source.clone().filter(|id| matches!(id.as_str(), "ltc" | "mtc" | "artnet")) {
                use ptz_core::chase::ChaseState;
                let lock = &s.status.lock;
                let (ink, state) = match lock.chase {
                    ChaseState::Locked => (PAPER, "arriving"),
                    ChaseState::Freewheel => (AMBER, "dropped out: carrying on without it"),
                    _ => (MUTED, "not arriving"),
                };
                ui.label(caps("TC in").color(MUTED));
                let font = mono(TITLE);
                let code = lock.last_code.clone().unwrap_or_else(|| "--:--:--:--".into());
                let room = ui.painter().layout_no_wrap("00:00:00:00".into(), font.clone(), ink).size();
                let (rect, r) = ui.allocate_exact_size(room, egui::Sense::hover());
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("Timecode in {code}")));
                figures(ui.painter(), rect.left_center(), egui::Align2::LEFT_CENTER, &code, font, ink);
                r.on_hover_text(format!("{}: {state}", crate::lock_ui::source_name(s, &source)));
                ui.separator();
            }

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
                        // Green is kept for live output: a committed revision is said in plain ink.
                        chip(ui, &rev, if st.committed_clean { OFF_WHITE } else { MUTED }, false);
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
                        ui.label(RichText::new(icon::WARNING).font(icon_font(TITLE)).color(AMBER));
                        let r = ui.add(
                            egui::Label::new(RichText::new(text).color(AMBER).font(semibold(BODY))).sense(egui::Sense::click()),
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

            // Who may edit, and the View menu, keep the right of the first row whatever has gone to the second.
            let song_now = ui.cursor().left() - song_from;
            let from = first_row_ends.unwrap_or(ui.cursor().left());
            let place = egui::Rect::from_min_max(egui::pos2(from, row), egui::pos2(ui.max_rect().right(), row + CONTROL_H));
            let at_the_right = egui::UiBuilder::new().max_rect(place).layout(Layout::right_to_left(Align::Center));
            let lease_now = ui.scope_builder(at_the_right, |ui| {
                let view = ui.button("View");
                // E-STOP is just beneath and is drawn above everything: the menu opens beside it, not across it.
                let clear = ui.max_rect().right() - ESTOP_GROUND.x - GAP;
                let beside =
                    egui::Rect::from_x_y_ranges(view.rect.left().min(clear)..=view.rect.right().min(clear), view.rect.y_range());
                egui::Popup::menu(&view).anchor(beside).align(egui::RectAlign::BOTTOM_END).align_alternatives(&[]).show(|ui| {
                    if ui.button("Reset layout").clicked() {
                        self.dock = default_layout();
                        self.lower = lower_layout();
                        self.layout_changed = true;
                        ui.close();
                    }
                    if ui.button("Fit timeline").clicked() {
                        s.fit_requested = true;
                        ui.close();
                    }
                    ui.separator();
                    let updates = match updater.offered() {
                        Some(version) => format!("Update to {version}"),
                        None => "Updates".to_string(),
                    };
                    if ui.button(updates).on_hover_text(format!("This is version {}", update::VERSION)).clicked() {
                        if matches!(updater.step(), Step::Newest | Step::NotChecked(_)) {
                            updater.check();
                        }
                        updater.open(true);
                        ui.close();
                    }
                });
                if updater.offered().is_some() {
                    // A newer version is waiting in the menu.
                    ui.painter().circle_filled(view.rect.right_top() + vec2(-U, U), 3.0, ACCENT);
                }
                if s.status.you_hold_lease {
                    if ui.button("Release").on_hover_text("Give up the edit lease so another station can take it").clicked() {
                        s.link.send(Cmd::ReleaseLease);
                    }
                    chip(ui, "You hold the edit lease", OFF_WHITE, false);
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
                    ui.label(RichText::new(icon::EYE).font(icon_font(TITLE)).color(AMBER));
                    ui.label(RichText::new(text).color(AMBER));
                }
                let needed = place.right() - ui.cursor().right();
                // The server's address is the first thing to give way: it is shown where there is room left for it.
                let address = egui::WidgetText::from(RichText::new(&s.link.base).color(MUTED).small()).into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Small,
                );
                if address.size().x + GAP <= ui.available_width() {
                    ui.add(egui::Label::new(address));
                }
                needed
            });
            let rooms_now = (song_now, lease_now.inner);
            if (rooms_now.0 - song_room).abs() > 0.5 || (rooms_now.1 - lease_room).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(rooms, rooms_now));
                // The rows were laid out for other widths: lay the bar out again before it is shown.
                ui.ctx().request_discard("the song header changed");
            }
        });
    }

    /// How much of the top bars the corner keeps: E-STOP, or the latch and the way out of it.
    fn corner_width(&self) -> f32 {
        let latched = if self.session.status.transport.estop { RESET_W + GAP } else { 0.0 };
        ESTOP_GROUND.x + latched + 2.0 * PAD
    }

    /// The right end of the camera bar: E-STOP, or while it is latched, the latch with the way out of it
    /// beside it.
    fn stop_corner(&mut self, ui: &mut Ui, bars: egui::Rect) {
        let s = &mut self.session;
        let at = egui::pos2(bars.right() - PAD - ESTOP_GROUND.x, bars.center().y - ESTOP_GROUND.y / 2.0);
        if !s.status.transport.estop {
            e_stop(ui.ctx(), s, egui::Rect::from_min_size(at, ESTOP_GROUND));
            return;
        }
        // The latch and the reset are part of the window, under any dialog, unlike E-STOP.
        let place = egui::Rect::from_min_max(
            egui::pos2(at.x - GAP - RESET_W, at.y),
            egui::pos2(bars.right() - PAD, at.y + ESTOP_GROUND.y),
        );
        let ctx = ui.ctx().clone();
        egui::Area::new(egui::Id::new("stop_corner")).order(egui::Order::Middle).fixed_pos(place.min).fade_in(false).show(
            &ctx,
            |ui| {
                ui.set_min_size(place.size());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    stop_latched(ui, "E-STOP LATCHED", ESTOP_GROUND, ESTOP_SIZE);
                    ui.add_space(GAP);
                    let reset = action_glyph(Action::ResetEstop);
                    action_button(ui, s, Action::ResetEstop, reset, "Reset E-Stop", Look::Solid(AMBER), RESET_W);
                });
            },
        );
    }

    fn transport(&mut self, ui: &mut Ui) {
        let s = &mut self.session;
        let t = s.status.transport;
        let flow = Flow { width: ui.available_width(), clip: ui.clip_rect() };
        ui.horizontal(|ui| {
            // The stop control is laid out first and takes its place at the right. What is left of the
            // row is all the rest may use: it goes onto further rows instead of running on under the button.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // E-STOP is not in this strip: it keeps the top right corner of the window, which nothing
                // laid out under it can reach (see [`Shell::stop_corner`]).
                crate::media_ui::meters(ui, s);
                ui.add_space(U);

                let room = ui.available_rect_before_wrap();
                ui.with_layout(Layout::left_to_right(Align::Center).with_main_wrap(true), |ui| {
                    // Should anything still be wider than the room it has, it is cut off there, not drawn over the meters and the stop.
                    ui.shrink_clip_rect(egui::Rect::everything_left_of(room.right()));
                    // Everything that runs the machine is the large size.
                    ui.spacing_mut().interact_size.y = CONTROL_LARGE;
                    let frame = s.playhead().floor() as i64;
                    let ink = clock_ink(&t, s.status.hardware_live);
                    // The main button: whatever the server says is the next step.
                    let g = s.status.guidance.clone();
                    let primary = g.primary.filter(|a| *a != Action::Estop && *a != Action::ResetEstop);
                    let live = t.output != Output::Rehearse;
                    flow.together(ui, "run", BUTTON_ROW, |ui| {
                        // The show's clock, the largest figures in the window, and under it the frame of the
                        // song and the song's length in frames.
                        let length = s.song.as_ref().map(|x| x.length_frames).unwrap_or(0);
                        // Set in Inter, each figure in a cell of its own, so that the clock holds still.
                        let tc = s.timecode_at(frame);
                        let count = format!("{:05}:{:05}", frame.max(0), length.max(0));
                        let (big, small_font) = (mono(DISPLAY + 2.0), mono(SMALL));
                        let p = ui.painter().clone();
                        let measure = |text: &str, font: FontId| {
                            figures(&p, egui::Pos2::ZERO, egui::Align2::LEFT_TOP, text, font, egui::Color32::TRANSPARENT).size()
                        };
                        let (a, b) = (measure(&tc, big.clone()), measure(&count, small_font.clone()));
                        let (rect, r) = ui.allocate_exact_size(vec2(a.x.max(b.x), a.y + b.y), egui::Sense::hover());
                        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, tc.as_str()));
                        r.on_hover_text("The timecode at the playhead; under it, the frame of the song and its length in frames");
                        figures(ui.painter(), rect.left_top(), egui::Align2::LEFT_TOP, &tc, big, ink);
                        figures(
                            ui.painter(),
                            rect.left_top() + vec2(0.0, a.y),
                            egui::Align2::LEFT_TOP,
                            &count,
                            small_font,
                            MUTED,
                        );
                        ui.add_space(PAD);
                        match primary {
                            Some(action) => {
                                let look = match action {
                                    // Going live is the brand's GO: Signal red.
                                    Action::Play if live => Look::Solid(RED),
                                    Action::Stop => Look::Solid(OFF_WHITE),
                                    _ => Look::Solid(ACCENT),
                                };
                                action_button(ui, s, action, action_glyph(action), &g.primary_label, look, 124.0);
                            }
                            None => {
                                icon_button(ui, false, icon::PLAY, "Rehearse", Look::Plain, vec2(124.0, CONTROL_LARGE))
                                    .on_disabled_hover_text(g.hint.clone());
                            }
                        }
                        if primary != Some(Action::Stop) {
                            action_button(ui, s, Action::Stop, icon::STOP, "Stop", Look::Plain, 0.0);
                        }
                        action_button(ui, s, Action::Cue, icon::SKIP_BACK, "Cue", Look::Plain, 0.0);
                    });
                    ui.add_space(GAP);

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
                    ui.add_space(GAP);

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
        // Save is both Cmd-S and Ctrl-S, on every machine: on a Mac Cmd is not Ctrl.
        if chord(Key::S).is_some_and(|m| m.command || m.ctrl) {
            if let Some(song) = &s.song {
                s.link.send(Cmd::Save(song.id.clone()));
            }
        }
        // Nothing below edits or runs the show while a text field has the keyboard. Cmd-Z there
        // is the field's own undo: it does not reach past the field into the song.
        if ctx.text_edit_focused() {
            return;
        }
        crate::camera_ui::shortcuts(&ctx, s);
        if s.settings.is_some() {
            // Esc closes Show settings, unless it is closing a list that is open over it. The song is
            // out of sight, so the keys that edit it or move through it do nothing; Space still runs
            // the transport, which is still on screen.
            if pressed(Key::Escape) && !egui::Popup::is_any_open(&ctx) {
                s.settings = None;
            }
            if pressed(Key::Space) {
                transport_key(&ctx, s);
            }
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
        if pressed(Key::Escape) {
            s.timeline.cancel();
        }
        if pressed(Key::Space) {
            transport_key(&ctx, s);
        }
        if pressed(Key::Home) && s.offers(Action::Cue) {
            s.act(Action::Cue);
        }
        // Page Down and Page Up step through the set list.
        if pressed(Key::PageDown) {
            s.step_song(1);
        }
        if pressed(Key::PageUp) {
            s.step_song(-1);
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
                    dialog_title(ui, "New song");
                    ui.add_space(U);
                    egui::Grid::new("new_grid").num_columns(2).spacing([PAD, GAP]).show(ui, |ui| {
                        field(ui, "Name");
                        ui.add(egui::TextEdit::singleline(name).desired_width(220.0));
                        ui.end_row();
                        field(ui, "Starts at");
                        ui.add(egui::TextEdit::singleline(start_tc).desired_width(120.0));
                        ui.end_row();
                    });
                    let valid = Timecode::parse_at(start_tc, s.project.rate).is_some();
                    if !valid {
                        ui.label(RichText::new(format!("Use HH:MM:SS:FF at {} fps", s.project.rate.label())).color(AMBER));
                    } else {
                        ui.label(RichText::new("The timecode at which this song's first frame plays.").color(MUTED));
                    }
                    ui.add_space(PAD);
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
                    dialog_title(ui, "Load a song");
                    ui.add_space(U);
                    if s.songs.is_empty() {
                        ui.label(RichText::new("There are no songs on this server yet. Use New to start one.").color(MUTED));
                    }
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        egui::Grid::new("load_grid").num_columns(5).striped(true).spacing([PAD + U, GAP]).show(ui, |ui| {
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
                                    let can = if song.loaded { Err("This song is open".to_string()) } else { s.can_load(&song) };
                                    let load = ui.add_enabled(can.is_ok(), egui::Button::new("Load"));
                                    if can.err().map_or(load.clone(), |why| load.on_disabled_hover_text(why)).clicked() {
                                        s.load_song(&song.id);
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
                    ui.add_space(PAD);
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            }
            Dialog::SaveAs { name } => {
                Prompt::SaveAs.show(ctx, |ui| {
                    ui.set_width(340.0);
                    dialog_title(ui, "Save As");
                    ui.label(RichText::new("Makes a copy with its own revisions and opens it.").color(MUTED));
                    ui.add_space(U);
                    ui.add(egui::TextEdit::singleline(name).desired_width(300.0));
                    ui.add_space(PAD);
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
                    dialog_title(ui, "Revert to the last Save?");
                    ui.label("Every change since the last Save is dropped. This cannot be undone.");
                    ui.add_space(PAD);
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
                    dialog_title(ui, format!("Delete {name}?"));
                    ui.label("The draft and every committed revision are removed from the server.");
                    ui.add_space(PAD);
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
                dialog_title(ui, "Read presets from the camera?");
                ui.label("The camera is recalled to each of its presets in turn, so that its pose and picture can be read. It will move, at its own speed, through every preset it holds.");
                ui.label(RichText::new("The pedestal does not move. E-Stop stops the camera and abandons the sync.").color(MUTED));
                ui.add_space(PAD);
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
                dialog_title(ui, format!("Take the edit lease from {holder}?"));
                ui.label(format!("{holder} stops being able to edit or run the show at once, and is told this station took over. The song stays as it is."));
                ui.add_space(PAD);
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
                dialog_title(ui, "Rename marker");
                let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(260.0));
                r.request_focus();
                if ui.input(|i| i.key_pressed(Key::Enter)) {
                    apply = true;
                }
                ui.add_space(U);
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

        let server_here = on_this_computer(&self.session.link.base) && self.session.lost.is_none();
        update_dialog(ctx, &self.updater, self.too_busy_to_update(), server_here);
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
                ui.label(RichText::new(icon::WARNING_OCTAGON).font(icon_font(DISPLAY)).color(RED));
                ui.heading(RichText::new(STOP_NOT_SENT).color(RED));
            });
            // The rule under this title is the stop's own red, and heavier than a hairline.
            let (rule, _) = ui.allocate_exact_size(vec2(ui.available_width(), RULE_STRONG), egui::Sense::hover());
            ui.painter().rect_filled(rule, 0.0, RED);
            ui.add_space(U);
            ui.label(RichText::new(STOP_NOT_SENT_DO).font(semibold(BODY)));
            ui.add_space(U);
            ui.label(RichText::new(format!("What went wrong: {}", why.trim_end_matches('.'))).color(MUTED));
            ui.label(if trying { STOP_STILL_TRYING } else { STOP_GAVE_UP });
            ui.add_space(PAD);
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
            dialog_title(ui, format!("{} starts at {file_start}", info.name));
            ui.add_space(U);
            ui.label(format!(
                "That is outside {}, which runs from {} to {}.",
                song.name,
                s.timecode_at(0),
                s.timecode_at(song.length_frames)
            ));
            ui.add_space(U);
            ui.label(
                RichText::new(
                    "Setting the song start puts the file on its own timecode at the first frame. \
                     Clips already on the timeline keep their places and take new timecodes.",
                )
                .color(MUTED),
            );
            ui.add_space(PAD);
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
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-4.0 * U, -4.0 * U))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                ui.with_layout(Layout::bottom_up(Align::RIGHT), |ui| {
                    for toast in self.session.toasts.iter().rev() {
                        // A plate with a bar down its leading edge: amber for what was refused, the accent for news.
                        let edge = if toast.warn { AMBER } else { ACCENT };
                        let plate = egui::Frame::new()
                            .fill(PANEL_DARK)
                            .stroke(Stroke::new(RULE, CONTROL_LINE))
                            .inner_margin(egui::Margin {
                                left: (PAD + U) as i8,
                                right: PAD as i8,
                                top: GAP as i8,
                                bottom: GAP as i8,
                            })
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                let line = vec2(ui.available_width(), ui.text_style_height(&egui::TextStyle::Body));
                                ui.allocate_ui_with_layout(line, Layout::left_to_right(Align::Min), |ui| {
                                    if toast.warn {
                                        ui.label(RichText::new(icon::WARNING).font(icon_font(TITLE)).color(AMBER));
                                    }
                                    ui.add(egui::Label::new(&toast.text).wrap());
                                });
                            })
                            .response
                            .rect;
                        ui.painter().rect_filled(plate.with_max_x(plate.left() + U), 0.0, edge);
                    }
                });
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

/// A layout saved by an earlier version lacks the tabs added since: they join the Program monitor's group,
/// or the set list the Presets group, and the arrangement the operator made is kept.
/// A layout saved before the transport strip went between the viewers and the timeline: the timeline's
/// tabs are taken out of it, for the panels under the strip.
fn without_lower(mut state: DockState<Tab>) -> DockState<Tab> {
    for tab in LOWER {
        if let Some(path) = state.find_tab(&tab) {
            state.remove_tab(path);
        }
    }
    state
}

fn with_every_tab(mut state: DockState<Tab>) -> DockState<Tab> {
    for tab in Tab::ALL.into_iter().filter(|t| !LOWER.contains(t)) {
        if state.find_tab(&tab).is_some() {
            continue;
        }
        let home = if tab == Tab::SetList { Tab::Presets } else { Tab::Program };
        let beside = state.find_tab(&home).or_else(|| state.iter_all_tabs().next().map(|(path, _)| path));
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
    updater: Updater,
    /// The app was reopened by an update that stopped the show server on this computer: start it again.
    start_server: bool,
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
            updater: Updater::of_this_build(repaint.clone()),
            repaint,
            last_layout_save: std::time::Instant::now(),
            have_server: None,
            start_server: std::env::args().any(|arg| arg == update::START_SERVER),
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
        // Which version this is, and a newer one when there is one, along the bottom of the window.
        let updater = self.updater.clone();
        egui::Panel::bottom("version")
            .show_separator_line(false)
            .frame(egui::Frame::new().fill(PANEL_DARK).inner_margin(PAD))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Version {}", update::VERSION)).color(MUTED).small());
                    if let Some(version) = updater.offered() {
                        if ui.button(format!("Update to {version}")).clicked() {
                            updater.open(true);
                        }
                    }
                });
            });
        // No show is connected, so nothing is running that this window would be leaving.
        update_dialog(ui.ctx(), &updater, None, false);
        let (mut go, mut start_local) = connect_form(ui, &mut self.settings, error.as_deref(), busy, have_server);
        // Reopened by an update that stopped the server here, and it is not answering: start the new one.
        if error.is_some() && std::mem::take(&mut self.start_server) && have_server && on_this_computer(&self.settings.server) {
            start_local = true;
        }
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

/// The connect screen: where the show server is and who this station is. Returns whether Connect
/// was asked for, and whether a server is to be started on this computer first.
pub(crate) fn connect_form(
    ui: &mut Ui,
    settings: &mut Settings,
    error: Option<&str>,
    busy: bool,
    have_server: bool,
) -> (bool, bool) {
    let mut go = false;
    let mut start_local = false;
    egui::CentralPanel::default().frame(egui::Frame::new().fill(PANEL_DARK)).show(ui, |ui| {
        // One column, set flush left, in the middle of the window: the name, a rule, the form, a rule, what to do.
        let column = 460.0_f32.min(ui.available_width() - 2.0 * PAD);
        let top = (ui.available_height() * 0.18).max(5.0 * U);
        let place = egui::Rect::from_min_size(
            ui.max_rect().min + vec2((ui.available_width() - column) / 2.0, top),
            vec2(column, ui.available_height() - top),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(place).layout(Layout::top_down(Align::LEFT)), |ui| {
            // The primary lockup, flush left, and the descriptor line under a rule.
            let lockup_h = 18.0 * U;
            match crate::brand::lockup(ui.ctx()) {
                Some(t) => {
                    let size = t.size_vec2() * (lockup_h / t.size_vec2().y);
                    let (rect, r) = ui.allocate_exact_size(size, egui::Sense::hover());
                    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Image, true, crate::brand::NAME));
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    ui.painter().image(t.id(), rect, uv, egui::Color32::WHITE);
                }
                None => {
                    ui.label(RichText::new(crate::brand::NAME).font(semibold(HERO)));
                }
            }
            ui.add_space(PAD);
            hairline(ui);
            ui.add_space(U);
            ui.label(caps(crate::brand::DESCRIPTOR).color(MUTED));
            ui.add_space(PAD);
            hairline(ui);
            ui.add_space(PAD);
            {
                egui::Grid::new("connect_grid").num_columns(2).spacing([4.0 * U, GAP]).show(ui, |ui| {
                    field(ui, "Show server");
                    let r = ui.add_enabled(
                        !busy,
                        egui::TextEdit::singleline(&mut settings.server)
                            .desired_width(290.0)
                            .hint_text("http://127.0.0.1:8787"),
                    );
                    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        go = true;
                    }
                    ui.end_row();
                    let others: Vec<String> =
                        settings.recent.iter().filter(|r| r.as_str() != settings.server.trim()).cloned().collect();
                    if !others.is_empty() {
                        ui.label(RichText::new("Recent").color(MUTED));
                        ui.vertical(|ui| {
                            for url in others {
                                let link = ui.add_enabled(!busy, egui::Button::new(&url).frame(false));
                                if link.on_hover_text("Use this server").clicked() {
                                    settings.server = url;
                                }
                            }
                        });
                        ui.end_row();
                    }
                    field(ui, "Station name");
                    ui.add_enabled(!busy, egui::TextEdit::singleline(&mut settings.station).desired_width(290.0))
                        .on_hover_text("Shown to other stations when this one holds the edit lease");
                    ui.end_row();
                    field(ui, "Access key");
                    ui.add_enabled(
                        !busy,
                        egui::TextEdit::singleline(&mut settings.access_key)
                            .password(true)
                            .desired_width(290.0)
                            .hint_text("only for a remote server"),
                    );
                    ui.end_row();
                });
                ui.add_space(PAD);
                hairline(ui);
                ui.add_space(PAD);
                ui.horizontal(|ui| {
                    if busy {
                        ui.spinner();
                        ui.label("Connecting");
                    } else {
                        let ok = !settings.server.trim().is_empty() && !settings.station.trim().is_empty();
                        if ui.add_enabled(ok, filled("Connect", ACCENT).min_size(vec2(120.0, CONTROL_LARGE))).clicked() {
                            go = true;
                        }
                        if ui
                            .add_enabled(
                                have_server,
                                egui::Button::new("Start a server on this computer").min_size(vec2(0.0, CONTROL_LARGE)),
                            )
                            .on_hover_text("Runs ptz-server here on the show folder in your home folder, then connects to it. It drives the rig that project names, the simulator on a new folder, and keeps running after the app quits")
                            .on_disabled_hover_text(link::NO_LOCAL_SERVER)
                            .clicked()
                        {
                            start_local = true;
                        }
                    }
                });
                if let Some(e) = error {
                    ui.add_space(PAD);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(icon::WARNING).font(icon_font(TITLE)).color(AMBER));
                        ui.label(RichText::new(e).color(AMBER));
                    });
                }
            }
        });
    });
    (go, start_local)
}

impl eframe::App for PtzApp {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.updater.tick();
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
                        let mut shell = Shell::new(session, self.settings.layout.take());
                        if let Some(lower) =
                            self.settings.lower_layout.take().filter(|l| LOWER.iter().all(|t| l.find_tab(t).is_some()))
                        {
                            shell.lower = lower;
                        }
                        shell.updater = self.updater.clone();
                        self.start_server = false;
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
                    self.settings.lower_layout = Some(shell.lower.clone());
                    self.settings.save();
                    self.settings.layout = None;
                    self.settings.lower_layout = None;
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
            self.settings.lower_layout = Some(shell.lower.clone());
            self.settings.save();
        }
    }
}
