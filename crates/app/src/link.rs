//! The connection to the show server. Background threads own the HTTP client,
//! the WebSocket and the picture fetches, so the UI thread never waits on the
//! network. Commands leave through one channel; replies and status arrive
//! through another.

use ptz_core::api::*;
use ptz_core::client::{Api, ApiErr};
use ptz_core::pose::{Preset, Project};
use ptz_core::song::Song;
use ptz_core::transport::Action;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum Cmd {
    PutWorking {
        seq: u64,
        song: Song,
    },
    Fetch(String),
    Act(Action, TransportRequest),
    TakeLease {
        name: String,
        force: bool,
    },
    ReleaseLease,
    NewSong {
        name: String,
        start_tc: Option<String>,
    },
    LoadSong(String),
    Save(String),
    Revert(String),
    Commit(String),
    CopySong {
        id: String,
        name: String,
    },
    DeleteSong(String),
    PutProject(Box<Project>),
    StorePreset(StorePreset),
    /// Change one stored preset: its id, and what to change.
    UpdatePreset(String, UpdatePreset),
    DeletePreset(String),
    /// Read one camera's presets from its rig.
    SyncPresets(String),
    /// Hand control of one camera.
    Jog(String, Jog),
    Upload(PathBuf),
    SetMediaTc {
        id: String,
        start_tc: String,
    },
    Refit {
        clip: String,
        scale: f64,
    },
    FetchPeaks {
        media: String,
        /// Samples per pixel at the current zoom.
        spp: u32,
    },
    DeleteMedia(String),
    FetchPresetThumb(String),
    /// Read an OBJ or 3DS file and make it the stage model.
    ImportStage(PathBuf),
    /// Fetch the stage model the project names, by its revision.
    FetchStageModel(u64),
    RemoveStageModel,
    RefreshInputs,
    RefreshSongs,
    RefreshPresets,
    RefreshProject,
    RefreshMedia,
    RefreshAll,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Why {
    /// Loaded, created or fetched: replaces the open song and clears undo.
    Open,
    /// Saved or similar: only the flags changed.
    Flags,
    /// The server refused an edit or changed the song itself; local state is replaced.
    Reload,
}

#[derive(Debug)]
pub enum Event {
    Connected(Box<Hello>),
    ConnectFailed(String),
    Disconnected(String),
    Reconnected,
    Status(Box<Status>),
    Working(Box<WorkingSong>, Why),
    PutDone {
        seq: u64,
        working: Box<WorkingSong>,
    },
    PutRefused {
        seq: u64,
        msg: String,
    },
    Songs(Vec<SongSummary>),
    Presets(Vec<Preset>),
    Project(Box<Project>),
    Media(Vec<MediaInfo>),
    Peaks {
        media: String,
        level: PeakLevel,
    },
    /// A proxy frame, decoded. `None` when the server has no such picture.
    Frame {
        key: FrameKey,
        image: Option<egui::ColorImage>,
    },
    Inputs(InputChoices),
    /// The camera's picture of a preset, decoded.
    PresetThumb {
        id: String,
        image: egui::ColorImage,
    },
    /// The server took a preset sync. On hardware it has only begun: the list arrives when it ends.
    SyncAccepted(Vec<Preset>),
    Committed(RevisionInfo),
    Uploaded(MediaInfo),
    /// The stage model of a revision, read from the server or just uploaded to it.
    StageModel {
        revision: u64,
        mesh: Arc<ptz_scene::mesh::Mesh>,
    },
    StageFetchFailed {
        revision: u64,
        msg: String,
    },
    StageImport(crate::stage::Import),
    Log(LogRecord),
    /// The server said no. `what` is the thing that was attempted.
    Refused {
        what: String,
        msg: String,
    },
    Info(String),
    /// An E-Stop was pressed here and the server has not taken it. `trying` while it is still being sent.
    StopNotSent {
        why: String,
        trying: bool,
    },
}

#[derive(Debug)]
pub struct Hello {
    pub status: Status,
    pub project: Project,
    pub presets: Vec<Preset>,
    pub songs: Vec<SongSummary>,
    pub media: Vec<MediaInfo>,
    pub working: Option<WorkingSong>,
    pub log: Vec<LogRecord>,
}

/// One picture of a video: the media, the frame of the file in timeline
/// frames, and whether the small filmstrip size is wanted.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FrameKey {
    pub media: String,
    pub frame: i64,
    pub thumb: bool,
}

pub struct Link {
    pub tx: Sender<Cmd>,
    pub rx: Receiver<Event>,
    pub base: String,
    frames: Sender<FrameKey>,
    /// E-Stop goes out here, on a thread that does nothing else.
    stops: Sender<Cmd>,
    stop: Arc<AtomicBool>,
}

impl Link {
    pub fn send(&self, cmd: Cmd) {
        // E-Stop does not wait its turn behind whatever else was asked of the server.
        let lane = if matches!(cmd, Cmd::Act(Action::Estop, _)) { &self.stops } else { &self.tx };
        let _ = lane.send(cmd);
    }

    /// Ask for a picture. It arrives later as `Event::Frame`, unless newer requests crowd it out.
    pub fn want_frame(&self, key: FrameKey) {
        let _ = self.frames.send(key);
    }

    /// A link with no server behind it, for tests. Commands pile up in the returned receiver.
    #[cfg(test)]
    pub fn detached() -> (Link, Receiver<Cmd>, Sender<Event>) {
        let (tx, cmd_rx) = channel();
        let (ev_tx, rx) = channel();
        let (frames, _) = channel();
        let stops = tx.clone();
        (Link { tx, rx, base: "test".into(), frames, stops, stop: Arc::new(AtomicBool::new(false)) }, cmd_rx, ev_tx)
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn new_client_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("app-{:x}-{:x}", std::process::id(), nanos & 0xffff_ffff)
}

type Repaint = Arc<dyn Fn() + Send + Sync>;

pub fn connect(base: &str, key: Option<&str>, station: &str, repaint: Repaint) -> Link {
    let api = Api::new(base, &new_client_id(), key);
    let (tx, cmd_rx) = channel::<Cmd>();
    let (ev_tx, rx) = channel::<Event>();
    let stop = Arc::new(AtomicBool::new(false));
    let (frames, frame_rx) = channel::<FrameKey>();
    let (stops, stop_rx) = channel::<Cmd>();
    let link = Link { tx: tx.clone(), rx, base: api.base().to_string(), frames, stops, stop: stop.clone() };
    let station = station.to_string();
    {
        let (api, events, repaint) = (api.clone(), ev_tx.clone(), repaint.clone());
        std::thread::Builder::new()
            .name("link-stop".into())
            .spawn(move || stop_loop(api, stop_rx, events, repaint, StopPatience::default()))
            .expect("stop thread");
    }
    {
        let (api, events, repaint) = (api.clone(), ev_tx.clone(), repaint.clone());
        std::thread::Builder::new()
            .name("link-frames".into())
            .spawn(move || frame_loop(api, frame_rx, events, repaint))
            .expect("frame thread");
    }
    std::thread::Builder::new()
        .name("link-http".into())
        .spawn(move || worker(api, station, cmd_rx, tx, ev_tx, stop, repaint))
        .expect("link thread");
    link
}

/// How E-Stop is sent when the server does not take it at once.
#[derive(Clone, Copy)]
struct StopPatience {
    /// With no answer for this long the operator is told. The request is left to run: it may yet arrive.
    answer: Duration,
    /// A stop that has not been taken is sent again, this long after the last try,
    again: Duration,
    /// until this long after it was pressed. A stop is not left to arrive out of the blue minutes later.
    give_up: Duration,
}

impl Default for StopPatience {
    fn default() -> Self {
        StopPatience { answer: Duration::from_secs(2), again: Duration::from_millis(500), give_up: Duration::from_secs(10) }
    }
}

/// Send each E-Stop the moment it is pressed, on a connection of its own, and say so if it is not taken.
/// It is sent whatever the rest of the link believes about the server: the press is never dropped.
fn stop_loop(api: Api, presses: Receiver<Cmd>, events: Sender<Event>, repaint: Repaint, patience: StopPatience) {
    let emit = |event: Event| {
        let _ = events.send(event);
        repaint();
    };
    while let Ok(press) = presses.recv() {
        let Cmd::Act(action, req) = press else { continue };
        let mut pressed = Instant::now();
        loop {
            let (answer, answered) = channel();
            let (api, req) = (api.clone(), req.clone());
            let sent = std::thread::Builder::new().name("link-stop-send".into()).spawn(move || {
                let _ = answer.send(api.act(action, &req));
            });
            let outcome = match sent {
                Err(e) => Err(e.to_string()),
                Ok(_) => match answered.recv_timeout(patience.answer) {
                    Ok(Ok(status)) => Ok(status),
                    Ok(Err(e)) => Err(e.message().to_string()),
                    Err(_) => Err(format!("The server has not answered in {:.0} s", patience.answer.as_secs_f32())),
                },
            };
            // Pressed again meanwhile: the time for trying starts over.
            if presses.try_iter().count() > 0 {
                pressed = Instant::now();
            }
            match outcome {
                Ok(status) => {
                    emit(Event::Status(Box::new(status)));
                    break;
                }
                Err(why) => {
                    let trying = pressed.elapsed() + patience.again < patience.give_up;
                    emit(Event::StopNotSent { why, trying });
                    if !trying {
                        break;
                    }
                    std::thread::sleep(patience.again);
                }
            }
        }
    }
}

fn hello(api: &Api, station: &str) -> Result<Hello, ApiErr> {
    let first = api.status()?;
    // Take the lease when nobody else holds it. Otherwise connect as a watcher.
    if first.lease.is_none() || first.you_hold_lease {
        let _ = api.take_lease(station, false);
    }
    let status = api.status()?;
    let working = match &status.song {
        Some(s) => Some(api.working(&s.id)?),
        None => None,
    };
    Ok(Hello {
        project: api.project()?,
        presets: api.presets()?,
        songs: api.songs()?,
        media: api.get("/media").unwrap_or_default(),
        log: api.get("/log?since=0").unwrap_or_default(),
        working,
        status,
    })
}

/// Requests still waiting when this many newer ones have arrived are dropped:
/// the playhead has moved on, and the window asks again for what it still shows.
const FRAME_BACKLOG: usize = 48;
pub const THUMB_WIDTH: u32 = 120;

fn decode_frame(jpeg: &[u8], thumb: bool) -> Option<egui::ColorImage> {
    let picture = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg).ok()?;
    let picture = if thumb { picture.thumbnail(THUMB_WIDTH, THUMB_WIDTH) } else { picture };
    let rgba = picture.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// Fetch and decode proxy frames, newest request first.
fn frame_loop(api: Api, wanted: Receiver<FrameKey>, events: Sender<Event>, repaint: Repaint) {
    let mut queue: Vec<FrameKey> = Vec::new();
    loop {
        if queue.is_empty() {
            match wanted.recv() {
                Ok(key) => queue.push(key),
                Err(_) => return,
            }
        }
        for key in wanted.try_iter() {
            queue.retain(|k| *k != key);
            queue.push(key);
        }
        if queue.len() > FRAME_BACKLOG {
            queue.drain(..queue.len() - FRAME_BACKLOG);
        }
        let Some(key) = queue.pop() else { continue };
        let image = api
            .get_bytes(&format!("/media/{}/frames/{}.jpg", key.media, key.frame))
            .ok()
            .and_then(|jpeg| decode_frame(&jpeg, key.thumb));
        if events.send(Event::Frame { key, image }).is_err() {
            return;
        }
        repaint();
    }
}

/// Drop commands that a later one in the same batch makes pointless.
fn coalesce(batch: Vec<Cmd>) -> Vec<Cmd> {
    let mut out: Vec<Cmd> = Vec::with_capacity(batch.len());
    for cmd in batch {
        let replaces_last = match (out.last(), &cmd) {
            (Some(Cmd::Jog(a, _)), Cmd::Jog(b, _)) => a == b,
            (Some(Cmd::PutProject(_)), Cmd::PutProject(_)) => true,
            (Some(Cmd::Act(Action::Seek, _)), Cmd::Act(Action::Seek, _)) => true,
            (Some(Cmd::PutWorking { song: a, .. }), Cmd::PutWorking { song: b, .. }) => a.id == b.id,
            _ => false,
        };
        if replaces_last {
            out.pop();
        }
        out.push(cmd);
    }
    out
}

struct Worker {
    api: Api,
    events: Sender<Event>,
    repaint: Repaint,
    counters: HashMap<String, u64>,
}

impl Worker {
    fn emit(&self, event: Event) {
        let _ = self.events.send(event);
        (self.repaint)();
    }

    fn refused(&self, what: &str, e: ApiErr) {
        self.emit(Event::Refused { what: what.to_string(), msg: e.message().to_string() });
    }

    /// Run `work` on a thread of its own, with a client and a way to report.
    fn aside(&self, name: &str, work: impl FnOnce(&Api, &dyn Fn(Event)) + Send + 'static) {
        let (api, events, repaint) = (self.api.clone(), self.events.clone(), self.repaint.clone());
        let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
            let emit = |event: Event| {
                let _ = events.send(event);
                repaint();
            };
            work(&api, &emit);
        });
        if let Err(e) = spawned {
            self.emit(Event::Refused { what: "Start the transfer".into(), msg: e.to_string() });
        }
    }

    fn working(&mut self, what: &str, result: Result<WorkingSong, ApiErr>, why: Why) {
        match result {
            Ok(w) => {
                self.counters.insert(w.song.id.clone(), w.edit_counter);
                self.emit(Event::Working(Box::new(w), why));
            }
            Err(e) => self.refused(what, e),
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::PutWorking { seq, song } => {
                let id = song.id.clone();
                let expected = self.counters.get(&id).copied().unwrap_or(0);
                match self.api.put_working(&PutWorking { expected_counter: expected, song }) {
                    Ok(w) => {
                        self.counters.insert(id, w.edit_counter);
                        self.emit(Event::PutDone { seq, working: Box::new(w) });
                    }
                    Err(e) => {
                        self.emit(Event::PutRefused { seq, msg: e.message().to_string() });
                        if e.code().is_some() {
                            let fresh = self.api.working(&id);
                            self.working("Reload the song", fresh, Why::Reload);
                        }
                    }
                }
            }
            Cmd::Fetch(id) => {
                let r = self.api.working(&id);
                self.working("Open the song", r, Why::Reload);
            }
            Cmd::Act(action, req) => match self.api.act(action, &req) {
                Ok(status) => self.emit(Event::Status(Box::new(status))),
                Err(e) => {
                    let label = action.labels().last().copied().unwrap_or("Action");
                    self.refused(label, e);
                }
            },
            Cmd::TakeLease { name, force } => match self.api.take_lease(&name, force) {
                Ok(status) => self.emit(Event::Status(Box::new(status))),
                Err(e) => self.refused("Take the lease", e),
            },
            Cmd::ReleaseLease => match self.api.release_lease() {
                Ok(status) => self.emit(Event::Status(Box::new(status))),
                Err(e) => self.refused("Release the lease", e),
            },
            Cmd::NewSong { name, start_tc } => match self.api.new_song(&name, start_tc.as_deref()) {
                Ok(w) => {
                    let r = self.api.load_song(&w.song.id);
                    self.working("Open the new song", r, Why::Open);
                }
                Err(e) => self.refused("New song", e),
            },
            Cmd::LoadSong(id) => {
                let r = self.api.load_song(&id);
                self.working("Load", r, Why::Open);
            }
            Cmd::Save(id) => {
                let r = self.api.save(&id);
                if r.is_ok() {
                    self.emit(Event::Info("Saved".into()));
                }
                self.working("Save", r, Why::Flags);
            }
            Cmd::Revert(id) => {
                let r = self.api.revert(&id);
                self.working("Revert", r, Why::Open);
            }
            Cmd::Commit(id) => match self.api.commit(&id) {
                Ok(info) => {
                    self.emit(Event::Committed(info));
                    let r = self.api.working(&id);
                    self.working("Commit", r, Why::Flags);
                }
                Err(e) => self.refused("Commit", e),
            },
            Cmd::CopySong { id, name } => match self.api.copy_song(&id, &name) {
                Ok(w) => {
                    let r = self.api.load_song(&w.song.id);
                    self.working("Open the copy", r, Why::Open);
                }
                Err(e) => self.refused("Save As", e),
            },
            Cmd::DeleteSong(id) => match self.api.delete_song(&id) {
                Ok(songs) => self.emit(Event::Songs(songs)),
                Err(e) => self.refused("Delete song", e),
            },
            Cmd::PutProject(project) => match self.api.put_project(&project) {
                Ok(p) => self.emit(Event::Project(Box::new(p))),
                Err(e) => {
                    self.refused("Project settings", e);
                    if let Ok(p) = self.api.project() {
                        self.emit(Event::Project(Box::new(p)));
                    }
                }
            },
            Cmd::StorePreset(req) => match self.api.post::<_, Preset>("/presets", &req) {
                Ok(p) => self.emit(Event::Info(format!("Stored preset {}", p.name))),
                Err(e) => self.refused("Store preset", e),
            },
            Cmd::UpdatePreset(id, req) => match self.api.put::<_, Preset>(&format!("/presets/{id}"), &req) {
                Ok(p) => {
                    self.emit(Event::Info(match req.elevation {
                        Some(_) if req.pose.is_none() => format!("Set the height of {}", p.name),
                        _ => format!("Updated {} to the current pose", p.name),
                    }));
                    if let Ok(all) = self.api.presets() {
                        self.emit(Event::Presets(all));
                    }
                }
                Err(e) => self.refused("Update preset", e),
            },
            Cmd::DeletePreset(id) => match self.api.delete::<Vec<Preset>>(&format!("/presets/{id}")) {
                Ok(p) => self.emit(Event::Presets(p)),
                Err(e) => self.refused("Delete preset", e),
            },
            Cmd::SyncPresets(camera) => match self.api.sync_presets(&camera) {
                Ok(p) => self.emit(Event::SyncAccepted(p)),
                Err(e) => self.refused("Sync presets", e),
            },
            Cmd::Jog(camera, jog) => {
                if let Err(e) = self.api.jog(&camera, &jog) {
                    if !jog.is_zero() {
                        self.refused("Rig controller", e);
                    }
                }
            }
            // Long transfers run on threads of their own, so E-Stop and everything else behind them is not kept waiting.
            Cmd::Upload(path) => self.aside("link-upload", move |api, emit| upload_media(api, &path, emit)),
            Cmd::ImportStage(path) => self.aside("link-stage-import", move |api, emit| import_stage(api, &path, emit)),
            Cmd::FetchStageModel(revision) => {
                self.aside("link-stage-fetch", move |api, emit| fetch_stage_model(api, revision, emit))
            }
            Cmd::RemoveStageModel => match self.api.delete_stage_model() {
                Ok(p) => self.emit(Event::Project(Box::new(p))),
                Err(e) => self.refused("Remove the stage model", e),
            },
            Cmd::SetMediaTc { id, start_tc } => {
                match self.api.post::<_, MediaInfo>(&format!("/media/{id}/start_tc"), &StoreTc { start_tc }) {
                    Ok(_) => self.handle(Cmd::RefreshMedia),
                    Err(e) => self.refused("Start timecode", e),
                }
            }
            Cmd::Refit { clip, scale } => {
                match self.api.post::<_, WorkingSong>(&format!("/record/{clip}/refit"), &Refit { scale }) {
                    Ok(w) => {
                        self.counters.insert(w.song.id.clone(), w.edit_counter);
                        self.emit(Event::Working(Box::new(w), Why::Reload));
                    }
                    Err(e) => self.refused("Refit", e),
                }
            }
            Cmd::FetchPeaks { media, spp } => {
                if let Ok(level) = self.api.peaks(&media, spp) {
                    self.emit(Event::Peaks { media, level });
                }
            }
            Cmd::DeleteMedia(id) => match self.api.delete_media(&id) {
                Ok(m) => self.emit(Event::Media(m)),
                Err(e) => self.refused("Delete media", e),
            },
            Cmd::RefreshInputs => {
                if let Ok(choices) = self.api.get::<InputChoices>("/inputs") {
                    self.emit(Event::Inputs(choices));
                }
            }
            Cmd::FetchPresetThumb(id) => {
                let jpeg = self.api.get_bytes(&format!("/presets/{id}/thumb.jpg")).ok();
                if let Some(image) = jpeg.and_then(|jpeg| decode_frame(&jpeg, false)) {
                    self.emit(Event::PresetThumb { id, image });
                }
            }
            Cmd::RefreshSongs => {
                if let Ok(s) = self.api.songs() {
                    self.emit(Event::Songs(s));
                }
            }
            Cmd::RefreshPresets => {
                if let Ok(p) = self.api.presets() {
                    self.emit(Event::Presets(p));
                }
            }
            Cmd::RefreshProject => {
                if let Ok(p) = self.api.project() {
                    self.emit(Event::Project(Box::new(p)));
                }
            }
            Cmd::RefreshMedia => {
                if let Ok(m) = self.api.media() {
                    self.emit(Event::Media(m));
                }
            }
            Cmd::RefreshAll => {
                for c in [Cmd::RefreshSongs, Cmd::RefreshPresets, Cmd::RefreshProject, Cmd::RefreshMedia] {
                    self.handle(c);
                }
            }
        }
    }
}

fn upload_media(api: &Api, path: &std::path::Path, emit: &dyn Fn(Event)) {
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "media".into());
    emit(Event::Info(format!("Uploading {name}")));
    match std::fs::File::open(path) {
        Ok(file) => match api.upload_media(&name, file) {
            Ok(info) => emit(Event::Uploaded(info)),
            Err(e) => emit(Event::Refused { what: format!("Import {name}"), msg: e.message().to_string() }),
        },
        Err(e) => emit(Event::Refused { what: format!("Import {name}"), msg: e.to_string() }),
    }
}

/// Read a model file here, where its material file is, and hand the server the mesh.
fn import_stage(api: &Api, path: &std::path::Path, emit: &dyn Fn(Event)) {
    use crate::stage::Import;
    let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "model".into());
    emit(Event::StageImport(Import::Reading(name.clone())));
    let read = match crate::stage::read_model(path) {
        Ok(read) => read,
        Err(why) => return emit(Event::StageImport(Import::Failed { name, why })),
    };
    emit(Event::StageImport(Import::Uploading(name.clone())));
    match api.upload_stage_model(&read.name, read.unit, read.up, &read.mesh.to_bytes()) {
        Ok(project) => {
            if let Some(model) = &project.stage.model {
                emit(Event::StageModel { revision: model.revision, mesh: Arc::new(read.mesh) });
            }
            emit(Event::Project(Box::new(project)));
            emit(Event::StageImport(Import::Done(name)));
        }
        Err(e) => emit(Event::StageImport(Import::Failed { name, why: e.message().to_string() })),
    }
}

fn fetch_stage_model(api: &Api, revision: u64, emit: &dyn Fn(Event)) {
    let mesh = api.stage_model().map_err(|e| e.message().to_string()).and_then(|bytes| {
        ptz_scene::mesh::Mesh::from_bytes(&bytes).map_err(|why| format!("the file the server sent is damaged: {why}"))
    });
    match mesh {
        Ok(mesh) => emit(Event::StageModel { revision, mesh: Arc::new(mesh) }),
        Err(msg) => emit(Event::StageFetchFailed { revision, msg }),
    }
}

fn worker(
    api: Api,
    station: String,
    cmds: Receiver<Cmd>,
    cmd_tx: Sender<Cmd>,
    events: Sender<Event>,
    stop: Arc<AtomicBool>,
    repaint: Repaint,
) {
    let mut w = Worker { api: api.clone(), events: events.clone(), repaint: repaint.clone(), counters: HashMap::new() };
    match hello(&api, &station) {
        Ok(h) => {
            if let Some(working) = &h.working {
                w.counters.insert(working.song.id.clone(), working.edit_counter);
            }
            w.emit(Event::Connected(Box::new(h)));
        }
        Err(e) => {
            w.emit(Event::ConnectFailed(e.message().to_string()));
            return;
        }
    }

    {
        let api = api.clone();
        let stop = stop.clone();
        let repaint = repaint.clone();
        let _ = std::thread::Builder::new().name("link-ws".into()).spawn(move || ws_loop(api, events, cmd_tx, stop, repaint));
    }

    while let Ok(first) = cmds.recv() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut batch = vec![first];
        batch.extend(cmds.try_iter());
        for cmd in coalesce(batch) {
            w.handle(cmd);
        }
    }
}

fn ws_connect(api: &Api) -> Result<tungstenite::WebSocket<std::net::TcpStream>, String> {
    let url = api.ws_url();
    let host = url.trim_start_matches("ws://").split('/').next().unwrap_or("").to_string();
    let addr = std::net::ToSocketAddrs::to_socket_addrs(&host.as_str())
        .map_err(|e| e.to_string())?
        .next()
        .ok_or_else(|| format!("{host} does not resolve"))?;
    let stream = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
    let (socket, _) = tungstenite::client::client(url.as_str(), stream).map_err(|e| e.to_string())?;
    socket.get_ref().set_read_timeout(Some(Duration::from_millis(250))).ok();
    Ok(socket)
}

fn ws_loop(api: Api, events: Sender<Event>, cmds: Sender<Cmd>, stop: Arc<AtomicBool>, repaint: Repaint) {
    let emit = |e: Event| {
        let ok = events.send(e).is_ok();
        repaint();
        ok
    };
    let mut was_down = false;
    while !stop.load(Ordering::Relaxed) {
        let mut socket = match ws_connect(&api) {
            Ok(s) => s,
            Err(e) => {
                if !was_down {
                    was_down = true;
                    if !emit(Event::Disconnected(format!("Lost the server: {e}"))) {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(800));
                continue;
            }
        };
        if was_down {
            if !emit(Event::Reconnected) {
                return;
            }
            let _ = cmds.send(Cmd::RefreshAll);
        }
        let mut last_beat = Instant::now();
        let mut last_heard = Instant::now();
        loop {
            if stop.load(Ordering::Relaxed) {
                let _ = socket.close(None);
                return;
            }
            // The heartbeat keeps the edit lease alive.
            if last_beat.elapsed() >= Duration::from_secs(2) {
                last_beat = Instant::now();
                if socket.send(tungstenite::Message::text("hb")).is_err() {
                    break;
                }
            }
            match socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    last_heard = Instant::now();
                    let Ok(msg) = serde_json::from_str::<WsMsg>(text.as_str()) else { continue };
                    let ok = match msg {
                        WsMsg::Status(s) => emit(Event::Status(s)),
                        WsMsg::Log { record } => emit(Event::Log(record)),
                        WsMsg::SongListChanged => cmds.send(Cmd::RefreshSongs).is_ok(),
                        WsMsg::PresetsChanged => cmds.send(Cmd::RefreshPresets).is_ok(),
                        WsMsg::MediaChanged => cmds.send(Cmd::RefreshMedia).is_ok(),
                        WsMsg::ProjectChanged => cmds.send(Cmd::RefreshProject).is_ok(),
                        WsMsg::SongChanged { .. } => true,
                    };
                    if !ok {
                        return;
                    }
                }
                Ok(tungstenite::Message::Close(_)) => break,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                {
                    // Status arrives at least five times a second; two silent seconds means the link is gone.
                    if last_heard.elapsed() > Duration::from_secs(2) {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        was_down = true;
        if !emit(Event::Disconnected("Lost the connection to the server. Reconnecting".into())) {
            return;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Where the show server is on this computer, if it is installed: beside this
/// executable (a source build, or the app bundle that carries it), in the folders
/// the installers use, or on PATH. An app opened from the Finder has a short PATH
/// that leaves those folders out, so they are looked in by name.
pub fn local_server() -> Option<PathBuf> {
    let name = if cfg!(windows) { "ptz-server.exe" } else { "ptz-server" };
    let beside = std::env::current_exe().ok().map(|exe| exe.with_file_name(name));
    let mut dirs = vec![PathBuf::from("/usr/local/ptz-programmer/bin"), PathBuf::from("/usr/local/bin")];
    dirs.extend(dirs::home_dir().map(|home| home.join(".local").join("bin")));
    dirs.extend(std::env::var_os("PATH").map(|path| std::env::split_paths(&path).collect::<Vec<_>>()).unwrap_or_default());
    find_server(beside, &dirs, name)
}

fn find_server(beside: Option<PathBuf>, dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    beside.into_iter().chain(dirs.iter().map(|dir| dir.join(name))).find(|path| path.is_file())
}

pub const NO_LOCAL_SERVER: &str =
    "The show server is not installed on this computer. Install it, or enter the address of a server on another machine";

/// Launch `ptz-server` detached.
pub fn start_local_server() -> Result<String, String> {
    let program = local_server().ok_or_else(|| NO_LOCAL_SERVER.to_string())?;
    let mut cmd = std::process::Command::new(&program);
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group, so it keeps running when the app quits.
        cmd.process_group(0);
    }
    cmd.spawn().map_err(|e| format!("Could not start {}: {e}", program.display()))?;
    Ok(format!("http://127.0.0.1:{DEFAULT_PORT}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_server_is_found_beside_the_app_first_then_where_the_installers_put_it() {
        let root = std::env::temp_dir().join(format!("ptz-find-server-{}", std::process::id()));
        let (bundle, system, user) = (root.join("bundle"), root.join("usr-local-bin"), root.join("home-local-bin"));
        for dir in [&bundle, &system, &user] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let dirs = [system.clone(), user.clone()];
        let beside = Some(bundle.join("ptz-server"));
        // An app installed without the server finds none, and says so instead of failing to launch a name.
        assert_eq!(find_server(beside.clone(), &dirs, "ptz-server"), None);
        // A folder with that name is not the server.
        std::fs::create_dir_all(system.join("ptz-server")).unwrap();
        assert_eq!(find_server(beside.clone(), &dirs, "ptz-server"), None);

        std::fs::write(user.join("ptz-server"), b"").unwrap();
        assert_eq!(find_server(beside.clone(), &dirs, "ptz-server"), Some(user.join("ptz-server")));
        std::fs::remove_dir(system.join("ptz-server")).unwrap();
        std::fs::write(system.join("ptz-server"), b"").unwrap();
        assert_eq!(find_server(beside.clone(), &dirs, "ptz-server"), Some(system.join("ptz-server")));
        std::fs::write(bundle.join("ptz-server"), b"").unwrap();
        assert_eq!(find_server(beside, &dirs, "ptz-server"), Some(bundle.join("ptz-server")));
        assert_eq!(find_server(None, &dirs, "ptz-server"), Some(system.join("ptz-server")));
        let _ = std::fs::remove_dir_all(&root);
    }
    use crate::stage::Import;
    use ptz_scene::mesh::Mesh;

    /// A real server on a free port, and a folder of files to import, both removed afterwards.
    struct Site {
        running: Option<ptz_server::http::Running>,
        dir: PathBuf,
        url: String,
    }

    impl Site {
        fn open(name: &str) -> Site {
            let dir = std::env::temp_dir().join(format!("ptz-app-test-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(dir.join("venue")).unwrap();
            let mut config = ptz_server::engine::Config::new(dir.join("show"));
            config.audio = ptz_server::engine::AudioChoice::Off;
            let running = ptz_server::http::start(&config, "127.0.0.1:0", None).expect("the server starts");
            Site { url: running.url(), running: Some(running), dir }
        }

        fn station(&self, name: &str) -> (Link, Hello) {
            let link = connect(&self.url, None, name, Arc::new(|| {}));
            let hello = wait(&link, |event| match event {
                Event::Connected(hello) => Some(*hello),
                Event::ConnectFailed(why) => panic!("{name} could not connect: {why}"),
                _ => None,
            });
            (link, hello)
        }

        fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
            let path = self.dir.join("venue").join(name);
            std::fs::write(&path, contents).unwrap();
            path
        }
    }

    impl Drop for Site {
        fn drop(&mut self) {
            self.running.take();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// The first event `pick` accepts. Status and log traffic in between is passed over.
    fn wait<T>(link: &Link, mut pick: impl FnMut(Event) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = link.rx.recv_timeout(left).expect("the link went quiet before the awaited event");
            if let Some(found) = pick(event) {
                return found;
            }
        }
    }

    /// A hall of 40 by 9 by 25 written in millimetres with Y up, and a platform in another colour.
    fn hall() -> Mesh {
        let mut mesh = Mesh::default();
        mesh.push_cuboid([0.0, 0.0, -25_000.0], [40_000.0, 9_000.0, 0.0], [120, 120, 130]);
        mesh.push_cuboid([15_000.0, 0.0, -24_000.0], [25_000.0, 1_000.0, -18_000.0], [90, 60, 40]);
        mesh
    }

    #[test]
    fn a_model_file_is_read_here_sent_to_the_server_and_served_to_another_station() {
        let site = Site::open("stage-import");
        let (obj, mtl) = ptz_scene::obj::write(&hall(), "hall.mtl");
        let path = site.file("hall.obj", obj.as_bytes());
        site.file("hall.mtl", mtl.as_bytes());
        let (luke, hello) = site.station("Luke");
        assert!(hello.status.you_hold_lease && hello.project.stage.model.is_none());

        // Reading, sending, then the mesh and the project that names it, in that order.
        luke.send(Cmd::ImportStage(path.clone()));
        let mut steps = Vec::new();
        let mut held = None;
        let mut named = None;
        wait(&luke, |event| {
            match event {
                Event::StageImport(Import::Done(name)) => {
                    steps.push(format!("done {name}"));
                    return Some(());
                }
                Event::StageImport(Import::Failed { name, why }) => panic!("{name} was not imported: {why}"),
                Event::StageImport(Import::Reading(name)) => steps.push(format!("reading {name}")),
                Event::StageImport(Import::Uploading(name)) => steps.push(format!("sending {name}")),
                Event::StageModel { revision, mesh } => {
                    steps.push(format!("mesh {revision}"));
                    held = Some(mesh);
                }
                Event::Project(project) if project.stage.model.is_some() && named.is_none() => {
                    steps.push("project".into());
                    named = Some(*project);
                }
                _ => {}
            }
            None
        });
        assert_eq!(steps, ["reading hall.obj", "sending hall.obj", "mesh 1", "project", "done hall.obj"]);
        let (held, project) = (held.unwrap(), named.unwrap());
        assert_eq!(*held, hall(), "the triangles and the colours of the material file");
        let model = project.stage.model.clone().unwrap();
        assert_eq!((model.name.as_str(), model.revision, model.triangles), ("hall.obj", 1, 24));
        assert_eq!((model.unit, model.up), (ptz_core::camera::Unit::Mm, ptz_core::camera::UpAxis::Y));
        assert_eq!(model.size_m(), [40.0, 25.0, 9.0], "read as millimetres with Y up, it is a hall");
        assert!(!project.stage.show_builtin);

        // Another station learns of it when it connects, and fetches the same mesh.
        let (watcher, hello) = site.station("Watcher");
        assert!(!hello.status.you_hold_lease);
        assert_eq!(hello.project.stage, project.stage);
        watcher.send(Cmd::FetchStageModel(1));
        let fetched = wait(&watcher, |event| match event {
            Event::StageModel { revision: 1, mesh } => Some(mesh),
            Event::StageFetchFailed { msg, .. } => panic!("{msg}"),
            _ => None,
        });
        assert_eq!(*fetched, *held);

        // A station without the lease cannot replace it, and is told why.
        watcher.send(Cmd::ImportStage(path));
        let why = wait(&watcher, |event| match event {
            Event::StageImport(Import::Failed { why, .. }) => Some(why),
            Event::StageImport(Import::Done(_)) => panic!("a watcher replaced the stage model"),
            _ => None,
        });
        assert_eq!(why, "Take the edit lease to control the show");

        // A file that cannot be read names the line, and the model on the server stays.
        let broken = site.file("broken.obj", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 9\n");
        luke.send(Cmd::ImportStage(broken));
        let why = wait(&luke, |event| match event {
            Event::StageImport(Import::Failed { name, why }) => Some(format!("{name}: {why}")),
            _ => None,
        });
        assert!(why.starts_with("broken.obj: line 4:"), "{why}");
        let api = Api::new(&site.url, "check", None);
        assert_eq!(api.project().unwrap().stage, project.stage);

        // Removing it brings the built-in stage back, and there is then nothing to fetch.
        luke.send(Cmd::RemoveStageModel);
        let after = wait(&luke, |event| match event {
            Event::Project(project) if project.stage.model.is_none() => Some(*project),
            Event::Refused { what, msg } => panic!("{what}: {msg}"),
            _ => None,
        });
        assert!(after.stage.show_builtin);
        watcher.send(Cmd::FetchStageModel(1));
        let msg = wait(&watcher, |event| match event {
            Event::StageFetchFailed { revision: 1, msg } => Some(msg),
            Event::StageModel { .. } => panic!("a removed model was served"),
            _ => None,
        });
        assert_eq!(msg, "No stage model has been imported");
    }

    #[test]
    fn a_long_upload_does_not_hold_up_the_commands_behind_it() {
        let site = Site::open("upload-aside");
        let (luke, _) = site.station("Luke");
        // Sixty megabytes of something that is not media: the server reads all of it before refusing it.
        let big = site.file("not-media.wav", &vec![7u8; 60 * 1024 * 1024]);
        luke.send(Cmd::Upload(big));
        luke.send(Cmd::Act(Action::Estop, TransportRequest::default()));
        let mut order = Vec::new();
        wait(&luke, |event| {
            match event {
                Event::Status(status) if status.transport.estop && !order.contains(&"estop") => order.push("estop"),
                Event::Uploaded(_) | Event::Refused { .. } => order.push("upload answered"),
                _ => {}
            }
            (order.len() == 2).then_some(())
        });
        assert_eq!(order, ["estop", "upload answered"], "E-Stop was answered while the file was still on its way");
    }

    /// Passes everything through to `upstream`, but holds any request for `slow` back by `delay`.
    fn slow_proxy(upstream: &str, slow: &'static str, delay: Duration) -> String {
        use std::io::{Read, Write};
        use std::net::{Shutdown, TcpListener, TcpStream};
        let upstream = upstream.trim_start_matches("http://").trim_end_matches('/').to_string();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let Ok(server) = TcpStream::connect(&upstream) else { continue };
                let (mut from_client, mut to_client) = (client.try_clone().unwrap(), client);
                let (mut from_server, mut to_server) = (server.try_clone().unwrap(), server);
                std::thread::spawn(move || {
                    let mut buf = [0u8; 16384];
                    while let Ok(n) = from_client.read(&mut buf) {
                        if n == 0 {
                            break;
                        }
                        let head = &buf[..n.min(200)];
                        if head.starts_with(b"GET ") && head.windows(slow.len()).any(|w| w == slow.as_bytes()) {
                            std::thread::sleep(delay);
                        }
                        if to_server.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                    let _ = to_server.shutdown(Shutdown::Both);
                });
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut from_server, &mut to_client);
                    let _ = to_client.shutdown(Shutdown::Both);
                });
            }
        });
        url
    }

    #[test]
    fn e_stop_does_not_wait_for_the_answer_to_an_earlier_request() {
        let site = Site::open("stop-first");
        // The list of timecode inputs takes two seconds to come back: a slow device scan, a busy network.
        let url = slow_proxy(&site.url, "/inputs", Duration::from_secs(2));
        let link = connect(&url, None, "Luke", Arc::new(|| {}));
        wait(&link, |event| match event {
            Event::Connected(_) => Some(()),
            Event::ConnectFailed(why) => panic!("{why}"),
            _ => None,
        });
        link.send(Cmd::RefreshInputs);
        std::thread::sleep(Duration::from_millis(100));
        let pressed = Instant::now();
        link.send(Cmd::Act(Action::Estop, TransportRequest::default()));
        wait(&link, |event| match event {
            Event::Status(status) if status.transport.estop => Some(()),
            _ => None,
        });
        let took = pressed.elapsed();
        assert!(took < Duration::from_millis(500), "E-Stop reached the server {took:?} after it was pressed");
    }

    #[test]
    fn an_e_stop_the_server_does_not_take_is_reported_at_once_and_sent_again_for_a_while() {
        // A port with nothing listening on it: the server is down.
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let api = Api::new(&format!("127.0.0.1:{port}"), "test", None);
        let (presses, press_rx) = channel();
        let (events, event_rx) = channel();
        let patience = StopPatience {
            answer: Duration::from_secs(2),
            again: Duration::from_millis(100),
            give_up: Duration::from_millis(650),
        };
        std::thread::spawn(move || stop_loop(api, press_rx, events, Arc::new(|| {}), patience));
        let pressed = Instant::now();
        presses.send(Cmd::Act(Action::Estop, TransportRequest::default())).unwrap();
        let mut said = Vec::new();
        loop {
            match event_rx.recv_timeout(Duration::from_secs(5)).expect("every try is reported") {
                Event::StopNotSent { why, trying } => {
                    assert!(!why.is_empty());
                    if said.is_empty() {
                        assert!(pressed.elapsed() < Duration::from_millis(500), "the first report took {:?}", pressed.elapsed());
                    }
                    said.push(trying);
                    if !trying {
                        break;
                    }
                }
                other => panic!("{other:?}"),
            }
        }
        assert!(said.len() >= 3, "it was sent again: {said:?}");
        assert!(said[..said.len() - 1].iter().all(|trying| *trying), "{said:?}");
        assert!(pressed.elapsed() < Duration::from_secs(2), "it gave up after {:?}", pressed.elapsed());
        assert!(event_rx.recv_timeout(Duration::from_millis(400)).is_err(), "nothing is sent after giving up");
    }

    #[test]
    fn coalesce_keeps_only_the_latest_scrub_jog_and_song() {
        let song = |name: &str| {
            let mut s = Song::new("a", name, "01:00:00:00", 100);
            s.name = name.into();
            s
        };
        let seek = |f| Cmd::Act(Action::Seek, TransportRequest { frame: Some(f), ..Default::default() });
        let out = coalesce(vec![
            seek(1),
            seek(2),
            seek(3),
            Cmd::Act(Action::Play, TransportRequest::default()),
            seek(9),
            Cmd::PutWorking { seq: 1, song: song("one") },
            Cmd::PutWorking { seq: 2, song: song("two") },
            Cmd::Jog("cam1".into(), Jog { pan: 0.1, ..Jog::default() }),
            Cmd::Jog("cam1".into(), Jog { pan: 0.9, ..Jog::default() }),
        ]);
        assert_eq!(out.len(), 5);
        assert!(matches!(&out[0], Cmd::Act(Action::Seek, r) if r.frame == Some(3)));
        assert!(matches!(&out[1], Cmd::Act(Action::Play, _)));
        assert!(matches!(&out[2], Cmd::Act(Action::Seek, r) if r.frame == Some(9)));
        assert!(matches!(&out[3], Cmd::PutWorking { seq: 2, song } if song.name == "two"));
        assert!(matches!(&out[4], Cmd::Jog(_, j) if j.pan == 0.9));
    }
}
