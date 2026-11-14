//! Premiere-style operator UI with After Effects-style timeline.

use crate::monitor::draw_program_monitor;
use crate::net::{load_backend_url, remember_backend_url, Backend, LiveFeed};
use crate::theme::{self, BLUE, CHARCOAL, DANGER, GREEN, NAVY, OFF_WHITE, WARN};
use egui::{Align, Color32, Layout, RichText, Sense, Stroke, Ui};
use ptz_core::transport::TransportPolicy;
use ptz_core::{RigPreset, ShowStatus, Song};

pub struct DirectorApp {
    backend_url: String,
    backend: Backend,
    feed: LiveFeed,
    status: Option<ShowStatus>,
    songs: Vec<Song>,
    presets: Vec<RigPreset>,
    active_song: Option<Song>,
    selected_move: Option<String>,
    timeline_zoom: f32,
    status_msg: String,
    error_msg: String,
    pixels_per_frame: f32,
}

impl DirectorApp {
    pub fn new(cc: &eframe::CreationContext<'_>, backend_url: String) -> Self {
        theme::apply(&cc.egui_ctx);
        let backend = Backend::new(&backend_url);
        let feed = LiveFeed::start(&backend_url);
        let mut app = Self {
            backend_url,
            backend,
            feed,
            status: None,
            songs: vec![],
            presets: vec![],
            active_song: None,
            selected_move: None,
            timeline_zoom: 1.0,
            status_msg: "Connecting…".into(),
            error_msg: String::new(),
            pixels_per_frame: 0.35,
        };
        app.refresh_catalog();
        app
    }

    fn refresh_catalog(&mut self) {
        match self.backend.songs() {
            Ok(s) => self.songs = s,
            Err(e) => self.error_msg = format!("songs: {e}"),
        }
        match self.backend.presets() {
            Ok(p) => self.presets = p,
            Err(e) => self.error_msg = format!("presets: {e}"),
        }
        if let Ok(st) = self.backend.status() {
            self.apply_status(st);
        }
    }

    fn apply_status(&mut self, st: ShowStatus) {
        if let Some(id) = st.active_song_id.clone() {
            if self.active_song.as_ref().map(|s| s.id.as_str()) != Some(id.as_str())
                || self
                    .active_song
                    .as_ref()
                    .map(|s| s.revision)
                    != self.songs.iter().find(|s| s.id == id).map(|s| s.revision)
            {
                if let Ok(song) = self.backend.select_song(&id) {
                    // select is idempotent-ish; prefer cached list update
                    let _ = song;
                }
                if let Some(song) = self.songs.iter().find(|s| s.id == id).cloned() {
                    self.active_song = Some(song);
                }
            }
        }
        self.status = Some(st);
    }

    fn sync_feed(&mut self) {
        if let Some(st) = self.feed.latest.lock().clone() {
            // Keep song body fresh when revision bumps via WS SongUpdated — poll songs lightly
            self.status = Some(st);
        }
        if let Some(err) = self.feed.last_error.lock().clone() {
            if self.error_msg.is_empty() {
                self.error_msg = err;
            }
        }
    }

    fn do_transport(&mut self, action: &str) {
        match self.backend.transport_with_body(action) {
            Ok(st) => {
                self.status_msg = format!("{action} OK");
                self.error_msg.clear();
                self.apply_status(st);
                // Refresh song list after commit
                if action == "save_commit" || action == "commit" {
                    self.refresh_catalog();
                }
            }
            Err(e) => {
                self.error_msg = format!("{action} rejected: {e}");
            }
        }
    }

    fn guidance_buttons(&self) -> ptz_core::TransportGuidance {
        if let Some(st) = &self.status {
            st.guidance.clone()
        } else {
            TransportPolicy::guidance(&ptz_core::TransportSnapshot::editing_default())
        }
    }
}

impl eframe::App for DirectorApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_feed();
        ctx.request_repaint_after(std::time::Duration::from_millis(33));

        egui::TopBottomPanel::top("menu")
            .frame(egui::Frame::new().fill(NAVY).inner_margin(8.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("PTZ DIRECTOR")
                            .size(22.0)
                            .color(GREEN)
                            .strong(),
                    );
                    ui.separator();
                    ui.label(RichText::new("Backend").color(OFF_WHITE.gamma_multiply(0.7)));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.backend_url)
                            .desired_width(220.0)
                            .hint_text("http://127.0.0.1:8787"),
                    );
                    if ui.button("Connect").clicked()
                        || (resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)))
                    {
                        let _ = remember_backend_url(&self.backend_url);
                        self.backend = Backend::new(&self.backend_url);
                        self.feed = LiveFeed::start(&self.backend_url);
                        self.refresh_catalog();
                        self.status_msg = format!("Connected to {}", self.backend_url);
                        self.error_msg.clear();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let live = self.feed.connected.lock();
                        let (dot, label) = if *live {
                            (GREEN, "LIVE FEED")
                        } else {
                            (WARN, "RECONNECTING")
                        };
                        ui.label(RichText::new(label).color(dot).small());
                        ui.colored_label(dot, "●");
                    });
                });
            });

        egui::TopBottomPanel::bottom("transport")
            .frame(egui::Frame::new().fill(CHARCOAL).inner_margin(10.0))
            .show(ctx, |ui| {
                self.ui_transport(ui);
            });

        egui::SidePanel::left("browser")
            .default_width(220.0)
            .frame(egui::Frame::new().fill(NAVY).inner_margin(8.0))
            .show(ctx, |ui| {
                self.ui_browser(ui);
            });

        egui::SidePanel::right("inspector")
            .default_width(260.0)
            .frame(egui::Frame::new().fill(NAVY).inner_margin(8.0))
            .show(ctx, |ui| {
                self.ui_inspector(ui);
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(NAVY).inner_margin(6.0))
            .show(ctx, |ui| {
                let total_h = ui.available_height();
                let monitor_h = (total_h * 0.42).clamp(160.0, 360.0);
                ui.allocate_ui(egui::vec2(ui.available_width(), monitor_h), |ui| {
                    let pose = self
                        .status
                        .as_ref()
                        .map(|s| s.live.pose)
                        .unwrap_or_default();
                    let playing = self.status.as_ref().map(|s| {
                        matches!(
                            s.transport.phase,
                            ptz_core::TransportPhase::LivePlaying
                                | ptz_core::TransportPhase::PreviewPlaying
                        )
                    }).unwrap_or(false);
                    let preview = self.status.as_ref().map(|s| {
                        matches!(
                            s.transport.phase,
                            ptz_core::TransportPhase::PreviewPlaying
                                | ptz_core::TransportPhase::PreviewPaused
                        )
                    }).unwrap_or(false);
                    draw_program_monitor(ui, &pose, playing, preview);
                });
                ui.add_space(4.0);
                self.ui_timeline(ui);
            });
    }
}

impl DirectorApp {
    fn ui_browser(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("SONGS").color(BLUE).strong());
        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                let songs = self.songs.clone();
                for song in songs {
                    let selected = self
                        .active_song
                        .as_ref()
                        .map(|s| s.id == song.id)
                        .unwrap_or(false);
                    let label = format!(
                        "{}{}",
                        song.name,
                        if song.dirty { " •" } else { "" }
                    );
                    if ui.selectable_label(selected, label).clicked() {
                        match self.backend.select_song(&song.id) {
                            Ok(s) => {
                                self.active_song = Some(s);
                                self.refresh_catalog();
                            }
                            Err(e) => self.error_msg = e.to_string(),
                        }
                    }
                }
            });
        ui.add_space(8.0);
        ui.label(RichText::new("RIG PRESETS").color(BLUE).strong());
        ui.label(
            RichText::new("Drag onto timeline (or click + Add at playhead)")
                .small()
                .color(OFF_WHITE.gamma_multiply(0.6)),
        );
        let presets = self.presets.clone();
        for preset in presets {
            ui.horizontal(|ui| {
                ui.label(&preset.name);
                if ui.small_button("Add").clicked() {
                    self.add_preset_at_playhead(&preset.id);
                }
            });
            // Drag payload
            let drag = ui.allocate_response(egui::vec2(ui.available_width(), 4.0), Sense::drag());
            if drag.dragged() {
                ui.ctx().memory_mut(|m| {
                    m.data.insert_temp(egui::Id::new("drag_preset"), preset.id.clone());
                });
            }
        }
        ui.add_space(8.0);
        if ui.button("Import audio…").clicked() {
            if let Some(song) = &self.active_song {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("Audio", &["wav", "aiff", "aif", "mp3"])
                    .pick_file()
                {
                    match self.backend.upload_audio(&song.id, &path) {
                        Ok(s) => {
                            self.active_song = Some(s);
                            self.refresh_catalog();
                            self.status_msg = "Audio imported".into();
                        }
                        Err(e) => self.error_msg = format!("audio import: {e}"),
                    }
                }
            } else {
                self.error_msg = "Select a song first".into();
            }
        }
    }

    fn add_preset_at_playhead(&mut self, preset_id: &str) {
        let Some(song) = self.active_song.clone() else {
            self.error_msg = "No song selected".into();
            return;
        };
        let start = self
            .status
            .as_ref()
            .map(|s| s.transport.playhead_frames)
            .unwrap_or(0);
        let dur = self
            .status
            .as_ref()
            .map(|s| s.settings.frame_rate.seconds_to_frames(8.0))
            .unwrap_or(200);
        match self.backend.add_move(&song.id, preset_id, start, dur) {
            Ok(s) => {
                self.active_song = Some(s);
                self.refresh_catalog();
                self.status_msg = "Move added — Save/Commit before live Play".into();
            }
            Err(e) => self.error_msg = e.to_string(),
        }
    }

    fn ui_inspector(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("CLIP INSPECTOR").color(BLUE).strong());
        let song = self.active_song.clone();
        let selected = self.selected_move.clone();
        if let (Some(song), Some(mid)) = (song, selected) {
            if let Some(mv) = song.moves.iter().find(|m| m.id == mid).cloned() {
                ui.label(RichText::new(&mv.name).strong());
                ui.label(format!("Start frame: {}", mv.start_frame));
                ui.label(format!("Duration: {} frames", mv.duration_frames));
                ui.label(format!("Easing: {}", mv.easing.label()));
                ui.label(format!(
                    "Axes: pan={} tilt={} zoom={} elev={}",
                    mv.axes.pan, mv.axes.tilt, mv.axes.zoom, mv.axes.elevation
                ));
                ui.separator();
                ui.label(format!("Target pan {:+.1}°", mv.target.pan_deg));
                ui.label(format!("Target tilt {:+.1}°", mv.target.tilt_deg));
                ui.label(format!("Target zoom {:.0}%", mv.target.zoom * 100.0));
                ui.label(format!("Target elev {:.2} m", mv.target.elevation_m));

                ui.add_space(6.0);
                let mut start = mv.start_frame as f32;
                let mut dur = mv.duration_frames as f32;
                if ui
                    .add(egui::Slider::new(&mut start, 0.0..=song.duration_frames as f32).text("Start"))
                    .changed()
                    || ui
                        .add(egui::Slider::new(&mut dur, 1.0..=5000.0).text("Duration"))
                        .changed()
                {
                    self.trim_move(&song.id, &mv.id, start as i64, dur as i64);
                }
            }
        } else {
            ui.label(
                RichText::new("Select a move on the timeline")
                    .color(OFF_WHITE.gamma_multiply(0.55)),
            );
        }

        ui.add_space(12.0);
        ui.label(RichText::new("PROJECT").color(BLUE).strong());
        if let Some(st) = &self.status {
            ui.label(format!("Frame rate: {}", st.settings.frame_rate.label()));
            ui.label(format!("Clock: {}", st.settings.clock_source.label()));
            ui.label(format!(
                "Audio L: {:?}  R: {:?}  LTC gen: {}",
                st.settings.audio_routing.left,
                st.settings.audio_routing.right,
                if st.settings.audio_routing.ltc_generator_enabled {
                    "ON"
                } else {
                    "OFF"
                }
            ));
        }
    }

    fn trim_move(&mut self, song_id: &str, move_id: &str, start: i64, dur: i64) {
        let Some(mut song) = self.songs.iter().find(|s| s.id == song_id).cloned() else {
            return;
        };
        if let Some(mv) = song.moves.iter_mut().find(|m| m.id == move_id) {
            mv.start_frame = start.max(0);
            mv.duration_frames = dur.max(1);
        }
        match self.backend.put_song(&song) {
            Ok(s) => {
                self.active_song = Some(s.clone());
                if let Some(slot) = self.songs.iter_mut().find(|x| x.id == s.id) {
                    *slot = s;
                }
            }
            Err(e) => self.error_msg = e.to_string(),
        }
    }

    fn ui_transport(&mut self, ui: &mut Ui) {
        let g = self.guidance_buttons();
        let st = self.status.clone();
        let tc = st
            .as_ref()
            .map(|s| s.live.playhead_tc.clone())
            .unwrap_or_else(|| "--:--:--:--".into());
        let phase = st
            .as_ref()
            .map(|s| format!("{:?}", s.transport.phase))
            .unwrap_or_else(|| "—".into());

        ui.horizontal(|ui| {
            ui.label(RichText::new(&tc).monospace().size(20.0).color(GREEN));
            ui.separator();
            ui.label(RichText::new(phase).small().color(BLUE));
            ui.separator();

            self.transport_btn(ui, "Save/Commit", g.buttons.save_commit, GREEN, "save_commit");
            self.transport_btn(ui, "Prepare", g.buttons.prepare, BLUE, "prepare");
            self.transport_btn(ui, "Arm", g.buttons.arm, WARN, "arm");
            self.transport_btn(ui, "Play", g.buttons.play, GREEN, "play");
            ui.separator();
            self.transport_btn(ui, "Preview", g.buttons.preview, BLUE, "preview");
            self.transport_btn(ui, "Stop", g.buttons.stop, OFF_WHITE, "stop");
            self.transport_btn(ui, "Cue", g.buttons.cue, OFF_WHITE, "cue");
            ui.separator();
            if g.buttons.clear_estop {
                self.transport_btn(ui, "Clear E-Stop", true, WARN, "clear_estop");
            } else {
                self.transport_btn(ui, "E-Stop", g.buttons.estop, DANGER, "estop");
            }
        });

        ui.horizontal(|ui| {
            ui.label(RichText::new("Next:").color(OFF_WHITE.gamma_multiply(0.6)));
            ui.label(RichText::new(&g.next_hint).color(GREEN));
            if let Some(reason) = g.play_blocked_reason.as_ref() {
                if !g.buttons.play {
                    ui.label(RichText::new(format!("| Play blocked: {reason}")).color(WARN).small());
                }
            }
        });

        if !self.status_msg.is_empty() {
            ui.label(RichText::new(&self.status_msg).small().color(OFF_WHITE.gamma_multiply(0.7)));
        }
        if !self.error_msg.is_empty() {
            ui.label(RichText::new(&self.error_msg).small().color(DANGER));
        }
    }

    fn transport_btn(&mut self, ui: &mut Ui, label: &str, enabled: bool, color: Color32, action: &str) {
        let text = RichText::new(label).color(if enabled { color } else { Color32::from_rgb(90, 90, 100) });
        let btn = egui::Button::new(text);
        if ui.add_enabled(enabled, btn).clicked() {
            self.do_transport(action);
        }
    }

    fn ui_timeline(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("TIMELINE").color(BLUE).strong());
            ui.add(egui::Slider::new(&mut self.timeline_zoom, 0.4..=3.0).text("Zoom"));
            self.pixels_per_frame = 0.25 * self.timeline_zoom;
        });

        let song = self.active_song.clone();
        let Some(song) = song else {
            ui.label("Select a song");
            return;
        };
        let playhead = self
            .status
            .as_ref()
            .map(|s| s.transport.playhead_frames)
            .unwrap_or(0);
        let rate = self
            .status
            .as_ref()
            .map(|s| s.settings.frame_rate)
            .unwrap_or(ptz_core::FrameRate::Fps25);

        let width = (song.duration_frames as f32 * self.pixels_per_frame).max(ui.available_width());
        let track_h = 28.0;
        let ruler_h = 22.0;
        let audio_h = 40.0;
        let height = ruler_h + audio_h + track_h * 3.0 + 16.0;

        egui::ScrollArea::horizontal().show(ui, |ui| {
            let (resp, painter) = ui.allocate_painter(egui::vec2(width, height), Sense::click_and_drag());
            let rect = resp.rect;
            painter.rect_filled(rect, 0.0, CHARCOAL);

            // Ruler
            let ruler = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), ruler_h));
            painter.rect_filled(ruler, 0.0, NAVY);
            let step = rate.seconds_to_frames(1.0).max(1);
            let mut f = 0i64;
            while f <= song.duration_frames {
                let x = rect.left() + f as f32 * self.pixels_per_frame;
                painter.line_segment(
                    [egui::pos2(x, ruler.top() + 10.0), egui::pos2(x, ruler.bottom())],
                    Stroke::new(1.0_f32, OFF_WHITE.gamma_multiply(0.25)),
                );
                let tc = song.smpte_start.offset(f).display();
                painter.text(
                    egui::pos2(x + 3.0, ruler.top() + 2.0),
                    egui::Align2::LEFT_TOP,
                    tc,
                    egui::FontId::monospace(10.0),
                    OFF_WHITE.gamma_multiply(0.7),
                );
                f += step * 5;
            }

            // Markers
            for m in &song.markers {
                let x = rect.left() + m.frame as f32 * self.pixels_per_frame;
                painter.line_segment(
                    [egui::pos2(x, ruler.top()), egui::pos2(x, rect.bottom())],
                    Stroke::new(1.0_f32, WARN.gamma_multiply(0.7)),
                );
            }

            // Audio track
            let audio_y = ruler.bottom() + 4.0;
            let audio_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left(), audio_y),
                egui::vec2(rect.width(), audio_h),
            );
            painter.rect_filled(audio_rect, 0.0, Color32::from_rgb(0x1a, 0x22, 0x3a));
            painter.text(
                egui::pos2(audio_rect.left() + 4.0, audio_rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                "Audio",
                egui::FontId::proportional(10.0),
                BLUE,
            );
            if let Some(audio) = &song.audio {
                let peaks = &audio.waveform;
                let clip_w = (audio.duration_secs as f32
                    * rate.exact_fps() as f32
                    * self.pixels_per_frame)
                    .min(audio_rect.width());
                let mid = audio_rect.center().y;
                for (i, peak) in peaks.iter().enumerate() {
                    let x = audio_rect.left() + (i as f32 / peaks.len() as f32) * clip_w;
                    let h = *peak * (audio_h * 0.4);
                    painter.line_segment(
                        [egui::pos2(x, mid - h), egui::pos2(x, mid + h)],
                        Stroke::new(1.0_f32, GREEN.gamma_multiply(0.8)),
                    );
                }
            }

            // Move track
            let move_y = audio_rect.bottom() + 6.0;
            let move_rect = egui::Rect::from_min_size(
                egui::pos2(rect.left(), move_y),
                egui::vec2(rect.width(), track_h),
            );
            painter.rect_filled(move_rect, 0.0, Color32::from_rgb(0x14, 0x1c, 0x34));
            painter.text(
                egui::pos2(move_rect.left() + 4.0, move_rect.top() + 2.0),
                egui::Align2::LEFT_TOP,
                "Moves (PTZ / Pedestal)",
                egui::FontId::proportional(10.0),
                BLUE,
            );

            for mv in &song.moves {
                let x = rect.left() + mv.start_frame as f32 * self.pixels_per_frame;
                let w = (mv.duration_frames as f32 * self.pixels_per_frame).max(8.0);
                let clip = egui::Rect::from_min_size(
                    egui::pos2(x, move_rect.top() + 14.0),
                    egui::vec2(w, track_h - 14.0),
                );
                let selected = self.selected_move.as_deref() == Some(mv.id.as_str());
                painter.rect_filled(
                    clip,
                    3.0,
                    if selected {
                        BLUE
                    } else {
                        Color32::from_rgb(0x2a, 0x4a, 0x6e)
                    },
                );
                painter.rect_stroke(
                    clip,
                    3.0,
                    Stroke::new(1.0_f32, if selected { GREEN } else { BLUE }),
                    egui::StrokeKind::Outside,
                );
                painter.text(
                    clip.left_top() + egui::vec2(4.0, 2.0),
                    egui::Align2::LEFT_TOP,
                    &mv.name,
                    egui::FontId::proportional(11.0),
                    OFF_WHITE,
                );
            }

            // Playhead
            let phx = rect.left() + playhead as f32 * self.pixels_per_frame;
            painter.line_segment(
                [egui::pos2(phx, rect.top()), egui::pos2(phx, rect.bottom())],
                Stroke::new(2.0_f32, GREEN),
            );

            // Interactions: click select move / drop preset
            if resp.clicked() {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let mut hit = None;
                    for mv in &song.moves {
                        let x = rect.left() + mv.start_frame as f32 * self.pixels_per_frame;
                        let w = (mv.duration_frames as f32 * self.pixels_per_frame).max(8.0);
                        let clip = egui::Rect::from_min_size(
                            egui::pos2(x, move_rect.top() + 14.0),
                            egui::vec2(w, track_h - 14.0),
                        );
                        if clip.contains(pos) {
                            hit = Some(mv.id.clone());
                            break;
                        }
                    }
                    self.selected_move = hit;
                }
            }
            if resp.dragged() {
                // drop preset if any
            }
            if resp.drag_stopped() {
                let preset_id: Option<String> = ui.ctx().memory(|m| {
                    m.data.get_temp(egui::Id::new("drag_preset"))
                });
                if let (Some(pid), Some(pos)) = (preset_id, resp.interact_pointer_pos()) {
                    let frame = ((pos.x - rect.left()) / self.pixels_per_frame).max(0.0) as i64;
                    let dur = rate.seconds_to_frames(8.0);
                    match self.backend.add_move(&song.id, &pid, frame, dur) {
                        Ok(s) => {
                            self.active_song = Some(s);
                            self.refresh_catalog();
                        }
                        Err(e) => self.error_msg = e.to_string(),
                    }
                    ui.ctx().memory_mut(|m| {
                        m.data.remove::<String>(egui::Id::new("drag_preset"));
                    });
                }
            }
        });

    }
}

pub fn initial_backend_url(cli: Option<String>) -> String {
    cli.unwrap_or_else(load_backend_url)
}
