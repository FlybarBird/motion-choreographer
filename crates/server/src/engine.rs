//! The show: one song loaded, one transport, one rig. Everything that changes
//! show state goes through `Show`, behind a single mutex, so there is no lock
//! order to get wrong.

use crate::audio::{AudioEngine, LtcOut, Mix, PlayClip};
use crate::media::{self, Ingested};
use crate::record::{self, Capture, RecordedPass};
use crate::rig::{RigAdapter, Simulator};
use crate::runlog::RunLog;
use crate::storage::{self, Storage};
use ptz_core::api::*;
use ptz_core::compile::{compile, Compiled};
use ptz_core::edit;
use ptz_core::pose::{Axis, Pose, Preset, Project};
use ptz_core::song::Song;
use ptz_core::time::{frame_to_sample, FrameRate, Timecode};
use ptz_core::transport::{self, Action, Context, Event, Output, Run, TimeMode, TransportState};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;

pub const NO_LEASE: &str = "Take the edit lease to control the show";

/// Which audio output to open.
#[derive(Clone, Debug, PartialEq)]
pub enum AudioChoice {
    /// The system's default output.
    Default,
    /// The first output whose name contains this text.
    Named(String),
    /// No device: run silent on the monotonic clock.
    Off,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub data: PathBuf,
    pub seed: bool,
    pub lease_ttl: Duration,
    pub audio: AudioChoice,
}

impl Config {
    pub fn new(data: impl Into<PathBuf>) -> Config {
        Config { data: data.into(), seed: true, lease_ttl: Duration::from_secs(10), audio: AudioChoice::Default }
    }
}

/// A refused request, with the HTTP status it maps to.
#[derive(Clone, PartialEq, Debug)]
pub struct Fail {
    pub code: u16,
    pub msg: String,
}

impl Fail {
    /// Policy refusal: the request was understood and is not allowed now.
    pub fn refused(msg: impl Into<String>) -> Fail {
        Fail { code: 409, msg: msg.into() }
    }
    pub fn bad(msg: impl Into<String>) -> Fail {
        Fail { code: 400, msg: msg.into() }
    }
    pub fn invalid(msg: impl Into<String>) -> Fail {
        Fail { code: 422, msg: msg.into() }
    }
    pub fn not_found(msg: impl Into<String>) -> Fail {
        Fail { code: 404, msg: msg.into() }
    }
    pub fn io(e: std::io::Error) -> Fail {
        Fail { code: 500, msg: format!("Could not write to the data folder: {e}") }
    }
}

pub type Res<T> = Result<T, Fail>;

/// What the WebSocket tasks hear about.
#[derive(Clone, Debug)]
pub enum Push {
    Msg(WsMsg),
    /// State changed; send a fresh status now instead of waiting for the timer.
    Poke,
}

pub struct Entry {
    pub song: Song,
    pub counter: u64,
    pub compiled: Compiled,
    pub draft: Option<Song>,
    pub committed: Option<(u32, Song)>,
}

impl Entry {
    pub fn dirty(&self) -> bool {
        self.draft.as_ref() != Some(&self.song)
    }

    pub fn committed_clean(&self) -> bool {
        self.committed.as_ref().is_some_and(|(_, s)| *s == self.song)
    }

    pub fn revision(&self) -> Option<u32> {
        self.committed.as_ref().map(|(n, _)| *n)
    }

    fn working(&self) -> WorkingSong {
        WorkingSong {
            song: self.song.clone(),
            edit_counter: self.counter,
            dirty: self.dirty(),
            revision: self.revision(),
            issues: self.compiled.issues.clone(),
        }
    }
}

struct Lease {
    client: String,
    name: String,
    last_seen: Instant,
}

pub struct Show {
    pub storage: Storage,
    pub project: Project,
    pub presets: Vec<Preset>,
    pub songs: BTreeMap<String, Entry>,
    pub current: Option<String>,
    pub transport: TransportState,
    pub media: Vec<MediaInfo>,
    /// Ids handed to imports that are still decoding.
    media_reserved: HashSet<String>,
    audio: AudioEngine,
    /// Each media file as one channel at the device rate, ready to play.
    audio_cache: HashMap<String, Arc<Vec<f32>>>,
    audio_loading: HashSet<String>,
    audio_failed: HashSet<String>,
    loaded_tx: mpsc::Sender<(String, Result<Vec<f32>, String>)>,
    loaded_rx: mpsc::Receiver<(String, Result<Vec<f32>, String>)>,
    audio_drift: f64,
    frame_f: f64,
    lease: Option<Lease>,
    lease_ttl: Duration,
    rig: Box<dyn RigAdapter>,
    jog_deadline: Option<Instant>,
    /// Axes that Record writes.
    record_axes: Vec<Axis>,
    /// The pass being captured, while there is one.
    capture: Option<Capture>,
    lock_source: Option<String>,
    /// The simulator was positioned by hand; the monitor shows it until the playhead moves.
    manual: bool,
    started: Instant,
    last_tick: Instant,
    log: RunLog,
    tx: broadcast::Sender<Push>,
}

pub type Shared = Arc<Mutex<Show>>;

/// What an import needs to do its work without holding the show.
#[derive(Clone, Debug)]
pub struct MediaTicket {
    pub id: String,
    pub name: String,
    pub dir: PathBuf,
    /// Folder for the upload while it arrives.
    pub incoming: PathBuf,
    pub rate: FrameRate,
    pub device_rate: u32,
}

fn lerp_pose(a: &Pose, b: &Pose, t: f64) -> Pose {
    Pose::from_fn(|ax| a.get(ax) + (b.get(ax) - a.get(ax)) * t)
}

impl Show {
    pub fn open(cfg: &Config) -> std::io::Result<(Show, broadcast::Receiver<Push>)> {
        let storage = Storage::open(&cfg.data)?;
        if cfg.seed && storage.is_fresh() {
            crate::seed::seed(&storage)?;
        }
        let project = storage.load_project();
        if storage.is_fresh() {
            storage.save_project(&project)?;
        }
        let presets = storage.load_presets();
        let mut songs = BTreeMap::new();
        for id in storage.song_ids() {
            let Some(song) = storage.load_working(&id) else {
                eprintln!("warning: songs/{id}/working.json does not parse; skipped");
                continue;
            };
            let compiled = compile(&song, &project);
            let draft = storage.load_draft(&id);
            let committed = storage.latest_revision(&id).map(|(info, s)| (info.number, s));
            songs.insert(id, Entry { song, counter: 0, compiled, draft, committed });
        }
        let current = storage.load_current().filter(|id| songs.contains_key(id)).or_else(|| songs.keys().next().cloned());
        let (tx, rx) = broadcast::channel(256);
        let now = Instant::now();
        let mut log = RunLog::open(&storage.log_dir());
        log.append("server_start", json!({ "version": env!("CARGO_PKG_VERSION"), "songs": songs.len() }));
        let media = media::load_all(&storage.media_root(), project.rate);
        let audio = AudioEngine::start(cfg.audio.clone());
        let (loaded_tx, loaded_rx) = mpsc::channel();
        let mut show = Show {
            storage,
            project,
            presets,
            songs,
            current,
            transport: TransportState::IDLE,
            media,
            media_reserved: HashSet::new(),
            audio,
            audio_cache: HashMap::new(),
            audio_loading: HashSet::new(),
            audio_failed: HashSet::new(),
            loaded_tx,
            loaded_rx,
            audio_drift: 0.0,
            frame_f: 0.0,
            lease: None,
            lease_ttl: cfg.lease_ttl,
            rig: Box::new(Simulator::new()),
            jog_deadline: None,
            record_axes: Vec::new(),
            capture: None,
            lock_source: None,
            manual: false,
            started: now,
            last_tick: now,
            log,
            tx,
        };
        show.rebuild_mix();
        Ok((show, rx))
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Push> {
        self.tx.subscribe()
    }

    fn push(&self, msg: WsMsg) {
        let _ = self.tx.send(Push::Msg(msg));
    }

    fn poke(&self) {
        let _ = self.tx.send(Push::Poke);
    }

    fn log(&mut self, kind: &str, data: serde_json::Value) {
        let record = self.log.append(kind, data);
        self.push(WsMsg::Log { record });
    }

    pub fn log_since(&self, seq: u64) -> Vec<LogRecord> {
        self.log.since(seq)
    }

    pub fn rate(&self) -> FrameRate {
        self.project.rate
    }

    fn entry(&self) -> Option<&Entry> {
        self.current.as_ref().and_then(|id| self.songs.get(id))
    }

    fn length(&self) -> i64 {
        self.entry().map(|e| e.song.length_frames).unwrap_or(0)
    }

    pub fn is_playing(&self) -> bool {
        self.transport.run != Run::Stopped || self.transport.output == Output::Positioning || self.rig.is_moving()
    }

    // ---- lease ----

    pub fn holds_lease(&self, client: &str) -> bool {
        self.lease.as_ref().is_some_and(|l| l.client == client)
    }

    fn require_lease(&self, client: &str) -> Res<()> {
        if self.holds_lease(client) {
            Ok(())
        } else {
            Err(Fail::refused(NO_LEASE))
        }
    }

    /// Any request or heartbeat from the holder keeps the lease alive.
    pub fn touch(&mut self, client: &str) {
        if let Some(l) = &mut self.lease {
            if l.client == client {
                l.last_seen = Instant::now();
            }
        }
    }

    pub fn take_lease(&mut self, client: &str, req: &LeaseRequest) -> Res<()> {
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Fail::bad("Give this station a name"));
        }
        if client.is_empty() {
            return Err(Fail::bad("The x-client-id header is required to hold the lease"));
        }
        if let Some(l) = &self.lease {
            if l.client != client && !req.force {
                return Err(Fail::refused(format!("{} holds the edit lease. Take it over to edit", l.name)));
            }
        }
        let previous = self.lease.as_ref().filter(|l| l.client != client).map(|l| l.name.clone());
        self.lease = Some(Lease { client: client.to_string(), name: name.to_string(), last_seen: Instant::now() });
        self.log("lease", json!({ "holder": name, "took_from": previous }));
        self.poke();
        Ok(())
    }

    pub fn release_lease(&mut self, client: &str) -> Res<()> {
        if self.holds_lease(client) {
            let name = self.lease.take().map(|l| l.name);
            self.log("lease", json!({ "released_by": name }));
            self.poke();
        }
        Ok(())
    }

    // ---- songs ----

    pub fn list_songs(&self) -> Vec<SongSummary> {
        self.songs
            .values()
            .map(|e| SongSummary {
                id: e.song.id.clone(),
                name: e.song.name.clone(),
                start_tc: e.song.start_tc.clone(),
                length_frames: e.song.length_frames,
                revision: e.revision(),
                dirty: e.dirty(),
                loaded: self.current.as_deref() == Some(e.song.id.as_str()),
            })
            .collect()
    }

    fn song_entry(&self, id: &str) -> Res<&Entry> {
        self.songs.get(id).ok_or_else(|| Fail::not_found(format!("No song with id {id}")))
    }

    pub fn working(&self, id: &str) -> Res<WorkingSong> {
        Ok(self.song_entry(id)?.working())
    }

    pub fn create_song(&mut self, client: &str, req: &NewSong) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Fail::bad("Give the song a name"));
        }
        let rate = self.rate();
        let start = req.start_tc.clone().unwrap_or_else(|| "01:00:00:00".into());
        let Some(tc) = Timecode::parse_at(&start, rate) else {
            return Err(Fail::invalid(format!("{start} is not a timecode at {} fps", rate.label())));
        };
        let id = self.storage.unique_song_id(name);
        let length = (rate.fps() * 60.0).round() as i64;
        let song = Song::new(&id, name, &tc.to_string(), length);
        self.storage.save_working(&song).map_err(Fail::io)?;
        self.storage.save_draft(&song).map_err(Fail::io)?;
        let compiled = compile(&song, &self.project);
        let entry = Entry { song: song.clone(), counter: 0, compiled, draft: Some(song), committed: None };
        let out = entry.working();
        self.songs.insert(id.clone(), entry);
        if self.current.is_none() {
            self.current = Some(id.clone());
            self.reset_for_new_song();
        }
        self.log("song_created", json!({ "song": id }));
        self.push(WsMsg::SongListChanged);
        Ok(out)
    }

    fn require_parked(&self, what: &str) -> Res<()> {
        if self.transport.run != Run::Stopped {
            return Err(Fail::refused(format!("Stop before {what}")));
        }
        if self.transport.output != Output::Rehearse {
            return Err(Fail::refused(format!("Disarm before {what}")));
        }
        Ok(())
    }

    pub fn load_song(&mut self, client: &str, id: &str) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let out = self.song_entry(id)?.working();
        if self.current.as_deref() != Some(id) {
            self.require_parked("loading another song")?;
            self.current = Some(id.to_string());
            self.reset_for_new_song();
            self.log("song_loaded", json!({ "song": id }));
            self.push(WsMsg::SongListChanged);
            self.poke();
        }
        Ok(out)
    }

    fn reset_for_new_song(&mut self) {
        self.audio.stop();
        self.audio.seek(0.0, self.rate());
        self.frame_f = 0.0;
        self.manual = false;
        self.lock_source = None;
        let _ = self.storage.save_current(self.current.as_deref());
        self.transport = TransportState { estop: self.transport.estop, ..TransportState::IDLE };
        self.rebuild_mix();
    }

    pub fn delete_song(&mut self, client: &str, id: &str) -> Res<()> {
        self.require_lease(client)?;
        self.song_entry(id)?;
        if self.current.as_deref() == Some(id) {
            self.require_parked("deleting the loaded song")?;
            self.current = self.songs.keys().find(|k| k.as_str() != id).cloned();
            self.reset_for_new_song();
        }
        self.storage.delete_song(id).map_err(Fail::io)?;
        self.songs.remove(id);
        self.log("song_deleted", json!({ "song": id }));
        self.push(WsMsg::SongListChanged);
        self.poke();
        Ok(())
    }

    fn require_editable(&self, id: &str) -> Res<()> {
        if self.current.as_deref() == Some(id) && self.transport.output != Output::Rehearse {
            return Err(Fail::refused("Disarm to edit the song"));
        }
        Ok(())
    }

    fn replace_working(&mut self, id: &str, song: Song, reason: &str) -> Res<WorkingSong> {
        self.storage.save_working(&song).map_err(Fail::io)?;
        let compiled = compile(&song, &self.project);
        let entry = self.songs.get_mut(id).ok_or_else(|| Fail::not_found("No such song"))?;
        entry.song = song;
        entry.compiled = compiled;
        entry.counter += 1;
        let out = entry.working();
        let counter = entry.counter;
        if self.current.as_deref() == Some(id) {
            let length = self.length() as f64;
            if self.frame_f > length {
                self.frame_f = length;
                self.audio.seek(length, self.rate());
            }
            self.rebuild_mix();
        }
        self.push(WsMsg::SongChanged { song_id: id.to_string(), edit_counter: counter, reason: reason.to_string() });
        Ok(out)
    }

    pub fn put_working(&mut self, client: &str, id: &str, put: PutWorking) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let entry = self.song_entry(id)?;
        self.require_editable(id)?;
        if put.expected_counter != entry.counter {
            return Err(Fail::refused(format!(
                "This song changed on the server (edit {}, this window has edit {}). Reload it",
                entry.counter, put.expected_counter
            )));
        }
        if put.song.id != id {
            return Err(Fail::bad("The song id does not match the address"));
        }
        put.song.validate(self.rate()).map_err(|e| Fail::invalid(e.to_string()))?;
        self.replace_working(id, put.song, "edit")
    }

    pub fn save_song(&mut self, client: &str, id: &str) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let entry = self.song_entry(id)?;
        let song = entry.song.clone();
        self.storage.save_draft(&song).map_err(Fail::io)?;
        let entry = self.songs.get_mut(id).unwrap();
        entry.draft = Some(song);
        let out = entry.working();
        self.push(WsMsg::SongListChanged);
        self.poke();
        Ok(out)
    }

    pub fn revert_song(&mut self, client: &str, id: &str) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let entry = self.song_entry(id)?;
        self.require_editable(id)?;
        let Some(draft) = entry.draft.clone() else {
            return Err(Fail::refused("There is no saved draft to revert to"));
        };
        let out = self.replace_working(id, draft, "revert")?;
        self.push(WsMsg::SongListChanged);
        Ok(out)
    }

    pub fn copy_song(&mut self, client: &str, id: &str, req: &NewSong) -> Res<WorkingSong> {
        self.require_lease(client)?;
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Fail::bad("Give the copy a name"));
        }
        let mut song = self.song_entry(id)?.song.clone();
        let new_id = self.storage.unique_song_id(name);
        song.id = new_id.clone();
        song.name = name.to_string();
        self.storage.save_working(&song).map_err(Fail::io)?;
        self.storage.save_draft(&song).map_err(Fail::io)?;
        let compiled = compile(&song, &self.project);
        let entry = Entry { song: song.clone(), counter: 0, compiled, draft: Some(song), committed: None };
        let out = entry.working();
        self.songs.insert(new_id.clone(), entry);
        self.log("song_copied", json!({ "from": id, "to": new_id }));
        self.push(WsMsg::SongListChanged);
        Ok(out)
    }

    pub fn commit_song(&mut self, client: &str, id: &str) -> Res<RevisionInfo> {
        self.require_lease(client)?;
        let entry = self.song_entry(id)?;
        if entry.dirty() {
            return Err(Fail::refused("Save the song before committing"));
        }
        if entry.committed_clean() {
            return Err(Fail::refused(format!("Revision {} already holds this song", entry.revision().unwrap_or(0))));
        }
        let song = entry.song.clone();
        let by = self.lease.as_ref().map(|l| l.name.clone()).unwrap_or_default();
        let info = self.storage.commit(&song, &by).map_err(Fail::io)?;
        self.songs.get_mut(id).unwrap().committed = Some((info.number, song));
        self.log("song_committed", json!({ "song": id, "revision": info.number, "sha256": info.sha256, "by": by }));
        self.push(WsMsg::SongListChanged);
        self.poke();
        Ok(info)
    }

    pub fn revisions(&self, id: &str) -> Res<Vec<RevisionInfo>> {
        self.song_entry(id)?;
        Ok(self.storage.revisions(id))
    }

    // ---- project and presets ----

    pub fn put_project(&mut self, client: &str, project: Project) -> Res<Project> {
        self.require_lease(client)?;
        if self.transport.output != Output::Rehearse {
            return Err(Fail::refused("Disarm to change project settings"));
        }
        if project.rate != self.project.rate {
            self.require_parked("changing the frame rate")?;
            for e in self.songs.values() {
                if let Err(err) = e.song.validate(project.rate) {
                    return Err(Fail::invalid(format!("{} does not fit {} fps: {err}", e.song.name, project.rate.label())));
                }
            }
        }
        for a in Axis::ALL {
            let l = project.axes.get(a);
            if !(l.min < l.max && l.max_speed > 0.0 && l.max_accel > 0.0 && l.settle_s >= 0.0) {
                return Err(Fail::invalid(format!(
                    "{} limits are not usable: min must be below max, speeds above zero",
                    a.label()
                )));
            }
        }
        self.storage.save_project(&project).map_err(Fail::io)?;
        let rate_changed = project.rate != self.project.rate;
        self.project = project.clone();
        for e in self.songs.values_mut() {
            e.compiled = compile(&e.song, &project);
        }
        if rate_changed {
            for m in &mut self.media {
                m.duration_frames = media::duration_frames(m.samples, m.sample_rate, project.rate);
            }
            self.push(WsMsg::MediaChanged);
        }
        self.rebuild_mix();
        self.log("project_changed", json!({ "by": self.lease.as_ref().map(|l| l.name.clone()) }));
        self.push(WsMsg::ProjectChanged);
        self.poke();
        Ok(project)
    }

    fn unique_preset_id(&self, name: &str) -> String {
        let base = storage::slug(name);
        if !self.presets.iter().any(|p| p.id == base) {
            return base;
        }
        (2..).map(|n| format!("{base}-{n}")).find(|id| !self.presets.iter().any(|p| &p.id == id)).unwrap()
    }

    pub fn store_preset(&mut self, client: &str, req: &StorePreset) -> Res<Preset> {
        self.require_lease(client)?;
        let name = req.name.trim();
        if name.is_empty() {
            return Err(Fail::bad("Give the preset a name"));
        }
        let pose = req.pose.unwrap_or_else(|| self.monitor_pose().0);
        let preset =
            Preset { id: self.unique_preset_id(name), name: name.to_string(), pose, camera_number: None, has_thumbnail: false };
        self.presets.push(preset.clone());
        self.storage.save_presets(&self.presets).map_err(Fail::io)?;
        self.push(WsMsg::PresetsChanged);
        Ok(preset)
    }

    pub fn delete_preset(&mut self, client: &str, id: &str) -> Res<()> {
        self.require_lease(client)?;
        let before = self.presets.len();
        self.presets.retain(|p| p.id != id);
        if self.presets.len() == before {
            return Err(Fail::not_found(format!("No preset with id {id}")));
        }
        self.storage.save_presets(&self.presets).map_err(Fail::io)?;
        self.push(WsMsg::PresetsChanged);
        Ok(())
    }

    /// Pull presets from the rig. Device presets are matched by camera number.
    pub fn sync_presets(&mut self, client: &str) -> Res<Vec<Preset>> {
        self.require_lease(client)?;
        let from_rig = self.rig.presets().map_err(Fail::refused)?;
        for mut p in from_rig {
            if let Some(existing) =
                self.presets.iter_mut().find(|e| e.camera_number.is_some() && e.camera_number == p.camera_number)
            {
                existing.pose = p.pose;
                existing.name = p.name;
            } else {
                p.id = self.unique_preset_id(&p.name);
                self.presets.push(p);
            }
        }
        self.storage.save_presets(&self.presets).map_err(Fail::io)?;
        self.log("presets_synced", json!({ "count": self.presets.len() }));
        self.push(WsMsg::PresetsChanged);
        Ok(self.presets.clone())
    }

    // ---- media and audio ----

    pub fn media_info(&self, id: &str) -> Res<&MediaInfo> {
        self.media.iter().find(|m| m.id == id).ok_or_else(|| Fail::not_found(format!("No media with id {id}")))
    }

    /// First half of an import: pick an id and a folder. The decode happens outside the show lock.
    pub fn reserve_media(&mut self, client: &str, name: &str) -> Res<MediaTicket> {
        self.require_lease(client)?;
        let name = name.trim();
        if name.is_empty() {
            return Err(Fail::bad("The upload needs a file name: /media?name=<file name>"));
        }
        let ext = media::extension(name);
        if !media::AUDIO_EXTENSIONS.contains(&ext.as_str()) && !media::VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            return Err(Fail::invalid(format!(
                "{name} was not imported: only WAV, AIFF and MP3 audio and QuickTime or MP4 video can be used"
            )));
        }
        let stem = std::path::Path::new(name).file_stem().and_then(|s| s.to_str()).unwrap_or(name);
        let base = storage::slug(stem);
        let taken = |id: &str| self.media.iter().any(|m| m.id == id) || self.media_reserved.contains(id);
        let id = if taken(&base) { (2..).map(|n| format!("{base}-{n}")).find(|id| !taken(id)).unwrap() } else { base };
        let dir = self.storage.media_dir(&id).ok_or_else(|| Fail::bad("That file name cannot be used"))?;
        self.media_reserved.insert(id.clone());
        Ok(MediaTicket {
            id,
            name: name.to_string(),
            dir,
            incoming: self.storage.media_root(),
            rate: self.rate(),
            device_rate: self.audio.rate(),
        })
    }

    pub fn cancel_media(&mut self, id: &str) {
        self.media_reserved.remove(id);
    }

    /// Second half of an import: the file decoded, add it to the library.
    pub fn register_media(&mut self, ingested: Ingested, playable: Option<Vec<f32>>) -> MediaInfo {
        let mut info = ingested.info;
        self.media_reserved.remove(&info.id);
        info.duration_frames = media::duration_frames(info.samples, info.sample_rate, self.rate());
        if let Some(mono) = playable {
            self.audio_cache.insert(info.id.clone(), Arc::new(mono));
        }
        self.media.retain(|m| m.id != info.id);
        self.media.push(info.clone());
        self.media.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
        self.log(
            "media_imported",
            json!({ "media": info.id, "name": info.name, "seconds": info.duration_s, "channels": info.channels, "rate": info.sample_rate }),
        );
        self.push(WsMsg::MediaChanged);
        self.rebuild_mix();
        info
    }

    pub fn delete_media(&mut self, client: &str, id: &str) -> Res<()> {
        self.require_lease(client)?;
        self.media_info(id)?;
        if let Some(e) = self.songs.values().find(|e| e.song.media.iter().any(|c| c.media_id == id)) {
            return Err(Fail::refused(format!("{} uses this file. Remove it from the timeline first", e.song.name)));
        }
        if let Some(dir) = self.storage.media_dir(id) {
            if dir.exists() {
                std::fs::remove_dir_all(dir).map_err(Fail::io)?;
            }
        }
        self.media.retain(|m| m.id != id);
        self.audio_cache.remove(id);
        self.audio_failed.remove(id);
        self.log("media_deleted", json!({ "media": id }));
        self.push(WsMsg::MediaChanged);
        Ok(())
    }

    pub fn media_file(&self, id: &str, file: &str) -> Res<PathBuf> {
        self.media_info(id)?;
        Ok(self.storage.media_dir(id).ok_or_else(|| Fail::not_found("No such media"))?.join(file))
    }

    /// Load a media file's audio for playback on a worker thread; `tick` collects it.
    fn load_audio(&mut self, id: &str) {
        if self.audio_loading.contains(id) || self.audio_failed.contains(id) {
            return;
        }
        let Some(dir) = self.storage.media_dir(id) else { return };
        let exclude = self.media.iter().find(|m| m.id == id).and_then(|m| m.ltc_channel);
        let (tx, id, device_rate) = (self.loaded_tx.clone(), id.to_string(), self.audio.rate());
        self.audio_loading.insert(id.clone());
        let spawned = std::thread::Builder::new().name("audio-load".into()).spawn(move || {
            let result = media::read_audio(&dir.join("audio.f32"))
                .and_then(|d| crate::audio::resample(&d.mono(exclude), d.rate, device_rate));
            let _ = tx.send((id, result));
        });
        if spawned.is_err() {
            self.audio_loading.clear();
        }
    }

    fn collect_loaded_audio(&mut self) {
        let mut changed = false;
        while let Ok((id, result)) = self.loaded_rx.try_recv() {
            self.audio_loading.remove(&id);
            match result {
                Ok(mono) => {
                    self.audio_cache.insert(id, Arc::new(mono));
                }
                Err(e) => {
                    self.audio_failed.insert(id.clone());
                    self.log("media_error", json!({ "media": id, "error": e }));
                }
            }
            changed = true;
        }
        if changed {
            self.rebuild_mix();
        }
    }

    /// The media changed (for example its timecode channel): load its audio again.
    pub fn reload_audio(&mut self, id: &str) {
        self.audio_cache.remove(id);
        self.audio_failed.remove(id);
        self.rebuild_mix();
    }

    /// Hand the audio thread what the loaded song plays.
    fn rebuild_mix(&mut self) {
        let rate = self.rate();
        let device_rate = self.audio.rate();
        let mut clips = Vec::new();
        let mut missing = Vec::new();
        let mut ltc = None;
        if let Some(e) = self.entry() {
            for c in e.song.media.iter().filter(|c| !c.muted) {
                let Some(data) = self.audio_cache.get(&c.media_id) else {
                    missing.push(c.media_id.clone());
                    continue;
                };
                let offset = frame_to_sample(c.in_frame, device_rate, rate);
                let len = (frame_to_sample(c.out_frame, device_rate, rate) - offset).min(data.len() as i64 - offset);
                if len > 0 {
                    clips.push(PlayClip {
                        start: frame_to_sample(c.start_frame, device_rate, rate),
                        offset,
                        len,
                        data: data.clone(),
                    });
                }
            }
            let start_code = Timecode::parse_at(&e.song.start_tc, rate).map(|tc| tc.to_frames(rate)).unwrap_or(0);
            ltc = Some(LtcOut { rate, start_code, level: self.project.ltc_level });
        }
        self.audio.set_mix(Mix { clips, routing: self.project.routing, ltc });
        for id in missing {
            self.load_audio(&id);
        }
    }

    fn audio_status(&self) -> AudioStatus {
        let info = self.audio.info();
        let (peak_left, peak_right) = self.audio.peaks();
        AudioStatus {
            output: info.name,
            silent_reason: info.silent_reason,
            device_rate: self.audio.rate(),
            routing: self.project.routing,
            callbacks: self.audio.callbacks(),
            position_frame: if self.audio.is_playing() { self.audio.position(self.rate()) } else { self.frame_f },
            drift_frames: self.audio_drift,
            peak_left,
            peak_right,
        }
    }

    // ---- transport ----

    fn context(&self, client: &str) -> Context {
        let entry = self.entry();
        Context {
            has_lease: self.holds_lease(client),
            song_loaded: entry.is_some(),
            committed_clean: entry.is_some_and(|e| e.committed_clean()),
            blocking_issues: entry.map(|e| e.compiled.blocking_count(&e.song)).unwrap_or(0),
            adapter_problem: self.rig.problem(),
        }
    }

    fn who(&self, client: &str) -> String {
        match &self.lease {
            Some(l) if l.client == client => l.name.clone(),
            _ => format!("watcher {client}"),
        }
    }

    pub fn act(&mut self, client: &str, action: Action, req: &TransportRequest) -> Res<()> {
        let ctx = self.context(client);
        transport::check(&self.transport, &ctx, action).map_err(|b| Fail::refused(b.text))?;
        let rate = self.rate();
        let length = self.length() as f64;
        let before = self.transport;
        match action {
            Action::Play => {
                if self.frame_f >= length - 0.5 {
                    self.frame_f = 0.0;
                }
                self.manual = false;
                if !self.transport.locked() {
                    self.audio.play(self.frame_f, rate);
                }
            }
            Action::Stop => {
                self.audio.stop();
                if before.hardware_live() {
                    self.rig.stop();
                }
            }
            Action::Cue => {
                self.audio.stop();
                self.audio.seek(0.0, rate);
                self.frame_f = 0.0;
                self.manual = false;
            }
            Action::Seek => {
                let Some(frame) = req.frame else {
                    return Err(Fail::bad("Seek needs a frame"));
                };
                self.frame_f = (frame as f64).clamp(0.0, length);
                self.manual = false;
                self.audio.seek(self.frame_f, rate);
            }
            Action::Lock => {
                let source = req.source.clone().unwrap_or_default();
                if !matches!(source.as_str(), "ltc" | "mtc" | "artnet") {
                    return Err(Fail::bad("Lock needs a source: ltc, mtc, artnet or file:<clip id>"));
                }
                self.lock_source = Some(source);
            }
            Action::Unlock => {
                self.lock_source = None;
                if self.transport.run == Run::Playing {
                    self.audio.play(self.frame_f, rate);
                }
            }
            Action::Arm => {}
            Action::Disarm => self.rig.stop(),
            Action::RecordOn => {
                let axes = match &req.axes {
                    Some(given) => Axis::ALL.into_iter().filter(|a| given.contains(a)).collect(),
                    None => self.record_axes.clone(),
                };
                if axes.is_empty() {
                    return Err(Fail::refused("Record-enable at least one axis lane first"));
                }
                self.record_axes = axes;
            }
            Action::RecordOff => {}
            Action::Estop => {
                self.audio.stop();
                self.rig.stop();
                let _ = self.rig.jog(Jog::default());
                self.jog_deadline = None;
            }
            Action::ResetEstop => {}
        }
        self.transport = transport::apply(&before, action, true);
        let frame = self.frame_f.floor() as i64;
        self.log("transport", json!({ "action": action.key(), "by": self.who(client), "frame": frame, "state": self.transport }));
        self.sync_capture();
        // The virtual rig follows a moved playhead at once, not a tick later.
        if self.rig.is_simulator() && self.transport.output == Output::Rehearse && self.capture.is_none() && !self.manual {
            self.rig.sync_virtual(&self.timeline_pose(), &Axis::ALL);
        }
        self.poke();
        Ok(())
    }

    fn event(&mut self, event: Event) {
        let before = self.transport;
        let after = transport::on_event(&before, event);
        if after == before {
            return;
        }
        match event {
            Event::SongEnded => self.audio.stop(),
            Event::Fault => {
                self.audio.stop();
                self.rig.stop();
            }
            _ => {}
        }
        self.transport = after;
        self.log("event", json!({ "event": format!("{event:?}"), "frame": self.frame_f.floor() as i64, "state": after }));
        self.sync_capture();
        self.poke();
    }

    // ---- record ----

    fn recording(&self) -> bool {
        self.transport.record && self.transport.run == Run::Playing
    }

    /// Start a pass when Record and Play are both on; end it when either goes off.
    fn sync_capture(&mut self) {
        if self.recording() {
            if self.capture.is_none() && self.entry().is_some() {
                // The pass includes the frame Play was pressed on.
                let mut capture = Capture::new(self.record_axes.clone());
                capture.feed(self.frame_f - self.rate().seconds_to_frames(self.rig.query_delay_s()), self.rig.pose());
                self.capture = Some(capture);
                let axes = self.record_axes.clone();
                self.log("record_start", json!({ "frame": self.frame_f.ceil() as i64, "axes": axes }));
            }
        } else if let Some(capture) = self.capture.take() {
            self.finish_pass(capture);
        }
    }

    fn free_pass_id(&self, song: &str) -> String {
        (1..)
            .map(|n| format!("pass-{n:04}"))
            .find(|id| self.storage.pass_path(song, id).is_some_and(|p| !p.exists()))
            .unwrap_or_else(|| "pass".into())
    }

    /// Turn a finished capture into keys on the timeline. The pass replaces
    /// what was on its axes over the frames it covers.
    fn finish_pass(&mut self, capture: Capture) {
        let _ = self.rig.jog(Jog::default());
        self.jog_deadline = None;
        self.manual = false;
        let Some(song_id) = self.current.clone() else { return };
        let frames = capture.frames() as i64;
        if frames < 2 {
            self.log("record_discarded", json!({ "song": song_id, "reason": "The pass was shorter than two frames" }));
            return;
        }
        let pass = RecordedPass {
            id: self.free_pass_id(&song_id),
            song_id: song_id.clone(),
            recorded_at: storage::now_rfc3339(),
            rate: self.rate(),
            start_frame: capture.start_frame(),
            query_delay_s: self.rig.query_delay_s(),
            samples: capture.samples(),
        };
        let saved = match self.storage.pass_path(&song_id, &pass.id) {
            Some(path) => storage::write_json(&path, &pass).map_err(|e| e.to_string()),
            None => Err("the pass has no usable file name".into()),
        };
        if let Err(e) = &saved {
            self.log(
                "record_warning",
                json!({ "song": song_id, "pass": pass.id, "problem": format!("Raw samples were not kept: {e}") }),
            );
        }
        let curves = record::fit_pass(&pass.samples, 1.0);
        let keys: BTreeMap<Axis, usize> = curves.iter().map(|(a, k)| (*a, k.len())).collect();
        let Some(entry) = self.songs.get(&song_id) else { return };
        let punched = edit::punch_keys(
            &entry.song,
            &self.project,
            pass.start_frame,
            frames - 1,
            curves,
            saved.is_ok().then(|| pass.id.clone()),
        );
        match punched {
            Ok((song, clip)) => match self.replace_working(&song_id, song, "record") {
                Ok(_) => self.log(
                    "record_pass",
                    json!({
                        "song": song_id, "clip": clip, "pass": pass.id, "start_frame": pass.start_frame,
                        "frames": frames, "axes": capture.axes, "keys": keys,
                    }),
                ),
                Err(fail) => self.log("record_failed", json!({ "song": song_id, "pass": pass.id, "reason": fail.msg })),
            },
            Err(refusal) => self.log("record_failed", json!({ "song": song_id, "pass": pass.id, "reason": refusal.to_string() })),
        }
    }

    /// Fit a recorded clip again from its raw samples, tighter or looser.
    pub fn refit(&mut self, client: &str, clip_id: &str, req: &Refit) -> Res<WorkingSong> {
        self.require_lease(client)?;
        if !(req.scale.is_finite() && (0.05..=50.0).contains(&req.scale)) {
            return Err(Fail::bad("The fit tolerance scale must be between 0.05 and 50"));
        }
        let Some(entry) = self.entry() else {
            return Err(Fail::refused("Create or load a song first"));
        };
        let song_id = entry.song.id.clone();
        self.require_editable(&song_id)?;
        let Some(clip) = entry.song.motion_clip(clip_id) else {
            return Err(Fail::not_found(format!("No clip with id {clip_id} in {}", entry.song.name)));
        };
        let Some(pass_id) = &clip.recorded_pass else {
            return Err(Fail::refused("This clip was not recorded, so there is nothing to refit"));
        };
        let pass: Option<RecordedPass> = self.storage.pass_path(&song_id, pass_id).and_then(|p| storage::read_json(&p));
        let Some(pass) = pass else {
            return Err(Fail::refused(format!("The raw samples of {pass_id} are missing from the data folder")));
        };
        let whole = clip.axes.iter().all(|a| pass.samples.get(a).is_some_and(|v| v.len() as i64 == clip.duration_frames + 1));
        if !whole {
            return Err(Fail::refused("This clip was trimmed or split after it was recorded. Refit works on a whole pass"));
        }
        let samples: BTreeMap<Axis, Vec<f64>> = pass.samples.into_iter().filter(|(a, _)| clip.axes.contains(a)).collect();
        let curves = record::fit_pass(&samples, req.scale);
        let song = edit::replace_curves(&entry.song, &self.project, clip_id, curves).map_err(|r| Fail::invalid(r.to_string()))?;
        let out = self.replace_working(&song_id, song, "refit")?;
        self.log(
            "record_refit",
            json!({ "song": song_id, "clip": clip_id, "pass": pass_id_of(&out.song, clip_id), "scale": req.scale }),
        );
        Ok(out)
    }

    pub fn jog(&mut self, client: &str, jog: Jog) -> Res<()> {
        self.require_lease(client)?;
        if self.transport.estop {
            return Err(Fail::refused("Reset E-Stop to continue"));
        }
        if self.transport.output != Output::Rehearse {
            return Err(Fail::refused("Disarm to drive the rig by hand"));
        }
        if self.transport.run != Run::Stopped && !self.transport.record {
            return Err(Fail::refused("Stop to position the rig by hand, or turn Record on to capture the move"));
        }
        // During a pass only the record-enabled axes are hand-driven; the rest play the timeline.
        let jog = if self.transport.run != Run::Stopped {
            let keep = |a: Axis| if self.record_axes.contains(&a) { jog.get(a) } else { 0.0 };
            Jog {
                pan: keep(Axis::Pan),
                tilt: keep(Axis::Tilt),
                zoom: keep(Axis::Zoom),
                focus: keep(Axis::Focus),
                elevation: keep(Axis::Elevation),
            }
        } else {
            jog
        };
        self.rig.jog(jog).map_err(Fail::refused)?;
        if jog.is_zero() {
            self.jog_deadline = None;
        } else {
            self.manual = true;
            // The app repeats jog while a control is held; a dropped connection stops the rig.
            self.jog_deadline = Some(Instant::now() + Duration::from_millis(250));
        }
        Ok(())
    }

    // ---- tick ----

    fn timeline_pose(&self) -> Pose {
        let Some(e) = self.entry() else { return Pose::home() };
        let poses = &e.compiled.poses;
        if poses.is_empty() {
            return e.song.home_pose;
        }
        let f = self.frame_f.clamp(0.0, (poses.len() - 1) as f64);
        let i = f.floor() as usize;
        let j = (i + 1).min(poses.len() - 1);
        lerp_pose(&poses[i], &poses[j], f - i as f64)
    }

    /// The pose the monitor shows, and where it comes from.
    fn monitor_pose(&self) -> (Pose, PoseSource) {
        if self.transport.hardware_live() {
            (self.rig.pose(), PoseSource::Rig)
        } else if self.transport.record && self.transport.run != Run::Stopped {
            (self.rig.pose(), PoseSource::Record)
        } else if self.manual && self.rig.is_simulator() {
            (self.rig.pose(), PoseSource::Rig)
        } else {
            (self.timeline_pose(), PoseSource::Timeline)
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f64().min(0.25);
        self.last_tick = now;

        if self.lease.as_ref().is_some_and(|l| now.duration_since(l.last_seen) > self.lease_ttl) {
            let name = self.lease.take().map(|l| l.name);
            self.log("lease", json!({ "expired": name }));
            self.poke();
        }
        if self.jog_deadline.is_some_and(|d| now > d) {
            let _ = self.rig.jog(Jog::default());
            self.jog_deadline = None;
        }
        self.collect_loaded_audio();

        if self.entry().is_some() {
            let length = self.length() as f64;
            // One reading of the audio clock per tick: the playhead and the drift figure share it.
            let audio_pos = self.audio.position(self.rate());
            if self.transport.run == Run::Playing && self.transport.time != TimeMode::LockedExternal {
                let f = audio_pos;
                if f >= length {
                    self.frame_f = length;
                    self.event(Event::SongEnded);
                } else {
                    self.frame_f = f;
                }
            }
            self.audio_drift = if self.audio.is_playing() { audio_pos - self.frame_f } else { 0.0 };

            let target = self.timeline_pose();
            if self.transport.output == Output::Positioning {
                self.rig.drive(&target, self.project.rig.positioning_speed);
                let tol = self.project.rig.tolerance;
                let pose = self.rig.pose();
                if Axis::ALL.iter().all(|a| (pose.get(*a) - target.get(*a)).abs() <= tol.get(*a) * 0.25) {
                    self.event(Event::RigArrived);
                }
            } else if self.transport.output == Output::Armed {
                self.rig.drive(&target, 1.0);
            } else if self.rig.is_simulator() {
                if self.capture.is_some() {
                    let playing: Vec<Axis> = Axis::ALL.into_iter().filter(|a| !self.record_axes.contains(a)).collect();
                    self.rig.sync_virtual(&target, &playing);
                } else if !self.manual {
                    self.rig.sync_virtual(&target, &Axis::ALL);
                }
            }
        }
        self.rig.tick(dt, &self.project.axes);
        if self.capture.is_some() {
            // The pose just read belongs to where the transport was `query_delay_s` ago.
            let stamp = self.frame_f - self.rate().seconds_to_frames(self.rig.query_delay_s());
            let pose = self.rig.pose();
            if let Some(capture) = &mut self.capture {
                capture.feed(stamp, pose);
            }
        }
    }

    // ---- status ----

    pub fn status(&self, client: &str) -> Status {
        let rate = self.rate();
        let entry = self.entry();
        let frame = self.frame_f.floor() as i64;
        let (pose, pose_source) = self.monitor_pose();
        let timecode = match entry {
            Some(e) => e.song.clock(rate).timecode_at(frame).to_string(),
            None => Timecode::from_frames(0, rate).to_string(),
        };
        let now = Instant::now();
        Status {
            server_time_ms: now.duration_since(self.started).as_millis() as u64,
            song: entry.map(|e| SongStatus {
                id: e.song.id.clone(),
                name: e.song.name.clone(),
                start_tc: e.song.start_tc.clone(),
                length_frames: e.song.length_frames,
                revision: e.revision(),
                dirty: e.dirty(),
                committed_clean: e.committed_clean(),
                edit_counter: e.counter,
                issues: e.compiled.issues.len(),
                blocking: e.compiled.blocking_count(&e.song),
            }),
            transport: self.transport,
            frame,
            frame_f: self.frame_f,
            timecode,
            rate,
            pose,
            pose_source,
            rig_pose: self.rig.pose(),
            hardware_live: self.transport.hardware_live(),
            lease: self.lease.as_ref().map(|l| LeaseStatus {
                holder: l.name.clone(),
                client_id: l.client.clone(),
                expires_in_ms: self.lease_ttl.saturating_sub(now.duration_since(l.last_seen)).as_millis() as u64,
            }),
            you_hold_lease: self.holds_lease(client),
            guidance: transport::guidance(&self.transport, &self.context(client)),
            audio: self.audio_status(),
            lock: LockStatus { source: self.lock_source.clone(), ..LockStatus::default() },
            adapters: self.rig.status(),
            record: RecordStatus {
                axes: self.record_axes.clone(),
                active: self.capture.is_some(),
                frames: self.capture.as_ref().map(|c| c.frames() as i64).unwrap_or(0),
            },
            telemetry: Telemetry::default(),
        }
    }
}

fn pass_id_of(song: &Song, clip: &str) -> Option<String> {
    song.motion_clip(clip).and_then(|c| c.recorded_pass.clone())
}

/// Start the 60 Hz tick thread. It stops when the last other handle is dropped.
pub fn spawn_tick(show: &Shared) {
    let weak = Arc::downgrade(show);
    std::thread::Builder::new()
        .name("show-tick".into())
        .spawn(move || {
            let period = Duration::from_micros(16_667);
            let mut next = Instant::now() + period;
            loop {
                let Some(show) = weak.upgrade() else { break };
                if let Ok(mut s) = show.lock() {
                    s.tick();
                }
                drop(show);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                }
                next = (next + period).max(Instant::now());
            }
        })
        .expect("tick thread");
}
