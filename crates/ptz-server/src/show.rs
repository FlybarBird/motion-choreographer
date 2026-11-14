//! Authoritative show state: songs, transport, clock, hardware fan-out.

use crate::audio_engine::AudioEngine;
use crate::hardware::{AutoPodSimulator, Fr7Simulator, SharedAutoPod, SharedFr7};
use crate::timecode_io::InternalClock;
use parking_lot::Mutex;
use ptz_core::audio::{decode_audio, waveform_peaks};
use ptz_core::transport::{
    TransportAction, TransportPolicy, TransportSnapshot,
};
use ptz_core::{
    demo_presets, demo_songs, interpolate_pose, AudioClipMeta, LiveState, MoveClip,
    ProjectSettings, RigPose, RigPreset, ServerEvent, ShowStatus, Song, API_VERSION,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::broadcast;
use tracing::{info, warn};
use uuid::Uuid;

pub struct ShowEngine {
    pub settings: Mutex<ProjectSettings>,
    songs: Mutex<HashMap<String, Song>>,
    presets: Mutex<Vec<RigPreset>>,
    transport: Mutex<TransportSnapshot>,
    clock: Arc<InternalClock>,
    audio: Arc<AudioEngine>,
    fr7: SharedFr7,
    autopod: SharedAutoPod,
    data_dir: PathBuf,
    audio_files: Mutex<HashMap<String, PathBuf>>,
    events: broadcast::Sender<ServerEvent>,
    /// Pose used as baseline before first move.
    home_pose: Mutex<RigPose>,
}

impl ShowEngine {
    pub fn new(data_dir: PathBuf) -> Arc<Self> {
        std::fs::create_dir_all(&data_dir).ok();
        std::fs::create_dir_all(data_dir.join("audio")).ok();
        let settings = ProjectSettings::default();
        let rate = settings.frame_rate;
        let songs: HashMap<_, _> = demo_songs(rate).into_iter().map(|s| (s.id.clone(), s)).collect();
        let active = songs.keys().next().cloned();
        let (tx, _) = broadcast::channel(256);
        let fr7_sim = Arc::new(Fr7Simulator::new());
        let autopod_sim = Arc::new(AutoPodSimulator::new());
        let clock = Arc::new(InternalClock::new(rate));
        let audio = Arc::new(AudioEngine::new());
        audio.set_routing(settings.audio_routing.clone());

        let engine = Arc::new(Self {
            settings: Mutex::new(settings),
            songs: Mutex::new(songs),
            presets: Mutex::new(demo_presets()),
            transport: Mutex::new(TransportSnapshot {
                phase: ptz_core::TransportPhase::Idle,
                mode: ptz_core::TransportMode::Idle,
                dirty: false,
                committed: true,
                playhead_frames: 0,
                song_id: active,
                hardware_live: false,
            }),
            clock,
            audio,
            fr7: fr7_sim as SharedFr7,
            autopod: autopod_sim as SharedAutoPod,
            data_dir,
            audio_files: Mutex::new(HashMap::new()),
            events: tx,
            home_pose: Mutex::new(RigPose::default()),
        });
        engine
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerEvent> {
        self.events.subscribe()
    }

    pub fn publish_status(&self) {
        let status = self.status();
        let _ = self.events.send(ServerEvent::Status(status));
    }

    pub fn status(&self) -> ShowStatus {
        let transport = self.transport.lock().clone();
        let guidance = TransportPolicy::guidance(&transport);
        let settings = self.settings.lock().clone();
        let playhead = transport.playhead_frames;
        let rate = settings.frame_rate;
        let pose = self.current_pose(playhead);
        let song_id = transport.song_id.clone();
        let smpte = if let Some(id) = &song_id {
            self.songs
                .lock()
                .get(id)
                .map(|s| {
                    s.smpte_start
                        .offset(playhead)
                        .display()
                })
                .unwrap_or_else(|| ptz_core::Timecode::from_seconds(0.0, rate).display())
        } else {
            ptz_core::Timecode::zero(rate).display()
        };
        ShowStatus {
            api_version: API_VERSION.to_string(),
            transport,
            guidance,
            settings,
            active_song_id: song_id,
            live: LiveState {
                pose,
                playhead_frames: playhead,
                playhead_tc: smpte,
                song_seconds: rate.frames_to_seconds(playhead),
            },
            server_time_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        }
    }

    pub fn current_pose(&self, frame: i64) -> RigPose {
        let home = *self.home_pose.lock();
        let transport = self.transport.lock().clone();
        let Some(id) = transport.song_id.clone() else {
            return home;
        };
        let songs = self.songs.lock();
        let Some(song) = songs.get(&id) else {
            return home;
        };
        interpolate_pose(&song.moves, frame, home)
    }

    pub fn list_songs(&self) -> Vec<Song> {
        let mut v: Vec<_> = self.songs.lock().values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn get_song(&self, id: &str) -> Option<Song> {
        self.songs.lock().get(id).cloned()
    }

    pub fn presets(&self) -> Vec<RigPreset> {
        self.presets.lock().clone()
    }

    pub fn select_song(&self, id: &str) -> Result<Song, String> {
        let song = self
            .songs
            .lock()
            .get(id)
            .cloned()
            .ok_or_else(|| format!("song not found: {id}"))?;
        {
            let mut t = self.transport.lock();
            t.song_id = Some(id.to_string());
            t.playhead_frames = 0;
            t.dirty = song.dirty;
            t.committed = song.committed;
            t.phase = if song.committed && !song.dirty {
                ptz_core::TransportPhase::Idle
            } else {
                ptz_core::TransportPhase::Editing
            };
            t.mode = ptz_core::TransportMode::Idle;
            t.hardware_live = false;
        }
        self.clock.seek_frames(0);
        self.audio.cue();
        if let Some(audio) = &song.audio {
            if let Some(path) = self.audio_files.lock().get(&audio.asset_id).cloned() {
                if let Ok(decoded) = decode_audio(&path) {
                    self.audio.load_program(&decoded);
                }
            }
        } else {
            self.audio.clear();
        }
        self.publish_status();
        Ok(song)
    }

    pub fn save_song(&self, mut song: Song) -> Result<Song, String> {
        song.dirty = false;
        song.committed = true;
        song.revision += 1;
        let id = song.id.clone();
        self.songs.lock().insert(id.clone(), song.clone());
        {
            let mut t = self.transport.lock();
            if t.song_id.as_deref() == Some(id.as_str()) {
                if TransportPolicy::can(&t, TransportAction::SaveCommit).is_ok() {
                    *t = TransportPolicy::apply(&t, TransportAction::SaveCommit)
                        .map_err(|e| e.to_string())?;
                } else {
                    t.dirty = false;
                    t.committed = true;
                    if t.phase == ptz_core::TransportPhase::Editing {
                        t.phase = ptz_core::TransportPhase::Idle;
                    }
                }
                t.song_id = Some(id);
            }
        }
        let _ = self.events.send(ServerEvent::SongUpdated {
            song: song.clone(),
        });
        self.publish_status();
        Ok(song)
    }

    pub fn update_song_draft(&self, mut song: Song) -> Result<Song, String> {
        song.dirty = true;
        song.committed = false;
        let id = song.id.clone();
        self.songs.lock().insert(id.clone(), song.clone());
        {
            let mut t = self.transport.lock();
            if t.song_id.as_deref() == Some(id.as_str()) {
                *t = TransportPolicy::mark_dirty(&t);
                t.song_id = Some(id);
            }
        }
        let _ = self.events.send(ServerEvent::SongUpdated {
            song: song.clone(),
        });
        self.publish_status();
        Ok(song)
    }

    pub fn create_song(&self, name: String) -> Song {
        let rate = self.settings.lock().frame_rate;
        let song = Song {
            id: format!("song-{}", Uuid::new_v4()),
            name,
            revision: 0,
            smpte_start: ptz_core::Timecode::zero(rate),
            duration_frames: rate.seconds_to_frames(60.0),
            markers: vec![],
            moves: vec![],
            audio: None,
            committed: false,
            dirty: true,
        };
        self.songs.lock().insert(song.id.clone(), song.clone());
        let _ = self.events.send(ServerEvent::SongUpdated {
            song: song.clone(),
        });
        self.publish_status();
        song
    }

    pub fn add_move_from_preset(
        &self,
        song_id: &str,
        preset_id: &str,
        start_frame: i64,
        duration_frames: i64,
    ) -> Result<Song, String> {
        let preset = self
            .presets
            .lock()
            .iter()
            .find(|p| p.id == preset_id)
            .cloned()
            .ok_or_else(|| format!("preset not found: {preset_id}"))?;
        let mut song = self
            .songs
            .lock()
            .get(song_id)
            .cloned()
            .ok_or_else(|| format!("song not found: {song_id}"))?;
        song.moves.push(MoveClip {
            id: Uuid::new_v4().to_string(),
            name: preset.name.clone(),
            preset_id: Some(preset.id),
            start_frame,
            duration_frames: duration_frames.max(1),
            easing: ptz_core::EaseKind::EaseInOut,
            axes: ptz_core::AxisSelection::default(),
            target: preset.pose,
            curves: vec![],
        });
        self.update_song_draft(song)
    }

    pub fn import_audio(&self, song_id: &str, filename: &str, bytes: &[u8]) -> Result<Song, String> {
        let asset_id = Uuid::new_v4().to_string();
        let ext = Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("wav");
        let dest = self.data_dir.join("audio").join(format!("{asset_id}.{ext}"));
        std::fs::write(&dest, bytes).map_err(|e| e.to_string())?;
        let decoded = decode_audio(&dest).map_err(|e| e.to_string())?;
        let wave = waveform_peaks(&decoded, 800);
        let rate = self.settings.lock().frame_rate;
        let mut song = self
            .songs
            .lock()
            .get(song_id)
            .cloned()
            .ok_or_else(|| format!("song not found: {song_id}"))?;
        song.audio = Some(AudioClipMeta {
            asset_id: asset_id.clone(),
            filename: filename.to_string(),
            duration_secs: decoded.duration_secs,
            waveform: wave.peaks,
        });
        // Extend song duration to at least audio length.
        let audio_frames = rate.seconds_to_frames(decoded.duration_secs);
        if audio_frames > song.duration_frames {
            song.duration_frames = audio_frames;
        }
        self.audio_files.lock().insert(asset_id, dest);
        self.audio.load_program(&decoded);
        self.update_song_draft(song)
    }

    pub fn update_settings(&self, settings: ProjectSettings) -> ProjectSettings {
        self.audio.set_routing(settings.audio_routing.clone());
        self.clock.set_rate(settings.frame_rate);
        *self.settings.lock() = settings.clone();
        self.publish_status();
        settings
    }

    pub fn transport_action(&self, action: TransportAction) -> Result<ShowStatus, String> {
        // Snapshot + validate before mutating side effects.
        let current = self.transport.lock().clone();
        TransportPolicy::can(&current, action).map_err(|e| e.to_string())?;
        let next = TransportPolicy::apply(&current, action).map_err(|e| e.to_string())?;

        match action {
            TransportAction::Preview => {
                self.clock.seek_frames(next.playhead_frames);
                self.clock.start();
                self.audio.seek_seconds(
                    self.settings
                        .lock()
                        .frame_rate
                        .frames_to_seconds(next.playhead_frames),
                );
                self.audio.play();
                info!("Preview started (hardware_live=false)");
            }
            TransportAction::Play => {
                self.clock.seek_frames(next.playhead_frames);
                self.clock.start();
                self.audio.seek_seconds(
                    self.settings
                        .lock()
                        .frame_rate
                        .frames_to_seconds(next.playhead_frames),
                );
                self.audio.play();
                info!("Live Play started (hardware_live=true)");
            }
            TransportAction::Stop => {
                self.clock.pause();
                self.audio.pause();
            }
            TransportAction::Cue => {
                self.clock.stop_and_cue();
                self.audio.cue();
            }
            TransportAction::Estop => {
                self.clock.pause();
                self.audio.pause();
                let fr7 = self.fr7.clone();
                let ap = self.autopod.clone();
                tokio::spawn(async move {
                    let _ = fr7.estop().await;
                    let _ = ap.estop().await;
                });
                warn!("E-Stop");
            }
            TransportAction::Prepare => {
                let fr7 = self.fr7.clone();
                let ap = self.autopod.clone();
                tokio::spawn(async move {
                    let _ = fr7.connect().await;
                    let _ = ap.connect().await;
                });
            }
            TransportAction::Arm => {
                info!("Armed for live hardware output");
            }
            TransportAction::SaveCommit => {
                if let Some(id) = current.song_id.clone() {
                    let mut songs = self.songs.lock();
                    if let Some(song) = songs.get_mut(&id) {
                        song.dirty = false;
                        song.committed = true;
                        song.revision += 1;
                    }
                }
            }
            TransportAction::ClearEstop => {}
        }

        *self.transport.lock() = next;
        // Sync song dirty/committed flags into transport after save.
        // Lock order is always transport → songs (never songs → transport).
        let song_id = self.transport.lock().song_id.clone();
        let flags = song_id.and_then(|id| {
            self.songs
                .lock()
                .get(&id)
                .map(|s| (s.dirty, s.committed))
        });
        if let Some((dirty, committed)) = flags {
            let mut t = self.transport.lock();
            t.dirty = dirty;
            t.committed = committed;
        }
        self.publish_status();
        Ok(self.status())
    }

    /// Tick from background task: advance playhead while playing, drive outputs.
    pub fn tick(&self) {
        let mut transport = self.transport.lock().clone();
        let playing = matches!(
            transport.phase,
            ptz_core::TransportPhase::LivePlaying | ptz_core::TransportPhase::PreviewPlaying
        );
        if !playing {
            return;
        }
        let rate = self.settings.lock().frame_rate;
        // Keep silent audio engine roughly in sync when no cpal device is present.
        self.audio.silent_advance(1.0 / 30.0);

        // Prefer audio cursor for A/V sync when a program clip is loaded & playing.
        let frame = if self.audio_loaded() && self.audio.is_playing() {
            rate.seconds_to_frames(self.audio.cursor_seconds())
        } else {
            self.clock.song_frames()
        };

        transport.playhead_frames = frame;
        *self.transport.lock() = transport.clone();

        let pose = self.current_pose(frame);
        let live = transport.hardware_live;
        let fr7 = self.fr7.clone();
        let ap = self.autopod.clone();
        // Simulator always tracks pose for the program monitor; `live` gates real adapters.
        tokio::spawn(async move {
            let _ = fr7.goto_ptzf(pose, live).await;
            let _ = ap.goto_elevation(pose.elevation_m, live).await;
        });

        let ended = transport.song_id.as_ref().and_then(|id| {
            self.songs
                .lock()
                .get(id)
                .map(|s| frame >= s.duration_frames)
        });
        if ended == Some(true) {
            let _ = self.transport_action(TransportAction::Stop);
            return;
        }
        self.publish_status();
    }

    fn audio_loaded(&self) -> bool {
        let song_id = self.transport.lock().song_id.clone();
        song_id
            .and_then(|id| self.songs.lock().get(&id).and_then(|s| s.audio.clone()))
            .is_some()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }
}

/// Spawn the 30 Hz show tick + status push.
pub fn spawn_ticker(engine: Arc<ShowEngine>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(33));
        loop {
            interval.tick().await;
            engine.tick();
        }
    });
}
