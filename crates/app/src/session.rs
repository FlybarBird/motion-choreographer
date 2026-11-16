//! Everything the window knows about one connected server: the last status,
//! the song being edited, undo history and selection.

use crate::link::{Cmd, Event, Link, Why};
use crate::timeline::Timeline;
use ptz_core::api::*;
use ptz_core::camera::CameraConfig;
use ptz_core::compile::{compile, Compiled, Issue};
use ptz_core::edit::Refusal;
use ptz_core::eval;
use ptz_core::pose::{Pose, Preset, Project};
use ptz_core::song::Song;
use ptz_core::transport::{Action, Output, Run};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct Toast {
    pub text: String,
    pub warn: bool,
    pub until: Instant,
}

/// A file whose own timecode falls outside the open song.
#[derive(Clone, Debug, PartialEq)]
pub struct StampedDrop {
    pub media: MediaInfo,
    /// Where the pointer let go, for "place it here".
    pub at_frame: i64,
}

pub enum PresetPicture {
    Asked,
    Decoded(egui::ColorImage),
    Shown(egui::TextureHandle),
}

pub struct Session {
    pub link: Link,
    pub station: String,
    /// Why the connection is down, while it is.
    pub lost: Option<String>,
    /// An E-Stop pressed here that the server has not taken: why, and whether it is still being sent.
    pub stop_not_sent: Option<(String, bool)>,
    pub status: Status,
    status_at: Instant,
    pub project: Project,
    /// The camera the monitor, presets, rig controller and Record are showing.
    pub active_camera: String,
    pub presets: Vec<Preset>,
    pub songs: Vec<SongSummary>,
    pub media: Vec<MediaInfo>,
    pub song: Option<Song>,
    pub compiled: Compiled,
    counter: u64,
    put_seq: u64,
    acked_seq: u64,
    fetching: bool,
    pub undo: Vec<Song>,
    pub redo: Vec<Song>,
    pub timeline: Timeline,
    pub fit_requested: bool,
    scrub: Option<(i64, Instant)>,
    pub toasts: Vec<Toast>,
    pub log: Vec<LogRecord>,
    /// Waveform overview per media id, at the level last asked for.
    pub peaks: HashMap<String, PeakLevel>,
    /// The level asked for per media id, so each is requested once.
    pub peaks_requested: HashMap<String, u32>,
    /// Proxy frames of reference video.
    pub frames: crate::frames::Frames,
    /// The picture last shown in the reference monitor, held while the next one loads.
    pub reference_shown: Option<(String, egui::TextureHandle)>,
    /// Audio inputs and MIDI ports on the server machine, once asked for.
    pub inputs: Option<InputChoices>,
    /// A stamped file dropped where its timecode is outside the song, waiting for the operator's choice.
    pub stamped_drop: Option<StampedDrop>,
    /// Axes Record writes, chosen on the axis lane headers.
    pub record_axes: Vec<ptz_core::Axis>,
    /// Axis lanes selected for `K`.
    pub axis_lanes: std::collections::BTreeSet<ptz_core::Axis>,
    pub graph: crate::graph::Graph,
    pub rig: crate::rig_ui::Controller,
    /// The stage as it is drawn: the model, the scene made of it, and the pictures.
    pub world: crate::stage::World,
    pub stage_view: crate::stage_ui::View,
    /// A model file chosen in the Inspector, for the window to import.
    pub stage_import_wanted: Option<std::path::PathBuf>,
    /// Where the Multiview drew each camera's tile last frame, for the UI tests.
    /// Show settings, and the page it is on, while it is open in place of the panels.
    pub settings: Option<crate::settings_ui::Page>,
    pub probe_tiles: Vec<(String, egui::Rect)>,
    /// The camera's own picture of each preset that has one.
    pub preset_pictures: HashMap<String, PresetPicture>,
    /// "Sync from rig" was pressed on a real rig and waits for the operator to confirm the move.
    pub confirm_sync: bool,
    /// "Take over" was pressed while another station holds the lease and waits for a second click.
    pub confirm_takeover: bool,
    /// Where things were drawn last frame, for the UI tests.
    pub probe_timeline: Option<egui::Rect>,
    pub probe_presets: Vec<(String, egui::Rect)>,
    pub probe_media: Vec<(String, egui::Rect)>,
    /// The last coalescing edit, so a dragged value is one undo step.
    last_edit: Option<(String, Instant)>,
    /// The server refused the last edit: its copy replaces the local one without an undo step.
    drop_local: bool,
}

#[cfg(test)]
fn idle_status() -> Status {
    Status {
        server_time_ms: 0,
        song: None,
        transport: ptz_core::transport::TransportState::IDLE,
        frame: 0,
        frame_f: 0.0,
        timecode: "00:00:00:00".into(),
        rate: ptz_core::FrameRate::F25,
        cameras: Vec::new(),
        hardware_live: false,
        lease: None,
        you_hold_lease: false,
        guidance: Default::default(),
        audio: Default::default(),
        lock: Default::default(),
        record: Default::default(),
        telemetry: Default::default(),
        panel: None,
        panel_host: None,
    }
}

impl Session {
    pub fn new(link: Link, station: &str, hello: crate::link::Hello) -> Session {
        let mut s = Session {
            link,
            station: station.to_string(),
            lost: None,
            stop_not_sent: None,
            status: hello.status,
            status_at: Instant::now(),
            active_camera: hello.project.first().id.clone(),
            project: hello.project,
            presets: hello.presets,
            songs: hello.songs,
            media: hello.media,
            song: None,
            compiled: Compiled::default(),
            counter: 0,
            put_seq: 0,
            acked_seq: 0,
            fetching: false,
            undo: Vec::new(),
            redo: Vec::new(),
            timeline: Timeline::default(),
            fit_requested: true,
            scrub: None,
            toasts: Vec::new(),
            log: hello.log,
            peaks: HashMap::new(),
            peaks_requested: Default::default(),
            frames: Default::default(),
            reference_shown: None,
            inputs: None,
            stamped_drop: None,
            record_axes: vec![ptz_core::Axis::Pan, ptz_core::Axis::Tilt],
            axis_lanes: Default::default(),
            graph: Default::default(),
            rig: Default::default(),
            world: Default::default(),
            stage_view: Default::default(),
            stage_import_wanted: None,
            settings: None,
            probe_tiles: Vec::new(),
            preset_pictures: HashMap::new(),
            confirm_sync: false,
            confirm_takeover: false,
            probe_timeline: None,
            probe_presets: Vec::new(),
            probe_media: Vec::new(),
            last_edit: None,
            drop_local: false,
        };
        if let Some(w) = hello.working {
            s.open(w);
        }
        s
    }

    /// A session with no server, for tests.
    #[cfg(test)]
    pub fn offline(link: Link, project: Project, presets: Vec<Preset>, song: Song) -> Session {
        let mut status = idle_status();
        status.you_hold_lease = true;
        status.rate = project.rate;
        status.cameras = project.cameras.iter().map(|c| CameraStatus { id: c.id.clone(), ..Default::default() }).collect();
        status.guidance = ptz_core::transport::guidance(&status.transport, &ptz_core::transport::Context::ready());
        let hello = crate::link::Hello {
            status,
            project,
            presets,
            songs: Vec::new(),
            media: Vec::new(),
            working: Some(WorkingSong { song, edit_counter: 0, dirty: false, revision: None, issues: Vec::new() }),
            log: Vec::new(),
        };
        Session::new(link, "Test", hello)
    }

    fn open(&mut self, w: WorkingSong) {
        self.counter = w.edit_counter;
        self.compiled = compile(&w.song, &self.project);
        self.song = Some(w.song);
        self.undo.clear();
        self.redo.clear();
        let view = self.timeline.view;
        self.timeline = Timeline::default();
        self.timeline.view = view;
        self.fit_requested = true;
    }

    // ---- cameras ----

    /// The selected camera's id. Falls back to the first when the selected one has left the project.
    pub fn cam_id(&self) -> &str {
        match self.project.camera(&self.active_camera) {
            Some(camera) => &camera.id,
            None => &self.project.first().id,
        }
    }

    pub fn cam_index(&self) -> usize {
        self.project.camera_index(&self.active_camera).unwrap_or(0)
    }

    pub fn cam(&self) -> &CameraConfig {
        &self.project.cameras[self.cam_index()]
    }

    /// What the server last said about the selected camera.
    pub fn cam_status(&self) -> &CameraStatus {
        self.status.camera(self.cam_id()).unwrap_or_else(|| self.status.first())
    }

    pub fn select_camera(&mut self, id: &str) {
        if self.project.camera(id).is_some() && self.active_camera != id {
            crate::placement_ui::cancel_drag(self);
            self.active_camera = id.to_string();
        }
    }

    /// Presets of the selected camera, in the order they were stored.
    pub fn camera_presets(&self) -> Vec<Preset> {
        let id = self.cam_id();
        self.presets.iter().filter(|p| p.camera == id).cloned().collect()
    }

    /// Change project settings: shown at once, and sent. The server's answer replaces it.
    pub fn put_project(&mut self, project: Project) {
        if project == self.project {
            return;
        }
        self.project = project.clone();
        if self.project.camera(&self.active_camera).is_none() {
            self.active_camera = self.project.first().id.clone();
        }
        if let Some(song) = &self.song {
            self.compiled = compile(song, &self.project);
        }
        self.link.send(Cmd::PutProject(Box::new(project)));
    }

    /// Drop pictures of presets and cameras that are no longer there.
    fn prune_pictures(&mut self) {
        let (presets, cameras) = (&self.presets, &self.project.cameras);
        self.world.keep_pictures(|name| match name.split_once(':') {
            Some(("preset", id)) => presets.iter().any(|p| p.id == id),
            Some(("multi", id)) => cameras.iter().any(|c| c.id == id),
            _ => true,
        });
    }

    pub fn toast(&mut self, text: impl Into<String>, warn: bool) {
        let text = text.into();
        self.toasts.retain(|t| t.text != text);
        let secs = if warn { 6 } else { 3 };
        self.toasts.push(Toast { text, warn, until: Instant::now() + Duration::from_secs(secs) });
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }

    fn pending(&self) -> bool {
        self.put_seq != self.acked_seq
    }

    /// Drain everything the link threads have sent since the last frame.
    pub fn poll(&mut self) {
        while let Ok(event) = self.link.rx.try_recv() {
            self.on_event(event);
        }
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        if let Some(revision) = self.world.sync(&self.project.stage) {
            self.link.send(Cmd::FetchStageModel(revision));
        }
    }

    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::Connected(_) | Event::ConnectFailed(_) => {}
            Event::Disconnected(msg) => self.lost = Some(msg),
            Event::Reconnected => {
                self.lost = None;
                self.toast("Reconnected", false);
            }
            Event::Status(status) => {
                let had = self.status.you_hold_lease;
                self.status = *status;
                if had && !self.status.you_hold_lease {
                    if let Some(l) = &self.status.lease {
                        let text =
                            format!("{} took the edit lease. This station is watching; nothing in the song was lost", l.holder);
                        self.toast(text, true);
                    }
                }
                if self.status.you_hold_lease || self.status.lease.is_none() {
                    self.confirm_takeover = false;
                }
                self.status_at = Instant::now();
                self.lost = None;
                if self.status.transport.estop {
                    self.stop_not_sent = None;
                }
                self.reconcile();
            }
            Event::Working(w, why) => {
                self.fetching = false;
                let same = self.song.as_ref().is_some_and(|s| s.id == w.song.id);
                match why {
                    Why::Flags if same => self.counter = w.edit_counter,
                    Why::Flags => {}
                    Why::Open => self.open(*w),
                    Why::Reload => {
                        if same && !self.pending() {
                            self.counter = w.edit_counter;
                            if self.song.as_ref() != Some(&w.song) {
                                if self.drop_local {
                                    self.undo.pop();
                                } else if let Some(old) = self.song.take() {
                                    // A change made on the server (a record pass, another window) is one undo step.
                                    self.undo.push(old);
                                    self.redo.clear();
                                }
                                self.compiled = compile(&w.song, &self.project);
                                self.song = Some(w.song);
                                self.timeline.cancel();
                                if let Some(song) = &self.song {
                                    self.timeline.prune(song);
                                }
                            }
                        } else if !same {
                            self.open(*w);
                        }
                        self.drop_local = false;
                    }
                }
            }
            Event::PutDone { seq, working } => {
                self.acked_seq = self.acked_seq.max(seq);
                self.counter = working.edit_counter;
            }
            Event::PutRefused { seq, msg } => {
                self.acked_seq = self.put_seq.max(seq);
                self.drop_local = true;
                self.toast(format!("The edit was not accepted: {msg}"), true);
            }
            Event::Songs(songs) => self.songs = songs,
            Event::Presets(presets) => {
                // A sync can replace a picture under the same preset: ask again.
                self.preset_pictures.clear();
                self.presets = presets;
                self.prune_pictures();
            }
            Event::SyncAccepted(presets) => {
                if self.cam_status().hardware {
                    self.toast("Reading presets from the camera. It moves to each one in turn".to_string(), false);
                } else {
                    let count = presets.iter().filter(|p| p.camera == self.cam_id()).count();
                    self.toast(format!("Synced {count} presets from the rig"), false);
                    self.preset_pictures.clear();
                    self.presets = presets;
                }
            }
            Event::Project(project) => {
                // A server refresh supersedes any unfinished local placement preview.
                // Do not restore its old mount over the authoritative project later.
                self.stage_view.preview_drag = None;
                self.project = *project;
                if self.project.camera(&self.active_camera).is_none() {
                    self.active_camera = self.project.first().id.clone();
                }
                if let Some(song) = &self.song {
                    self.compiled = compile(song, &self.project);
                }
                self.prune_pictures();
            }
            Event::Media(media) => {
                // A file that changed or went away loses its cached waveform.
                let same = |id: &String| media.iter().find(|m| &m.id == id) == self.media.iter().find(|m| &m.id == id);
                self.peaks.retain(|id, _| same(id));
                self.peaks_requested.retain(|id, _| same(id));
                self.frames.retain_media(|id| same(&id.to_string()));
                self.media = media;
            }
            Event::Peaks { media, level } => {
                self.peaks.insert(media, level);
            }
            Event::Frame { key, image } => self.frames.store(key, image),
            Event::Inputs(choices) => self.inputs = Some(choices),
            Event::PresetThumb { id, image } => {
                self.preset_pictures.insert(id, PresetPicture::Decoded(image));
            }
            Event::Committed(info) => self.toast(format!("Committed revision {}", info.number), false),
            Event::Uploaded(info) => {
                self.toast(format!("Imported {}", info.name), false);
                self.link.send(Cmd::RefreshMedia);
            }
            Event::StageModel { revision, mesh } => self.world.set_model(revision, mesh),
            Event::StageFetchFailed { revision, msg } => {
                self.world.fetch_failed(revision);
                self.toast(format!("The stage model could not be fetched: {msg}. Trying again"), true);
            }
            Event::StageImport(import) => {
                use crate::stage::Import;
                match &import {
                    Import::Done(name) => self.toast(format!("Imported {name}"), false),
                    Import::Failed { name, why } => self.toast(format!("{name} was not imported: {why}"), true),
                    Import::Reading(_) | Import::Uploading(_) => {}
                }
                self.world.import = Some(import);
            }
            Event::Log(record) => {
                self.log.push(record);
                if self.log.len() > 2000 {
                    self.log.drain(..500);
                }
            }
            Event::Refused { what, msg } => self.toast(format!("{what}: {msg}"), true),
            Event::StopNotSent { why, trying } => self.stop_not_sent = Some((why, trying)),
            Event::Info(text) => self.toast(text, false),
        }
    }

    /// Follow the server's loaded song, and pick up changes made there.
    fn reconcile(&mut self) {
        if self.fetching || self.pending() {
            return;
        }
        match (&self.status.song, &self.song) {
            (Some(remote), Some(local)) if remote.id == local.id => {
                if remote.edit_counter != self.counter {
                    self.fetching = true;
                    self.link.send(Cmd::Fetch(remote.id.clone()));
                }
            }
            (Some(remote), _) => {
                self.fetching = true;
                self.link.send(Cmd::Fetch(remote.id.clone()));
            }
            (None, Some(_)) => {
                self.song = None;
                self.compiled = Compiled::default();
            }
            (None, None) => {}
        }
    }

    // ---- editing ----

    pub fn can_edit(&self) -> Result<(), String> {
        if self.lost.is_some() {
            Err("The server is not reachable".into())
        } else if !self.status.you_hold_lease {
            Err(match &self.status.lease {
                Some(l) => format!("{} holds the edit lease. Take it over to edit", l.holder),
                None => "Take the edit lease to edit".into(),
            })
        } else if self.status.transport.output != Output::Rehearse {
            Err("Disarm to edit the song".into())
        } else {
            Ok(())
        }
    }

    /// Why another song cannot be loaded now. Armed is no reason: the rig travels to the new song.
    pub fn can_change_song(&self) -> Result<(), String> {
        if self.lost.is_some() {
            Err("The server is not reachable".into())
        } else if !self.status.you_hold_lease {
            Err(match &self.status.lease {
                Some(l) => format!("{} holds the edit lease. Take it over to change songs", l.holder),
                None => "Take the edit lease to change songs".into(),
            })
        } else if self.status.transport.run != ptz_core::transport::Run::Stopped {
            Err("Stop to change songs".into())
        } else {
            Ok(())
        }
    }

    /// Why this song cannot be loaded now. An armed show takes only a song it could have been armed with.
    pub fn can_load(&self, song: &ptz_core::api::SongSummary) -> Result<(), String> {
        self.can_change_song()?;
        if self.status.transport.output != Output::Rehearse && (song.revision.is_none() || song.dirty) {
            return Err(format!("{} is not committed. Commit it, or disarm, to load it", song.name));
        }
        Ok(())
    }

    pub fn load_song(&mut self, id: &str) {
        self.link.send(Cmd::LoadSong(id.to_string()));
    }

    /// Load the next song of the set list, or with -1 the one before. False when there is none to go to.
    pub fn step_song(&mut self, by: i64) -> bool {
        let loaded = self.status.song.as_ref().map(|x| x.id.as_str());
        let Some(id) = self.project.set_neighbour(loaded, by).map(str::to_string) else { return false };
        let can = self.songs.iter().find(|x| x.id == id).map(|x| self.can_load(x)).unwrap_or_else(|| self.can_change_song());
        if can.is_ok() {
            self.load_song(&id);
        }
        can.is_ok()
    }

    fn push(&mut self, song: Song) {
        self.compiled = compile(&song, &self.project);
        self.timeline.prune(&song);
        self.put_seq += 1;
        self.link.send(Cmd::PutWorking { seq: self.put_seq, song: song.clone() });
        self.song = Some(song);
    }

    /// Like `edit`, but repeated edits with the same key inside a second share one undo step.
    pub fn edit_coalesced(&mut self, key: &str, op: impl FnOnce(&Song, &Project) -> Result<Song, Refusal>) -> bool {
        let joined = self.last_edit.as_ref().is_some_and(|(k, at)| k == key && at.elapsed() < Duration::from_secs(1));
        let depth = self.undo.len();
        let ok = self.edit(op);
        if ok {
            if joined && self.undo.len() > depth {
                self.undo.pop();
            }
            self.last_edit = Some((key.to_string(), Instant::now()));
        }
        ok
    }

    /// Apply a finished edit: one undo step, sent to the server.
    pub fn commit_edit(&mut self, song: Song) {
        if self.song.as_ref() == Some(&song) {
            return;
        }
        self.last_edit = None;
        if let Err(why) = self.can_edit() {
            self.toast(why, true);
            return;
        }
        if let Some(old) = self.song.take() {
            self.undo.push(old);
            if self.undo.len() > 200 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
        self.push(song);
    }

    /// Run one core edit operation on the open song.
    pub fn edit(&mut self, op: impl FnOnce(&Song, &Project) -> Result<Song, Refusal>) -> bool {
        let Some(song) = &self.song else { return false };
        if let Err(why) = self.can_edit() {
            self.toast(why, true);
            return false;
        }
        match op(song, &self.project) {
            Ok(next) => {
                self.commit_edit(next);
                true
            }
            Err(r) => {
                self.toast(r.to_string(), true);
                false
            }
        }
    }

    pub fn undo(&mut self) {
        if let Err(why) = self.can_edit() {
            self.toast(why, true);
            return;
        }
        if let Some(prev) = self.undo.pop() {
            if let Some(cur) = self.song.take() {
                self.redo.push(cur);
            }
            self.push(prev);
        }
    }

    pub fn redo(&mut self) {
        if let Err(why) = self.can_edit() {
            self.toast(why, true);
            return;
        }
        if let Some(next) = self.redo.pop() {
            if let Some(cur) = self.song.take() {
                self.undo.push(cur);
            }
            self.push(next);
        }
    }

    pub fn selected_ids(&self) -> Vec<String> {
        self.timeline.selection.iter().cloned().collect()
    }

    pub fn issues_for(&self, clip: &str) -> Vec<&Issue> {
        self.compiled.issues.iter().filter(|i| i.clip_id() == Some(clip)).collect()
    }

    // ---- transport ----

    /// The axes Record writes: the server's while Record is on, this window's choice otherwise.
    pub fn record_axes_shown(&self) -> &[ptz_core::Axis] {
        if self.status.transport.record {
            &self.status.record.axes
        } else {
            &self.record_axes
        }
    }

    pub fn toggle_record_axis(&mut self, axis: ptz_core::Axis) {
        if self.status.transport.record {
            self.toast("Turn Record off to change which axes it writes", true);
            return;
        }
        if self.record_axes.contains(&axis) {
            self.record_axes.retain(|a| *a != axis);
        } else {
            self.record_axes.push(axis);
            self.record_axes.sort();
        }
    }

    pub fn act(&mut self, action: Action) {
        if action == Action::RecordOn && self.record_axes.is_empty() {
            self.timeline.view.set_open(self.cam_index(), true);
            self.toast("Record-enable an axis first: click the red dot on an axis lane", true);
            return;
        }
        let req = match action {
            Action::RecordOn => TransportRequest {
                axes: Some(self.record_axes.clone()),
                camera: Some(self.cam_id().to_string()),
                ..Default::default()
            },
            _ => TransportRequest::default(),
        };
        self.link.send(Cmd::Act(action, req));
    }

    /// May this station ask for `action` now? The one rule for a button and for the key that stands for it.
    /// E-Stop is always offered: it is sent even to a server that seems out of reach.
    pub fn offers(&self, action: Action) -> bool {
        action == Action::Estop || (self.status.guidance.allows(action) && self.lost.is_none())
    }

    pub fn seek(&mut self, frame: i64) {
        // Nothing is queued up for a server that cannot be reached, to arrive whenever it comes back.
        if self.lost.is_some() {
            return;
        }
        let length = self.song.as_ref().map(|s| s.length_frames).unwrap_or(0);
        let frame = frame.clamp(0, length);
        self.scrub = Some((frame, Instant::now()));
        self.link.send(Cmd::Act(Action::Seek, TransportRequest { frame: Some(frame), ..Default::default() }));
    }

    /// The playhead in frames, advanced locally between status messages so it moves smoothly.
    pub fn playhead(&self) -> f64 {
        if let Some((frame, at)) = self.scrub {
            let settling = at.elapsed() < Duration::from_millis(400) && self.status.frame != frame;
            if self.timeline.is_scrubbing() || settling {
                return frame as f64;
            }
        }
        let length = self.song.as_ref().map(|s| s.length_frames).unwrap_or(0) as f64;
        let mut f = self.status.frame_f;
        if self.status.transport.run == Run::Playing && self.lost.is_none() {
            let ahead = self.status_at.elapsed().as_secs_f64().min(0.5);
            f += self.status.rate.seconds_to_frames(ahead);
        }
        f.clamp(0.0, length.max(0.0))
    }

    pub fn is_playing(&self) -> bool {
        self.status.transport.run != Run::Stopped
    }

    /// The pose a camera is drawn at. While it follows the timeline it is evaluated
    /// here, so scrubbing and edits show without a round trip.
    pub fn camera_pose(&self, camera: &str) -> (Pose, PoseSource) {
        let status = self.status.camera(camera);
        let source = status.map(|c| c.pose_source).unwrap_or_default();
        match (&self.song, source) {
            (Some(song), PoseSource::Timeline) => {
                let shown = self.timeline.preview.as_ref().unwrap_or(song);
                (eval::pose_at_f(shown, camera, self.playhead()), PoseSource::Timeline)
            }
            _ => (status.map(|c| c.pose).unwrap_or_else(Pose::home), source),
        }
    }

    /// Every camera's pose now, in project order.
    pub fn poses(&self) -> Vec<Pose> {
        self.project.cameras.iter().map(|c| self.camera_pose(&c.id).0).collect()
    }

    /// The pose the program monitor draws: the selected camera's.
    pub fn monitor_pose(&self) -> (Pose, PoseSource) {
        self.camera_pose(self.cam_id())
    }

    pub fn timecode_at(&self, frame: i64) -> String {
        match &self.song {
            Some(song) => song.clock(self.project.rate).timecode_at(frame).to_string(),
            None => self.status.timecode.clone(),
        }
    }
}
