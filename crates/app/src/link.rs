//! The connection to the show server. Two background threads own the HTTP
//! client and the WebSocket, so the UI thread never waits on the network.
//! Commands leave through one channel; replies and status arrive through another.

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
    PutWorking { seq: u64, song: Song },
    Fetch(String),
    Act(Action, TransportRequest),
    TakeLease { name: String, force: bool },
    ReleaseLease,
    NewSong { name: String, start_tc: Option<String> },
    LoadSong(String),
    Save(String),
    Revert(String),
    Commit(String),
    CopySong { id: String, name: String },
    DeleteSong(String),
    PutProject(Box<Project>),
    StorePreset(StorePreset),
    DeletePreset(String),
    SyncPresets,
    Jog(Jog),
    Upload(PathBuf),
    SetMediaTc { id: String, start_tc: String },
    Refit { clip: String, scale: f64 },
    FetchPeaks(String),
    FetchFrame { media: String, frame: i64 },
    FetchPresetThumb(String),
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
        peaks: Vec<f32>,
    },
    Frame {
        media: String,
        frame: i64,
        jpeg: Vec<u8>,
    },
    PresetThumb {
        id: String,
        jpeg: Vec<u8>,
    },
    Committed(RevisionInfo),
    Uploaded(MediaInfo),
    Log(LogRecord),
    /// The server said no. `what` is the thing that was attempted.
    Refused {
        what: String,
        msg: String,
    },
    Info(String),
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

pub struct Link {
    pub tx: Sender<Cmd>,
    pub rx: Receiver<Event>,
    pub base: String,
    stop: Arc<AtomicBool>,
}

impl Link {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// A link with no server behind it, for tests. Commands pile up in the returned receiver.
    #[cfg(test)]
    pub fn detached() -> (Link, Receiver<Cmd>, Sender<Event>) {
        let (tx, cmd_rx) = channel();
        let (ev_tx, rx) = channel();
        (Link { tx, rx, base: "test".into(), stop: Arc::new(AtomicBool::new(false)) }, cmd_rx, ev_tx)
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
    let link = Link { tx: tx.clone(), rx, base: api.base().to_string(), stop: stop.clone() };
    let station = station.to_string();
    std::thread::Builder::new()
        .name("link-http".into())
        .spawn(move || worker(api, station, cmd_rx, tx, ev_tx, stop, repaint))
        .expect("link thread");
    link
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

/// Drop commands that a later one in the same batch makes pointless.
fn coalesce(batch: Vec<Cmd>) -> Vec<Cmd> {
    let mut out: Vec<Cmd> = Vec::with_capacity(batch.len());
    for cmd in batch {
        let replaces_last = match (out.last(), &cmd) {
            (Some(Cmd::Jog(_)), Cmd::Jog(_)) => true,
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
            Cmd::DeletePreset(id) => match self.api.delete::<Vec<Preset>>(&format!("/presets/{id}")) {
                Ok(p) => self.emit(Event::Presets(p)),
                Err(e) => self.refused("Delete preset", e),
            },
            Cmd::SyncPresets => match self.api.post_empty::<Vec<Preset>>("/presets/sync") {
                Ok(p) => {
                    self.emit(Event::Info(format!("Synced {} presets from the rig", p.len())));
                    self.emit(Event::Presets(p));
                }
                Err(e) => self.refused("Sync presets", e),
            },
            Cmd::Jog(jog) => {
                if let Err(e) = self.api.jog(&jog) {
                    if !jog.is_zero() {
                        self.refused("Rig controller", e);
                    }
                }
            }
            Cmd::Upload(path) => {
                let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "media".into());
                self.emit(Event::Info(format!("Uploading {name}")));
                let encoded: String = name
                    .bytes()
                    .map(|b| match b {
                        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
                        other => format!("%{other:02X}"),
                    })
                    .collect();
                match std::fs::File::open(&path) {
                    Ok(file) => match self.api.upload::<MediaInfo>(&format!("/media?name={encoded}"), file) {
                        Ok(info) => self.emit(Event::Uploaded(info)),
                        Err(e) => self.refused(&format!("Import {name}"), e),
                    },
                    Err(e) => self.emit(Event::Refused { what: format!("Import {name}"), msg: e.to_string() }),
                }
            }
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
            Cmd::FetchPeaks(media) => {
                if let Ok(peaks) = self.api.get::<Vec<f32>>(&format!("/media/{media}/peaks")) {
                    self.emit(Event::Peaks { media, peaks });
                }
            }
            Cmd::FetchFrame { media, frame } => {
                if let Ok(jpeg) = self.api.get_bytes(&format!("/media/{media}/frames/{frame}.jpg")) {
                    self.emit(Event::Frame { media, frame, jpeg });
                }
            }
            Cmd::FetchPresetThumb(id) => {
                if let Ok(jpeg) = self.api.get_bytes(&format!("/presets/{id}/thumb.jpg")) {
                    self.emit(Event::PresetThumb { id, jpeg });
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
                if let Ok(m) = self.api.get::<Vec<MediaInfo>>("/media") {
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

/// Launch `ptz-server` detached, from beside this executable or from PATH.
pub fn start_local_server() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let name = if cfg!(windows) { "ptz-server.exe" } else { "ptz-server" };
    let beside = exe.with_file_name(name);
    let program = if beside.exists() { beside } else { PathBuf::from(name) };
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
            Cmd::Jog(Jog { pan: 0.1, ..Jog::default() }),
            Cmd::Jog(Jog { pan: 0.9, ..Jog::default() }),
        ]);
        assert_eq!(out.len(), 5);
        assert!(matches!(&out[0], Cmd::Act(Action::Seek, r) if r.frame == Some(3)));
        assert!(matches!(&out[1], Cmd::Act(Action::Play, _)));
        assert!(matches!(&out[2], Cmd::Act(Action::Seek, r) if r.frame == Some(9)));
        assert!(matches!(&out[3], Cmd::PutWorking { seq: 2, song } if song.name == "two"));
        assert!(matches!(&out[4], Cmd::Jog(j) if j.pan == 0.9));
    }
}
