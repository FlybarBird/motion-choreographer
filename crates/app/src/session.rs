//! Everything the window knows about one connected server: the last status,
//! the song being edited, undo history and selection.

use crate::link::{Cmd, Event, Link, Why};
use crate::timeline::Timeline;
use ptz_core::api::*;
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

pub struct Session {
    pub link: Link,
    pub station: String,
    /// Why the connection is down, while it is.
    pub lost: Option<String>,
    pub status: Status,
    status_at: Instant,
    pub project: Project,
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
    pub frames: HashMap<(String, i64), Option<egui::TextureHandle>>,
    pub frame_jpegs: Vec<(String, i64, Vec<u8>)>,
    pub record_axes: Vec<ptz_core::Axis>,
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
        pose: Pose::home(),
        pose_source: PoseSource::Timeline,
        rig_pose: Pose::home(),
        hardware_live: false,
        lease: None,
        you_hold_lease: false,
        guidance: Default::default(),
        audio: Default::default(),
        lock: Default::default(),
        adapters: Vec::new(),
        record: Default::default(),
        telemetry: Default::default(),
    }
}

impl Session {
    pub fn new(link: Link, station: &str, hello: crate::link::Hello) -> Session {
        let mut s = Session {
            link,
            station: station.to_string(),
            lost: None,
            status: hello.status,
            status_at: Instant::now(),
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
            frames: HashMap::new(),
            frame_jpegs: Vec::new(),
            record_axes: vec![ptz_core::Axis::Pan, ptz_core::Axis::Tilt],
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
                self.status = *status;
                self.status_at = Instant::now();
                self.lost = None;
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
            Event::Presets(presets) => self.presets = presets,
            Event::Project(project) => {
                self.project = *project;
                if let Some(song) = &self.song {
                    self.compiled = compile(song, &self.project);
                }
            }
            Event::Media(media) => {
                // A file that changed or went away loses its cached waveform.
                let same = |id: &String| media.iter().find(|m| &m.id == id) == self.media.iter().find(|m| &m.id == id);
                self.peaks.retain(|id, _| same(id));
                self.peaks_requested.retain(|id, _| same(id));
                self.media = media;
            }
            Event::Peaks { media, level } => {
                self.peaks.insert(media, level);
            }
            Event::Frame { media, frame, jpeg } => self.frame_jpegs.push((media, frame, jpeg)),
            Event::PresetThumb { .. } => {}
            Event::Committed(info) => self.toast(format!("Committed revision {}", info.number), false),
            Event::Uploaded(info) => {
                self.toast(format!("Imported {}", info.name), false);
                self.link.send(Cmd::RefreshMedia);
            }
            Event::Log(record) => {
                self.log.push(record);
                if self.log.len() > 2000 {
                    self.log.drain(..500);
                }
            }
            Event::Refused { what, msg } => self.toast(format!("{what}: {msg}"), true),
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

    pub fn act(&mut self, action: Action) {
        let req = match action {
            Action::RecordOn => TransportRequest { axes: Some(self.record_axes.clone()), ..Default::default() },
            _ => TransportRequest::default(),
        };
        self.link.send(Cmd::Act(action, req));
    }

    pub fn seek(&mut self, frame: i64) {
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

    /// The pose the program monitor draws. While stopped it is evaluated here,
    /// so scrubbing and edits show without a round trip.
    pub fn monitor_pose(&self) -> (Pose, PoseSource) {
        match (&self.song, self.status.pose_source) {
            (Some(song), PoseSource::Timeline) => {
                let shown = self.timeline.preview.as_ref().unwrap_or(song);
                (eval::pose_at_f(shown, self.playhead()), PoseSource::Timeline)
            }
            _ => (self.status.pose, self.status.pose_source),
        }
    }

    pub fn timecode_at(&self, frame: i64) -> String {
        match &self.song {
            Some(song) => song.clock(self.project.rate).timecode_at(frame).to_string(),
            None => self.status.timecode.clone(),
        }
    }
}
