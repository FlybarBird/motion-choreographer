//! Whole-window tests: real egui input events through the real widgets, with
//! a detached link in place of the server.

use crate::app::Shell;
use crate::link::{Cmd, Event, Link};
use crate::session::Session;
use crate::settings_ui::Page;
use crate::timeline::{self, Lane};
use egui::{pos2, vec2, Pos2};
use egui_kittest::Harness;
use ptz_core::pose::{Pose, Preset, Project};
use ptz_core::song::{ClipBody, Song};
use std::sync::mpsc::{Receiver, Sender};

pub fn presets() -> Vec<Preset> {
    let p = |id: &str, name: &str, pan: f64, tilt: f64, zoom: f64, elevation: f64| Preset {
        id: id.into(),
        name: name.into(),
        pose: Pose { pan, tilt, zoom, focus: 0.5, elevation },
        camera: ptz_core::camera::first_camera(),
        camera_number: None,
        has_thumbnail: false,
    };
    vec![
        p("wide", "Stage Wide", 0.0, 2.0, 0.05, 120.0),
        p("lectern", "Lectern MCU", -24.0, -1.5, 0.62, 310.0),
        p("awards", "Awards Two-Shot", 18.0, 0.5, 0.38, 220.0),
    ]
}

pub struct Rig {
    pub harness: Harness<'static, Shell>,
    pub cmds: Receiver<Cmd>,
    #[allow(dead_code)]
    pub events: Sender<Event>,
    /// The pictures the window has uploaded, for [`Rig::picture`].
    textures: picture::Textures,
}

pub fn rig(song: Song) -> Rig {
    rig_with(Project::default(), presets(), song)
}

/// Three cameras: the first has the three usual presets, the second one of its own, the third none.
pub fn three_cameras() -> (Project, Vec<Preset>) {
    let mut project = Project::default();
    project.cameras[0].name = "Stage left".into();
    for (id, name, x) in [("cam2", "Centre", 4.0), ("cam3", "Balcony", 8.0)] {
        let mut camera = ptz_core::CameraConfig::new(id, name);
        camera.mount.x = x;
        project.cameras.push(camera);
    }
    let mut all = presets();
    let mut drums = Preset::new("drums", "Drums", Pose { pan: 12.0, tilt: -3.0, zoom: 0.4, focus: 0.5, elevation: 200.0 });
    drums.camera = "cam2".into();
    all.push(drums);
    (project, all)
}

/// The size the window opens at, and the smallest it can be made (main.rs).
pub const DEFAULT_WINDOW: egui::Vec2 = vec2(1500.0, 920.0);
pub const SMALLEST_WINDOW: egui::Vec2 = vec2(1100.0, 700.0);

pub fn rig_with(project: Project, presets: Vec<Preset>, song: Song) -> Rig {
    rig_sized(DEFAULT_WINDOW, project, presets, song)
}

pub fn rig_sized(window: egui::Vec2, project: Project, presets: Vec<Preset>, song: Song) -> Rig {
    let (link, cmds, events) = Link::detached();
    let session = Session::offline(link, project, presets, song);
    let shell = Shell::new(session, None);
    let mut themed = false;
    let textures = picture::Textures::default();
    let harness = Harness::builder().with_size(window).renderer(textures.clone()).build_ui_state(
        move |ui, shell: &mut Shell| {
            if !themed {
                // Fonts set on the context are bound from the next pass, so the first one draws nothing.
                crate::theme::apply(ui.ctx());
                themed = true;
                return;
            }
            shell.ui(ui);
        },
        shell,
    );
    let mut rig = Rig { harness, cmds, events, textures };
    rig.harness.run_steps(4);
    rig
}

impl Rig {
    fn song(&self) -> &Song {
        self.harness.state().session.song.as_ref().unwrap()
    }

    fn layout(&self) -> timeline::Layout {
        let s = &self.harness.state().session;
        timeline::layout(self.song(), &s.project, &s.timeline.view, s.probe_timeline.expect("the timeline was drawn"))
    }

    fn lane_point(&self, lane: Lane, frame: f64) -> Pos2 {
        let l = self.layout();
        pos2(l.x_of(frame), l.lane_rect(lane).unwrap().center().y)
    }

    fn preset_point(&self, id: &str) -> Pos2 {
        let s = &self.harness.state().session;
        s.probe_presets.iter().find(|(p, _)| p == id).expect("the preset card was drawn").1.center()
    }

    /// Press, move in steps and release, one frame per event.
    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.harness.hover_at(from);
        self.harness.run_steps(2);
        self.harness.drag_at(from);
        self.harness.run_steps(2);
        for i in 1..=6 {
            self.harness.hover_at(from + (to - from) * (i as f32 / 6.0));
            self.harness.run_steps(1);
        }
        self.harness.run_steps(2);
        self.harness.drop_at(to);
        self.harness.run_steps(3);
    }

    fn puts(&self) -> Vec<Song> {
        self.cmds.try_iter().filter_map(|c| if let Cmd::PutWorking { song, .. } = c { Some(song) } else { None }).collect()
    }
}

#[test]
fn dragging_a_preset_from_the_browser_drops_a_move_on_the_timeline() {
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    assert!(r.song().motion.is_empty());

    let from = r.preset_point("lectern");
    let to = r.lane_point(Lane::Motion(0), 200.0);
    r.drag(from, to);

    let song = r.song().clone();
    assert_eq!(song.motion.len(), 1, "one move was created");
    let clip = &song.motion[0];
    assert!((clip.start_frame - 200).abs() <= 2, "it starts where it was dropped: {}", clip.start_frame);
    match &clip.body {
        ClipBody::PresetMove { auto_duration, to, .. } => {
            assert!(*auto_duration, "a dropped preset uses auto time");
            assert_eq!(to.name, "Lectern MCU");
        }
        other => panic!("expected a preset move, got {other:?}"),
    }
    assert!(clip.duration_frames > 25, "auto time is derived from the travel: {}", clip.duration_frames);

    // The edit went to the server and is one undo step.
    let puts = r.puts();
    assert_eq!(puts.last(), Some(&song));
    assert_eq!(r.harness.state().session.undo.len(), 1);

    // A second preset dropped later links from the first.
    let from = r.preset_point("awards");
    let to = r.lane_point(Lane::Motion(0), 700.0);
    r.drag(from, to);
    assert_eq!(r.song().motion.len(), 2);
    let label = timeline::move_label(r.song(), &r.song().motion[1].id, &Project::default());
    assert!(label.starts_with("Lectern MCU to Awards Two-Shot"), "{label}");
}

#[test]
fn dragging_a_preset_onto_a_move_replaces_where_it_goes() {
    let project = Project::default();
    let (song, id) =
        ptz_core::edit::insert_preset_move(&Song::new("s", "Test Song", "01:00:00:00", 1500), &project, &presets()[1], 200)
            .unwrap();
    let mut r = rig(song);
    let before = r.song().motion_clip(&id).unwrap().clone();

    let from = r.preset_point("awards");
    let mid = (before.start_frame + before.end_frame()) as f64 / 2.0;
    r.drag(from, r.lane_point(Lane::Motion(0), mid));

    assert_eq!(r.song().motion.len(), 1, "the move is replaced, not stacked");
    let after = r.song().motion_clip(&id).unwrap();
    assert_eq!(after.start_frame, before.start_frame, "the move stays where it was");
    match &after.body {
        ClipBody::PresetMove { to, auto_duration, .. } => {
            assert_eq!(to.name, "Awards Two-Shot");
            assert!(*auto_duration, "auto time is kept; it may retake from the new travel");
        }
        other => panic!("expected a preset move, got {other:?}"),
    }
    assert_eq!(r.puts().len(), 1);
}

#[test]
fn ctrl_s_and_cmd_s_save_the_song() {
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    let _ = r.cmds.try_iter().count();
    r.harness.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::S);
    r.harness.run_steps(2);
    let cmds: Vec<Cmd> = r.cmds.try_iter().collect();
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Save(id) if id == "s")), "Ctrl-S saves: {cmds:?}");

    r.harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::S);
    r.harness.run_steps(2);
    let cmds: Vec<Cmd> = r.cmds.try_iter().collect();
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Save(id) if id == "s")), "Cmd-S saves: {cmds:?}");
}

#[test]
fn dragging_the_end_of_a_move_makes_it_longer_with_custom_time() {
    let project = Project::default();
    let (song, id) =
        ptz_core::edit::insert_preset_move(&Song::new("s", "Test Song", "01:00:00:00", 1500), &project, &presets()[1], 200)
            .unwrap();
    let mut r = rig(song);
    let before = r.song().motion_clip(&id).unwrap().clone();

    let l = r.layout();
    let y = l.lane_rect(Lane::Motion(0)).unwrap().center().y;
    let from = pos2(l.x_of(before.end_frame() as f64) - 2.0, y);
    let to = pos2(l.x_of(before.end_frame() as f64 + 150.0) - 2.0, y);
    r.drag(from, to);

    let after = r.song().motion_clip(&id).unwrap().clone();
    assert_eq!(after.start_frame, before.start_frame, "the start stays put");
    assert!(
        (after.duration_frames - (before.duration_frames + 150)).abs() <= 3,
        "{} -> {}",
        before.duration_frames,
        after.duration_frames
    );
    assert!(matches!(after.body, ClipBody::PresetMove { auto_duration: false, .. }), "stretching switches to custom time");
    assert_eq!(r.puts().len(), 1, "one edit reached the server, not one per mouse move");

    // Undo with the keyboard restores auto time.
    r.harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    r.harness.run_steps(2);
    assert_eq!(r.song().motion_clip(&id).unwrap(), &before);
    r.harness.key_press_modifiers(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, egui::Key::Z);
    r.harness.run_steps(2);
    assert_eq!(r.song().motion_clip(&id).unwrap(), &after);

    // A chord pressed and let go inside one frame still counts: no modifier is held when the frame ends.
    for pressed in [true, false] {
        r.harness.event(egui::Event::Key {
            key: egui::Key::Z,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        });
    }
    r.harness.run_steps(2);
    assert_eq!(r.song().motion_clip(&id).unwrap(), &before);
}

#[test]
fn squeezing_a_move_warns_and_the_inspector_button_clears_it() {
    use egui_kittest::kittest::Queryable as _;
    let project = Project::default();
    let (song, id) =
        ptz_core::edit::insert_preset_move(&Song::new("s", "Test Song", "01:00:00:00", 1500), &project, &presets()[1], 200)
            .unwrap();
    let mut r = rig(song);
    let before = r.song().motion_clip(&id).unwrap().clone();

    let l = r.layout();
    let y = l.lane_rect(Lane::Motion(0)).unwrap().center().y;
    let from = pos2(l.x_of(before.end_frame() as f64) - 2.0, y);
    let to = pos2(l.x_of(before.start_frame as f64 + 12.0) - 2.0, y);
    r.drag(from, to);
    let squeezed = r.song().motion_clip(&id).unwrap().clone();
    assert!(squeezed.duration_frames < 40, "{}", squeezed.duration_frames);
    assert!(!r.harness.state().session.issues_for(&id).is_empty(), "the move is flagged too fast");

    // The header counts it and the inspector offers the fix.
    r.harness.run_steps(2);
    r.harness.get_by_label("1 issue");
    r.harness.get_by_label("Use auto time").scroll_to_me();
    r.harness.run_steps(2);
    r.harness.get_by_label("Use auto time").click();
    r.harness.run_steps(3);
    let fixed = r.song().motion_clip(&id).unwrap().clone();
    let toasts: Vec<String> = r.harness.state().session.toasts.iter().map(|t| t.text.clone()).collect();
    assert_eq!(fixed.duration_frames, before.duration_frames, "{toasts:?}");
    assert!(r.harness.state().session.issues_for(&id).is_empty());
    assert!(r.harness.query_by_label("1 issue").is_none());
}

#[test]
fn clicking_the_ruler_seeks_and_space_asks_for_the_primary_action() {
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    let l = r.layout();
    let at = pos2(l.x_of(480.0), l.ruler.center().y);
    r.harness.hover_at(at);
    r.harness.run_steps(1);
    r.harness.drag_at(at);
    r.harness.run_steps(1);
    r.harness.drop_at(at);
    r.harness.run_steps(2);
    let seeks: Vec<i64> = r
        .cmds
        .try_iter()
        .filter_map(|c| match c {
            Cmd::Act(ptz_core::transport::Action::Seek, req) => req.frame,
            _ => None,
        })
        .collect();
    assert!(seeks.last().is_some_and(|f| (f - 480).abs() <= 2), "{seeks:?}");

    r.harness.key_press(egui::Key::Space);
    r.harness.run_steps(2);
    let acts: Vec<_> = r.cmds.try_iter().filter_map(|c| if let Cmd::Act(a, _) = c { Some(a) } else { None }).collect();
    assert_eq!(acts, vec![ptz_core::transport::Action::Play]);
}

#[test]
fn space_does_not_release_a_latched_e_stop() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    {
        let s = &mut r.harness.state_mut().session;
        s.status.transport.estop = true;
        s.status.guidance = ptz_core::transport::guidance(&s.status.transport, &ptz_core::transport::Context::ready());
        assert_eq!(s.status.guidance.primary, Some(ptz_core::transport::Action::ResetEstop));
    }
    r.harness.run_steps(2);
    let acts =
        |r: &Rig| -> Vec<_> { r.cmds.try_iter().filter_map(|c| if let Cmd::Act(a, _) = c { Some(a) } else { None }).collect() };

    r.harness.key_press(egui::Key::Space);
    r.harness.run_steps(2);
    assert_eq!(acts(&r), vec![], "the latch is released by its own button, never by the transport key");

    r.harness.get_by_label("Reset E-Stop").click();
    r.harness.run_steps(2);
    assert_eq!(acts(&r), vec![ptz_core::transport::Action::ResetEstop]);
}

fn click_media() -> ptz_core::api::MediaInfo {
    ptz_core::api::MediaInfo {
        id: "click".into(),
        name: "Click 12.4s.wav".into(),
        kind: ptz_core::api::MediaKind::Audio,
        channels: 1,
        sample_rate: 48_000,
        samples: 595_200,
        duration_s: 12.4,
        duration_frames: 310,
        start_tc: None,
        start_tc_source: None,
        proxy_frames: 0,
        ltc_channel: None,
        start_samples: None,
        proxy_rate: None,
    }
}

/// The overview the server would send for a click at 2.5 s.
fn click_level(samples_per_bucket: u32) -> ptz_core::api::PeakLevel {
    let buckets = 595_200usize.div_ceil(samples_per_bucket as usize);
    let mut pairs = vec![(0i8, 0i8); buckets];
    pairs[120_000 / samples_per_bucket as usize] = (-90, 114);
    ptz_core::api::PeakLevel { samples_per_bucket, sample_rate: 48_000, pairs }
}

impl Rig {
    /// Screen x of the loudest waveform column of a media clip, computed as the painter does.
    fn loudest_x(&self, clip: &str) -> f32 {
        let s = &self.harness.state().session;
        let l = self.layout();
        let m = self.song().media_clip(clip).unwrap();
        let rect = l.clips.iter().find(|c| c.id == clip).expect("the clip is laid out").rect;
        let level = s.peaks.get(&m.media_id).expect("the waveform arrived");
        let fps = s.project.rate.fps();
        let cols = crate::timeline_ui::waveform_columns(level, rect, rect, m.in_frame as f64 / fps, m.out_frame as f64 / fps);
        cols.iter().max_by(|a, b| (a.2 - a.1).total_cmp(&(b.2 - b.1))).expect("columns").0
    }
}

#[test]
fn dragging_audio_from_the_media_tab_places_a_clip_and_its_waveform_shows_the_click() {
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.events.send(Event::Media(vec![click_media()])).unwrap();
    r.harness.run_steps(2);
    r.harness.state_mut().show_tab(crate::app::Tab::Media);
    r.harness.run_steps(3);

    let card = r.harness.state().session.probe_media.iter().find(|(id, _)| id == "click").expect("the media card was drawn").1;
    let to = r.lane_point(Lane::Audio, 100.0);
    r.drag(card.center(), to);

    let song = r.song().clone();
    let toasts: Vec<String> = r.harness.state().session.toasts.iter().map(|t| t.text.clone()).collect();
    assert_eq!(song.media.len(), 1, "one audio clip was created; card {card:?} to {to:?}; {toasts:?}");
    let clip = song.media[0].clone();
    assert_eq!(clip.media_id, "click");
    assert!((clip.start_frame - 100).abs() <= 2, "it starts where it was dropped: {}", clip.start_frame);
    assert_eq!((clip.in_frame, clip.out_frame, clip.media_frames), (0, 310, 310));

    // The app asked for the waveform once, at the level its zoom needs.
    let cmds: Vec<Cmd> = r.cmds.try_iter().collect();
    let asked: Vec<u32> =
        cmds.iter().filter_map(|c| if let Cmd::FetchPeaks { spp, .. } = c { Some(*spp) } else { None }).collect();
    let ppf = r.harness.state().session.timeline.view.px_per_frame;
    assert_eq!(asked, vec![crate::timeline_ui::peak_level_for(48_000, 25.0, ppf)]);
    assert!(cmds.iter().any(|c| matches!(c, Cmd::PutWorking { song: put, .. } if *put == song)));

    // With the waveform in, the click is drawn at 2.5 s into the clip.
    r.events.send(Event::Peaks { media: "click".into(), level: click_level(asked[0]) }).unwrap();
    r.harness.run_steps(2);
    let click_frame = clip.start_frame as f64 + 62.5;
    let px_per_bucket = ppf * 25.0 * asked[0] as f32 / 48_000.0;
    let x = r.loudest_x(&clip.id);
    assert!(
        (x - r.layout().x_of(click_frame)).abs() <= px_per_bucket + 1.0,
        "click drawn at {x}, frame {click_frame} is at {}",
        r.layout().x_of(click_frame)
    );

    // Trimming the head hides the start of the file; the click stays under the same frame.
    let l = r.layout();
    let y = l.lane_rect(Lane::Audio).unwrap().center().y;
    let from = pos2(l.x_of(clip.start_frame as f64) + 2.0, y);
    let to = pos2(l.x_of(clip.start_frame as f64 + 30.0) + 2.0, y);
    r.drag(from, to);
    let trimmed = r.song().media[0].clone();
    assert!((trimmed.in_frame - 30).abs() <= 2, "{trimmed:?}");
    assert_eq!(trimmed.start_frame - clip.start_frame, trimmed.in_frame, "the sound did not slide");
    assert_eq!(trimmed.end_frame(), clip.end_frame());
    let x_after = r.loudest_x(&clip.id);
    assert!((x_after - x).abs() <= 1.0, "the click moved from {x} to {x_after}");

    // Dragging the body moves the clip and the click with it.
    let l = r.layout();
    let from = pos2(l.x_of(trimmed.start_frame as f64 + 100.0), y);
    let to = pos2(l.x_of(trimmed.start_frame as f64 + 300.0), y);
    r.drag(from, to);
    let moved = r.song().media[0].clone();
    assert!((moved.start_frame - trimmed.start_frame - 200).abs() <= 3, "{moved:?}");
    assert_eq!((moved.in_frame, moved.out_frame), (trimmed.in_frame, trimmed.out_frame));

    // Zooming in asks for a finer level, once.
    let before = r.harness.state().session.timeline.view.px_per_frame;
    r.harness.state_mut().session.timeline.view.px_per_frame = before * 16.0;
    r.harness.run_steps(3);
    let asked: Vec<u32> =
        r.cmds.try_iter().filter_map(|c| if let Cmd::FetchPeaks { spp, .. } = c { Some(spp) } else { None }).collect();
    assert_eq!(asked, vec![crate::timeline_ui::peak_level_for(48_000, 25.0, before * 16.0)]);
}

#[test]
fn the_audio_output_page_names_the_output_or_says_why_it_is_silent() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.settings(Page::Audio);
    r.harness.state_mut().session.status.audio.silent_reason =
        Some("There is no default audio output device. Running silent".into());
    r.harness.run_steps(2);
    r.harness.get_by_label("There is no default audio output device. Running silent");
    r.harness.state_mut().session.status.audio.output = Some("MacBook Pro Speakers".into());
    r.harness.state_mut().session.status.audio.device_rate = 48_000;
    r.harness.run_steps(2);
    r.harness.get_by_label("MacBook Pro Speakers, 48000 Hz");
}

// ---- stage 3: keys, graph editor, rig controller, record ----

use crate::app::Tab;
use crate::link::Why;
use ptz_core::api::{Jog, WorkingSong};
use ptz_core::pose::Axis;
use ptz_core::song::{Interp, Key};
use ptz_core::transport::{Action, Output};

/// A keys clip from frame 100: pan 0, 30, 10 degrees at 0, 100 and 200; zoom on a straight line.
fn keys_song() -> (Song, String) {
    let mut curves = std::collections::BTreeMap::new();
    curves.insert(
        Axis::Pan,
        vec![Key::new(0, 0.0, Interp::Bezier), Key::new(100, 30.0, Interp::Bezier), Key::new(200, 10.0, Interp::Bezier)],
    );
    curves.insert(Axis::Zoom, vec![Key::new(0, 0.2, Interp::Linear), Key::new(200, 0.6, Interp::Linear)]);
    let empty = Song::new("s", "Test Song", "01:00:00:00", 600);
    ptz_core::edit::punch_keys(&empty, &Project::default(), ptz_core::FIRST_CAMERA, 100, 200, curves, None).unwrap()
}

impl Rig {
    fn plot(&self) -> crate::graph::Plot {
        let s = &self.harness.state().session;
        crate::graph::Plot::new(s.graph.probe.expect("the graph editor was drawn"), &s.timeline.view, s.graph.ranges)
    }

    fn click(&mut self, at: Pos2) {
        self.harness.hover_at(at);
        self.harness.run_steps(2);
        self.harness.drag_at(at);
        self.harness.run_steps(1);
        self.harness.drop_at(at);
        self.harness.run_steps(2);
    }

    fn jogs(&self) -> Vec<Jog> {
        self.cmds.try_iter().filter_map(|c| if let Cmd::Jog(_, j) = c { Some(j) } else { None }).collect()
    }

    fn toasts(&self) -> Vec<String> {
        self.harness.state().session.toasts.iter().map(|t| t.text.clone()).collect()
    }
}

#[test]
fn a_key_dragged_in_the_graph_editor_changes_time_and_value_as_one_undo_step() {
    use egui_kittest::kittest::Queryable as _;
    let (song, id) = keys_song();
    let mut r = rig(song.clone());
    r.harness.state_mut().session.timeline.selection.insert(id.clone());
    r.harness.state_mut().show_tab(Tab::Graph);
    r.harness.run_steps(3);

    let plot = r.plot();
    let clip = song.motion_clip(&id).unwrap().clone();
    let ppf = r.harness.state().session.timeline.view.px_per_frame;
    let from = plot.key_pos(&clip, Axis::Pan, &clip.keys(Axis::Pan)[1]);
    let to = from + vec2(-30.0 * ppf, 40.0);
    r.drag(from, to);

    let moved = r.song().motion_clip(&id).unwrap().keys(Axis::Pan)[1];
    assert!((moved.frame - 70).abs() <= 2, "the key went 30 frames earlier: {}", moved.frame);
    let (lo, hi) = plot.ranges[Axis::Pan.index()];
    let want = 30.0 - 40.0 * (hi - lo) / (plot.body.height() as f64 - 20.0);
    assert!((moved.value - want).abs() < 0.6, "the key went down to about {want}: {}", moved.value);
    assert_eq!(r.song().motion_clip(&id).unwrap().keys(Axis::Zoom), clip.keys(Axis::Zoom));
    assert_eq!(r.puts().len(), 1, "one edit reached the server, not one per mouse move");
    assert_eq!(r.harness.state().session.undo.len(), 1);
    assert!(r.harness.state().session.timeline.key_selection.contains(&(id.clone(), Axis::Pan, 1)));

    // The key is still selected, so the ease buttons work on it.
    r.harness.get_by_label("Easy Ease").click();
    r.harness.run_steps(3);
    let eased = r.song().motion_clip(&id).unwrap().keys(Axis::Pan)[1];
    assert_eq!(eased.in_handle[1], 0.0);
    assert_eq!(eased.out_handle[1], 0.0);
    assert!((eased.in_handle[0] + 0.42 * moved.frame as f64).abs() < 1e-9, "{:?}", eased.in_handle);
    assert!((eased.out_handle[0] - 0.42 * (200 - moved.frame) as f64).abs() < 1e-9, "{:?}", eased.out_handle);

    r.harness.get_by_label("Hold").click();
    r.harness.run_steps(3);
    assert_eq!(r.song().motion_clip(&id).unwrap().keys(Axis::Pan)[1].interp, Interp::Hold);

    // Undo three times is back to the song as it was loaded.
    for _ in 0..3 {
        r.harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
        r.harness.run_steps(2);
    }
    assert_eq!(r.song(), &song);

    // Speed view: the toggle is a real button and the editor stops taking key drags.
    r.harness.get_by_label("Speed").click();
    r.harness.run_steps(3);
    assert!(r.harness.state().session.graph.speed);
    r.drag(from, to);
    assert_eq!(r.song(), &song, "nothing is editable in the speed view");
}

#[test]
fn the_graph_editor_shares_the_timeline_zoom_and_frame_fits_the_clip() {
    use egui_kittest::kittest::Queryable as _;
    let (song, id) = keys_song();
    let mut r = rig(song.clone());
    r.harness.state_mut().session.timeline.selection.insert(id.clone());
    let timeline_x = r.layout().x_of(200.0);
    r.harness.state_mut().show_tab(Tab::Graph);
    r.harness.run_steps(3);
    assert_eq!(r.plot().x_of(200.0), timeline_x, "frame 200 is at the same x in both panels");

    r.harness.get_by_label("Frame").click();
    r.harness.run_steps(3);
    let plot = r.plot();
    let (x0, x1) = (plot.x_of(100.0), plot.x_of(300.0));
    assert!(x0 > plot.body.left() && x0 < plot.body.left() + plot.body.width() * 0.15, "the clip starts near the left: {x0}");
    assert!(x1 < plot.body.right() && x1 > plot.body.right() - plot.body.width() * 0.15, "and ends near the right: {x1}");
    r.harness.state_mut().show_tab(Tab::Timeline);
    r.harness.run_steps(3);
    assert!((r.layout().x_of(100.0) - x0).abs() < 0.5, "the timeline zoomed with it");
}

#[test]
fn the_rig_pad_sends_velocity_while_held_and_stops_on_release() {
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.harness.state_mut().show_tab(Tab::Rig);
    r.harness.run_steps(3);
    let surface = r.harness.state().session.rig.probe.expect("the controller was drawn");
    let speed = r.harness.state().session.rig.speed;
    let c = surface.pad.center();
    let right = pos2(surface.pad.right() - 8.0, c.y);

    r.harness.hover_at(right);
    r.harness.run_steps(2);
    r.harness.drag_at(right);
    r.harness.run_steps(4);
    let held = r.jogs();
    assert!(!held.is_empty(), "pressing the pad moves the rig without waiting for a drag");
    assert!(held.iter().all(|j| (j.pan - speed).abs() < 1e-9 && j.tilt == 0.0 && j.zoom == 0.0), "{held:?}");

    // Sliding up adds tilt; the message repeats while held.
    let up_right = pos2(surface.pad.right() - 8.0, surface.pad.top() + 8.0);
    r.harness.hover_at(up_right);
    for _ in 0..3 {
        std::thread::sleep(crate::rig_ui::SEND_EVERY + std::time::Duration::from_millis(5));
        r.harness.run_steps(1);
    }
    let held = r.jogs();
    assert!(held.len() >= 2, "{held:?}");
    let last = held.last().unwrap();
    assert!((last.pan - speed).abs() < 1e-9 && (last.tilt - speed).abs() < 1e-9, "{last:?}");

    r.harness.drop_at(up_right);
    r.harness.run_steps(3);
    assert_eq!(r.jogs(), vec![Jog::default()], "release sends one stop");
    r.harness.run_steps(3);
    assert!(r.jogs().is_empty());

    // The zoom slider drives zoom only, and springs back.
    let (_, track) = surface.sliders[0];
    let grab = pos2(track.left() + 2.0, track.center().y);
    r.harness.hover_at(grab);
    r.harness.run_steps(2);
    r.harness.drag_at(grab);
    r.harness.run_steps(3);
    let zoom = r.jogs();
    assert!(zoom.iter().all(|j| (j.zoom + speed).abs() < 1e-9 && j.pan == 0.0 && j.elevation == 0.0), "{zoom:?}");
    r.harness.drop_at(grab);
    r.harness.run_steps(3);
    assert_eq!(r.jogs(), vec![Jog::default()]);

    // With the controller focused the arrow keys drive the rig and do not move the playhead.
    r.harness.key_down(egui::Key::ArrowLeft);
    r.harness.run_steps(3);
    let cmds: Vec<Cmd> = r.cmds.try_iter().collect();
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Jog(_, j) if (j.pan + speed).abs() < 1e-9)), "{}", cmds.len());
    assert!(!cmds.iter().any(|c| matches!(c, Cmd::Act(Action::Seek, _))), "the arrow did not scrub");
    r.harness.key_up(egui::Key::ArrowLeft);
    r.harness.run_steps(3);
    assert_eq!(r.jogs().last(), Some(&Jog::default()));

    // Armed: the controller says why and sends nothing.
    r.harness.state_mut().session.status.transport.output = Output::Armed;
    r.harness.run_steps(2);
    r.harness.hover_at(right);
    r.harness.run_steps(2);
    r.harness.drag_at(right);
    r.harness.run_steps(3);
    r.harness.drop_at(right);
    r.harness.run_steps(2);
    assert!(r.jogs().is_empty());
    assert!(r.toasts().iter().any(|t| t.contains("Disarm to drive the rig by hand")), "{:?}", r.toasts());
}

#[test]
fn record_dots_on_the_axis_lanes_choose_what_record_writes_and_k_keys_the_selected_lanes() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.harness.state_mut().session.timeline.view.set_open(0, true);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.record_axes, vec![Axis::Pan, Axis::Tilt]);

    let dot = |r: &Rig, axis: Axis| crate::timeline_ui::record_dot(&r.layout(), 0, axis).expect("the axis lane is open");
    let at = dot(&r, Axis::Zoom);
    r.click(at);
    let at = dot(&r, Axis::Pan);
    r.click(at);
    assert_eq!(r.harness.state().session.record_axes, vec![Axis::Tilt, Axis::Zoom]);

    r.harness.get_by_label("Record").click();
    r.harness.run_steps(2);
    let sent: Vec<_> =
        r.cmds.try_iter().filter_map(|c| if let Cmd::Act(Action::RecordOn, req) = c { Some(req.axes) } else { None }).collect();
    assert_eq!(sent, vec![Some(vec![Axis::Tilt, Axis::Zoom])]);

    // While Record is on the server's axes are shown and cannot be changed.
    {
        let s = &mut r.harness.state_mut().session;
        s.status.transport.record = true;
        s.status.record.axes = vec![Axis::Tilt, Axis::Zoom];
    }
    r.harness.run_steps(2);
    let at = dot(&r, Axis::Focus);
    r.click(at);
    assert_eq!(r.harness.state().session.record_axes, vec![Axis::Tilt, Axis::Zoom]);
    assert!(r.toasts().iter().any(|t| t.contains("Turn Record off")), "{:?}", r.toasts());
    r.harness.state_mut().session.status.transport.record = false;

    // With no axis enabled Record explains itself instead of asking the server.
    r.harness.state_mut().session.record_axes.clear();
    r.harness.run_steps(2);
    r.harness.get_by_label("Record").click();
    r.harness.run_steps(2);
    assert!(!r.cmds.try_iter().any(|c| matches!(c, Cmd::Act(Action::RecordOn, _))));
    assert!(r.toasts().iter().any(|t| t.contains("Record-enable an axis first")), "{:?}", r.toasts());

    // Clicking a lane's name selects it for K; K keys only that lane.
    let l = r.layout();
    let name = pos2(l.rect.left() + 60.0, l.lane_rect(Lane::Axis(0, Axis::Tilt)).unwrap().center().y);
    r.click(name);
    assert_eq!(r.harness.state().session.axis_lanes.iter().copied().collect::<Vec<_>>(), vec![Axis::Tilt]);
    r.harness.key_press(egui::Key::K);
    r.harness.run_steps(3);
    let song = r.song().clone();
    assert_eq!(song.motion.len(), 1, "{:?}", r.toasts());
    assert_eq!(song.motion[0].axes, vec![Axis::Tilt]);
    assert_eq!(song.motion[0].keys(Axis::Tilt).len(), 1);
}

#[test]
fn a_recorded_pass_arriving_from_the_server_is_one_undo_step_and_opens_in_the_graph_editor() {
    let before = Song::new("s", "Test Song", "01:00:00:00", 600);
    let mut r = rig(before.clone());
    // What the server sends after a pass: the song with a recorded keys clip punched in.
    let mut curves = std::collections::BTreeMap::new();
    curves.insert(Axis::Pan, vec![Key::new(0, 0.0, Interp::Bezier), Key::new(120, 40.0, Interp::Bezier)]);
    let (recorded, clip) = ptz_core::edit::punch_keys(
        &before,
        &Project::default(),
        ptz_core::FIRST_CAMERA,
        0,
        120,
        curves,
        Some("pass-0001".into()),
    )
    .unwrap();
    let working = WorkingSong { song: recorded.clone(), edit_counter: 1, dirty: true, revision: None, issues: Vec::new() };
    r.events.send(Event::Working(Box::new(working), Why::Reload)).unwrap();
    r.harness.run_steps(3);
    assert_eq!(r.song(), &recorded);
    assert_eq!(r.harness.state().session.undo.len(), 1);
    assert!(r.puts().is_empty(), "the server's own change is not sent back to it");

    // The playhead is inside the pass, so the graph editor shows it without a click.
    r.harness.state_mut().show_tab(Tab::Graph);
    r.harness.run_steps(3);
    let s = &r.harness.state().session;
    let shown = crate::graph::target_clip(r.song(), s.cam_id(), &s.timeline.selection, &s.timeline.key_selection, s.playhead());
    assert_eq!(shown.map(|c| c.id.clone()), Some(clip));

    // One undo restores everything the pass replaced.
    r.harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    r.harness.run_steps(2);
    assert_eq!(r.song(), &before);
    assert_eq!(r.puts(), vec![before]);
}

fn stamped_media(id: &str, kind: ptz_core::api::MediaKind, start_tc: &str) -> ptz_core::api::MediaInfo {
    ptz_core::api::MediaInfo {
        id: id.into(),
        name: format!("{id}.wav"),
        kind,
        duration_s: 8.0,
        duration_frames: 200,
        start_tc: Some(start_tc.into()),
        start_tc_source: Some("bext".into()),
        proxy_frames: if kind == ptz_core::api::MediaKind::Video { 200 } else { 0 },
        proxy_rate: Some(ptz_core::FrameRate::F25),
        ..click_media()
    }
}

impl Rig {
    fn media_card(&mut self, id: &str) -> Pos2 {
        self.harness.state_mut().show_tab(Tab::Media);
        self.harness.run_steps(3);
        let s = &self.harness.state().session;
        s.probe_media.iter().find(|(m, _)| m == id).expect("the media card was drawn").1.center()
    }

    fn press(&mut self, label: &str) {
        use egui_kittest::kittest::Queryable as _;
        // Scrolling into view is animated: wait for the button to come to rest before clicking it.
        self.harness.get_by_label(label).scroll_to_me();
        let mut at = self.harness.get_by_label(label).rect();
        for _ in 0..40 {
            self.harness.run_steps(1);
            let now = self.harness.get_by_label(label).rect();
            if now == at {
                break;
            }
            at = now;
        }
        self.harness.run_steps(2);
        self.harness.get_by_label(label).click();
        self.harness.run_steps(3);
    }
}

#[test]
fn a_stamped_file_lands_on_its_code_wherever_it_is_dropped_and_the_inspector_locks_to_it() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.events.send(Event::Media(vec![stamped_media("bed", ptz_core::api::MediaKind::Audio, "01:00:14:00")])).unwrap();
    r.harness.run_steps(2);

    // Dropped 24 seconds in, it goes to 14 seconds: where its timecode says it starts.
    let card = r.media_card("bed");
    let to = r.lane_point(Lane::Audio, 600.0);
    r.drag(card, to);
    let song = r.song().clone();
    assert_eq!(song.media.len(), 1, "{:?}", r.toasts());
    let clip = song.media[0].clone();
    assert_eq!(clip.start_frame, 350);
    assert_eq!(song.clock(ptz_core::FrameRate::F25).timecode_at(clip.start_frame).to_string(), "01:00:14:00");

    // It is selected, and the inspector says where it stands and offers the lock.
    r.harness.state_mut().show_tab(Tab::Inspector);
    r.harness.run_steps(3);
    r.harness.get_by_label("On its timecode");
    r.harness.get_by_label("from the broadcast WAV");
    let _ = r.cmds.try_iter().count();
    r.press("Lock to this file");
    let locks: Vec<Option<String>> =
        r.cmds.try_iter().filter_map(|c| if let Cmd::Act(Action::Lock, req) = c { Some(req.source) } else { None }).collect();
    assert_eq!(locks, vec![Some(format!("file:{}", clip.id))]);

    // Dragged four seconds late, the inspector says so and puts it back.
    let l = r.layout();
    let y = l.lane_rect(Lane::Audio).unwrap().center().y;
    r.drag(pos2(l.x_of(400.0), y), pos2(l.x_of(500.0), y));
    let moved = r.song().media[0].start_frame;
    assert!((moved - 450).abs() <= 3, "{moved}");
    r.harness.run_steps(2);
    r.harness.get_by_label(format!("Sits {} frames late against its timecode", moved - 350).as_str());
    r.press("Move to its timecode");
    assert_eq!(r.song().media[0].start_frame, 350, "{:?}", r.toasts());

    // Alt places it by hand: a second copy goes where the pointer is.
    let card = r.media_card("bed");
    let to = r.lane_point(Lane::Audio, 900.0);
    r.harness.event(egui::Event::ModifiersChanged(egui::Modifiers::ALT));
    r.drag(card, to);
    r.harness.event(egui::Event::ModifiersChanged(egui::Modifiers::default()));
    r.harness.run_steps(1);
    let starts: Vec<i64> = r.song().media.iter().map(|c| c.start_frame).collect();
    assert_eq!(starts.len(), 2, "{:?}", r.toasts());
    assert!((starts[1] - 900).abs() <= 3, "{starts:?}");
}

#[test]
fn a_stamped_file_outside_the_song_offers_to_move_the_song_start_to_it() {
    use egui_kittest::kittest::Queryable as _;
    let project = Project::default();
    let (before, _) =
        ptz_core::edit::insert_preset_move(&Song::new("s", "Test Song", "01:00:00:00", 1500), &project, &presets()[1], 200)
            .unwrap();
    let mut r = rig(before.clone());
    r.events.send(Event::Media(vec![stamped_media("bed", ptz_core::api::MediaKind::Audio, "02:00:10:00")])).unwrap();
    r.harness.run_steps(2);

    let card = r.media_card("bed");
    let to = r.lane_point(Lane::Audio, 600.0);
    r.drag(card, to);
    assert!(r.song().media.is_empty(), "nothing is placed until the operator chooses");
    r.harness.get_by_label("bed.wav starts at 02:00:10:00");
    r.harness.get_by_label("That is outside Test Song, which runs from 01:00:00:00 to 01:01:00:00.");

    r.press("Set the song start to 02:00:10:00");
    let song = r.song().clone();
    assert_eq!(song.start_tc, "02:00:10:00");
    assert_eq!(song.media.iter().map(|c| c.start_frame).collect::<Vec<_>>(), vec![0]);
    assert_eq!(song.motion[0].start_frame, 200, "moves keep their places");
    assert!(r.harness.query_by_label("bed.wav starts at 02:00:10:00").is_none(), "the dialog closed");
    assert_eq!(r.harness.state().session.undo.len(), 1, "the new start and the clip are one undo step");
    r.harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::Z);
    r.harness.run_steps(2);
    assert_eq!(r.song(), &before);

    // The other answer: leave the song alone and place it where it was dropped.
    let card = r.media_card("bed");
    let to = r.lane_point(Lane::Audio, 600.0);
    r.drag(card, to);
    r.press("Place it where I dropped it");
    let song = r.song().clone();
    assert_eq!(song.start_tc, "01:00:00:00");
    assert!((song.media[0].start_frame - 600).abs() <= 3, "{}", song.media[0].start_frame);
}

#[test]
fn the_reference_monitor_shows_the_frame_under_the_playhead_and_the_video_lane_a_filmstrip() {
    use crate::link::FrameKey;
    let project = Project::default();
    let info = stamped_media("plate", ptz_core::api::MediaKind::Video, "01:00:04:00");
    let (song, clip) = ptz_core::edit::add_media_clip(
        &Song::new("s", "Test Song", "01:00:00:00", 600),
        &project,
        &info.id,
        &info.name,
        ptz_core::song::Track::Video,
        info.duration_frames,
        100,
    )
    .unwrap();
    let mut r = rig(song);
    r.events.send(Event::Media(vec![info])).unwrap();
    r.harness.state_mut().show_tab(Tab::Reference);
    r.harness.run_steps(3);

    // The filmstrip asks for small pictures across the clip, at frames of the file.
    let asked = r.harness.state().session.frames.asked();
    let thumbs: Vec<i64> = asked.iter().filter(|k| k.thumb).map(|k| k.frame).collect();
    assert!(thumbs.len() >= 3 && thumbs[0] == 0, "{thumbs:?}");
    assert!(thumbs.iter().all(|f| (0..200).contains(f)), "{thumbs:?}");
    // The playhead is before the clip: no monitor picture is asked for.
    assert!(asked.iter().all(|k| k.thumb), "{asked:?}");

    // Playhead 30 frames into the clip: the monitor asks for frame 30 of the file.
    {
        let status = &mut r.harness.state_mut().session.status;
        status.frame = 130;
        status.frame_f = 130.0;
    }
    r.harness.run_steps(3);
    let full = FrameKey { media: "plate".into(), frame: 30, thumb: false };
    assert!(r.harness.state().session.frames.asked().contains(&full));
    assert!(r.harness.state().session.reference_shown.is_none());

    // When it arrives it is shown, and a filmstrip picture becomes a texture.
    let picture = |shade: u8| Some(egui::ColorImage::filled([48, 27], egui::Color32::from_gray(shade)));
    r.events.send(Event::Frame { key: full.clone(), image: picture(200) }).unwrap();
    let first_thumb = FrameKey { media: "plate".into(), frame: 0, thumb: true };
    r.events.send(Event::Frame { key: first_thumb.clone(), image: picture(90) }).unwrap();
    r.harness.run_steps(3);
    let s = &r.harness.state().session;
    assert_eq!(s.reference_shown.as_ref().map(|(media, _)| media.as_str()), Some("plate"));
    assert!(s.frames.ready(&first_thumb).is_some());
    assert!(!s.frames.asked().contains(&full));

    // Trimming the head of the clip does not change which pictures are needed.
    let l = r.layout();
    let rect = l.clip(&clip).unwrap().rect;
    let strip = crate::timeline_ui::filmstrip_rect(rect);
    let m = r.song().media_clip(&clip).unwrap().clone();
    let tiles = crate::timeline_ui::filmstrip_tiles(strip, rect, &m, l.px_per_frame);
    let mut trimmed = m.clone();
    trimmed.in_frame = 40;
    trimmed.start_frame += 40;
    let later = egui::Rect::from_min_max(pos2(strip.left() + 40.0 * l.px_per_frame, strip.top()), strip.max);
    let after = crate::timeline_ui::filmstrip_tiles(later, later, &trimmed, l.px_per_frame);
    assert!(after.iter().all(|t| tiles.iter().any(|b| b.frame == t.frame && (b.rect.left() - t.rect.left()).abs() < 0.01)));
    assert!(tiles.windows(2).all(|w| w[1].frame >= w[0].frame && (w[1].rect.left() - w[0].rect.right()).abs() < 0.01));
}

#[test]
fn arm_says_that_it_moves_the_rig_and_a_fault_is_shown_until_it_is_cleared() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.harness.get_by_label("Arm (moves rig to playhead)").click();
    r.harness.run_steps(2);
    assert!(r.cmds.try_iter().any(|c| matches!(c, Cmd::Act(Action::Arm, _))));

    // Positioning, then a tracking fault: output is cut and the reason stays on show.
    {
        let s = &mut r.harness.state_mut().session;
        s.status.transport.output = Output::Positioning;
        s.status.hardware_live = true;
        s.status.telemetry.hardware = true;
        s.status.cameras[0].hardware = true;
        s.status.cameras[0].error.pan = 1.4;
    }
    r.harness.state_mut().show_tab(Tab::Rig);
    r.harness.run_steps(3);
    r.harness.get_by_label("ARMING");
    r.harness.get_by_label("Disarm");
    r.harness.get_by_label("TRACKING");
    r.harness.get_by_label("+1.40");
    {
        let s = &mut r.harness.state_mut().session;
        s.status.transport.output = Output::Rehearse;
        s.status.hardware_live = false;
        s.status.telemetry.fault = Some("Tracking fault: pan is 3.2° from the timeline. The rig is held".into());
        s.status.telemetry.halting = true;
    }
    r.harness.run_steps(3);
    assert_eq!(r.harness.query_all_by_label("Tracking fault: pan is 3.2° from the timeline. The rig is held").count(), 2);
    r.harness.get_by_label("Stopping: stop is being repeated until the rig stands still");
    assert!(r.harness.query_by_label("TRACKING").is_none(), "the error bars are for live output only");
}

#[test]
fn syncing_presets_from_a_real_rig_asks_first_and_is_refused_while_live() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    let synced = |r: &Rig| r.cmds.try_iter().filter(|c| matches!(c, Cmd::SyncPresets(_))).count();

    // The simulator has nothing to move: one click syncs.
    r.harness.get_by_label("Sync from rig").click();
    r.harness.run_steps(2);
    assert_eq!(synced(&r), 1);

    // A real camera travels to every preset, so the click only asks.
    r.harness.state_mut().session.status.cameras[0].hardware = true;
    r.harness.run_steps(2);
    r.harness.get_by_label("Sync from rig").click();
    r.harness.run_steps(3);
    assert_eq!(synced(&r), 0);
    r.harness.get_by_label("Read presets from the camera?");
    r.harness.get_by_label("Cancel").click();
    r.harness.run_steps(3);
    assert_eq!(synced(&r), 0);
    assert!(r.harness.query_by_label("Read presets from the camera?").is_none());

    r.harness.get_by_label("Sync from rig").click();
    r.harness.run_steps(3);
    r.harness.get_by_label("Move the camera and sync").click();
    r.harness.run_steps(3);
    assert_eq!(synced(&r), 1);
    assert!(r.harness.query_by_label("Read presets from the camera?").is_none());

    // The server's answer on hardware is only that it began; progress comes with the status.
    r.events.send(Event::SyncAccepted(presets())).unwrap();
    r.harness.state_mut().session.status.cameras[0].preset_sync = Some("Recalling preset 2 of 3: Lectern".into());
    r.harness.run_steps(3);
    assert!(r.toasts().iter().any(|t| t.contains("It moves to each one in turn")), "{:?}", r.toasts());
    r.harness.get_by_label("Recalling preset 2 of 3: Lectern");
    r.harness.get_by_label("Sync from rig").click();
    r.harness.run_steps(3);
    assert_eq!(synced(&r), 0, "the button is off while a sync runs");
    assert!(r.harness.query_by_label("Read presets from the camera?").is_none());

    // Armed, the button is off: the camera is not free to travel.
    {
        let s = &mut r.harness.state_mut().session;
        s.status.cameras[0].preset_sync = None;
        s.status.transport.output = Output::Armed;
    }
    r.harness.run_steps(3);
    r.harness.get_by_label("Sync from rig").click();
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label("Read presets from the camera?").is_none());

    // A preset with a picture on the camera asks for it once, and shows it when it arrives.
    {
        let s = &mut r.harness.state_mut().session;
        s.status.transport.output = Output::Rehearse;
        s.presets[1].has_thumbnail = true;
    }
    r.harness.run_steps(4);
    let asked: Vec<String> =
        r.cmds.try_iter().filter_map(|c| if let Cmd::FetchPresetThumb(id) = c { Some(id) } else { None }).collect();
    assert_eq!(asked, ["lectern"]);
    let image = egui::ColorImage::filled([16, 9], egui::Color32::from_rgb(200, 40, 40));
    r.events.send(Event::PresetThumb { id: "lectern".into(), image }).unwrap();
    r.harness.run_steps(3);
    let pictures = &r.harness.state().session.preset_pictures;
    assert!(matches!(pictures.get("lectern"), Some(crate::session::PresetPicture::Shown(_))));
}

#[test]
fn a_keep_out_rule_is_added_and_edited_in_the_project_settings_and_blocks_the_song_at_once() {
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    // A move that ends low and tilted down: fine until a rule says otherwise.
    {
        let s = &mut r.harness.state_mut().session;
        let low = Preset {
            id: "low".into(),
            name: "Low".into(),
            pose: Pose { tilt: -20.0, elevation: 50.0, ..Pose::home() },
            camera: ptz_core::camera::first_camera(),
            camera_number: None,
            has_thumbnail: false,
        };
        let (song, _) = ptz_core::edit::insert_preset_move(s.song.as_ref().unwrap(), &s.project, &low, 100).unwrap();
        s.commit_edit(song);
    }
    r.harness.run_steps(3);
    assert_eq!(r.blocking(), 0);
    let projects = |r: &Rig| -> Vec<Project> {
        r.cmds.try_iter().filter_map(|c| if let Cmd::PutProject(p) = c { Some(*p) } else { None }).collect()
    };
    let _ = projects(&r);

    r.settings(Page::KeepOut);
    r.press("Add a keep-out rule");
    let sent = projects(&r);
    let rule = &sent.last().expect("the project was sent").cameras[0].keep_out[0];
    assert_eq!(rule.describe(), "when elevation is below 200 mm, tilt must be above -10.0°");
    assert_eq!(rule.name, "Rule 1");
    // The local compile already shows it: the move now breaks the rule, and that cannot be accepted.
    let s = &r.harness.state().session;
    assert_eq!(r.blocking(), 1);
    assert!(s.compiled.issues.iter().any(|i| i.message().contains("Keep-out rule broken") && !i.acceptable()));

    r.press("Remove");
    let sent = projects(&r);
    assert!(sent.last().unwrap().cameras[0].keep_out.is_empty());
    assert_eq!(r.blocking(), 0);
}

#[test]
fn rig_settings_switch_to_hardware_and_take_a_height_from_the_leg_box() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    let projects = |r: &Rig| -> Vec<Project> {
        r.cmds.try_iter().filter_map(|c| if let Cmd::PutProject(p) = c { Some(*p) } else { None }).collect()
    };
    r.settings(Page::Rig);
    assert!(r.harness.query_by_label("FR7 address").is_none(), "the simulator has no addresses to set");
    {
        let s = &mut r.harness.state_mut().session;
        s.project.cameras[0].rig.kind = ptz_core::pose::RigKind::Hardware;
        s.project.cameras[0].rig.autopod_host = "10.0.0.31".into();
        s.status.cameras[0].adapters = vec![ptz_core::api::AdapterStatus {
            name: "AutoPod+".into(),
            connected: true,
            detail: "10.0.0.31:80 answers in 3.0 ms".into(),
            calibrated: Some(true),
            round_trip_ms: Some(3.0),
            raw_position: Some(9120.0),
        }];
    }
    r.harness.run_steps(3);
    r.harness.get_by_label("FR7 address");
    r.harness.get_by_label("assumed until bring-up measures them");
    r.harness.get_by_label("0 of 3 heights. Arm is refused until the map has three; ptz-bringup measures them with you.");
    r.press("Add the current height");
    let sent = projects(&r);
    let map = &sent.last().expect("the project was sent").cameras[0].rig.autopod_calibration;
    assert_eq!(map.len(), 1);
    assert_eq!(map[0].raw, 9120.0);
    r.harness.get_by_label("1 of 3 heights. Arm is refused until the map has three; ptz-bringup measures them with you.");

    // Armed or playing, the rig cannot be swapped out from under the show.
    r.harness.state_mut().session.status.transport.output = Output::Armed;
    r.harness.run_steps(3);
    r.harness.get_by_label("Stop and disarm to change which rig the server drives.");
    r.press("Add the current height");
    assert!(projects(&r).is_empty());
}

impl Rig {
    /// Open Show settings at a page.
    pub fn settings(&mut self, page: Page) {
        self.harness.state_mut().session.settings = Some(page);
        self.harness.run_steps(3);
    }

    /// Close Show settings: back to the timeline and monitors.
    pub fn panels(&mut self) {
        self.harness.state_mut().session.settings = None;
        self.harness.run_steps(3);
    }

    fn blocking(&self) -> usize {
        let s = &self.harness.state().session;
        s.compiled.blocking_count(s.song.as_ref().unwrap())
    }
}

#[test]
fn the_camera_bar_and_the_number_keys_select_a_camera_and_presets_store_sync_and_record_follow() {
    use egui_kittest::kittest::Queryable as _;
    let (project, presets) = three_cameras();
    let mut r = rig_with(project, presets, Song::new("s", "Test Song", "01:00:00:00", 1500));
    let shown = |r: &Rig| -> Vec<String> { r.harness.state().session.probe_presets.iter().map(|p| p.0.clone()).collect() };
    assert_eq!(r.harness.state().session.cam_id(), "cam1");
    assert_eq!(shown(&r), vec!["wide", "lectern", "awards"]);

    r.press("2 Centre");
    assert_eq!(r.harness.state().session.cam_id(), "cam2");
    assert_eq!(shown(&r), vec!["drums"], "only the selected camera's presets are offered");

    r.harness.key_press(egui::Key::Num3);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.cam_id(), "cam3");
    r.harness.get_by_label("No presets yet for Balcony.");
    r.harness.key_press(egui::Key::Num7);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.cam_id(), "cam3", "a key past the last camera does nothing");

    // Store, sync and Record all act on the selected camera.
    let _ = r.cmds.try_iter().count();
    r.press("Store current pose");
    r.press("Sync from rig");
    r.press("Record");
    let cmds: Vec<Cmd> = r.cmds.try_iter().collect();
    assert!(cmds.iter().any(|c| matches!(c, Cmd::StorePreset(p) if p.camera.as_deref() == Some("cam3") && p.name == "Preset 1")));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::SyncPresets(camera) if camera == "cam3")));
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Act(Action::RecordOn, req) if req.camera.as_deref() == Some("cam3"))));

    // The monitor follows too: the selected camera's pose at the playhead, not the first camera's.
    {
        let s = &mut r.harness.state_mut().session;
        let drums = s.presets.iter().find(|p| p.id == "drums").unwrap().clone();
        let (song, _) = ptz_core::edit::insert_preset_move(s.song.as_ref().unwrap(), &s.project, &drums, 0).unwrap();
        s.commit_edit(song);
        s.seek(1400);
    }
    r.harness.run_steps(3);
    let s = &r.harness.state().session;
    assert_eq!(s.monitor_pose().0, Pose::home(), "the balcony camera has no moves");
    assert_eq!(s.camera_pose("cam2").0.pan, 12.0);

    // A camera's state is on its chip: a held camera says so, and why is in its hover text.
    {
        let s = &mut r.harness.state_mut().session;
        s.status.cameras[1].fault = Some("FR7: no answer for 600 ms".into());
        s.status.cameras[1].held = true;
    }
    r.harness.run_steps(3);
    r.harness.get_by_label("held");
}

#[test]
fn each_camera_has_its_own_lane_and_a_move_dragged_to_another_lane_changes_camera() {
    let (project, presets) = three_cameras();
    let mut r = rig_with(project, presets, Song::new("s", "Test Song", "01:00:00:00", 1500));
    let l = r.layout();
    assert_eq!(l.lanes.len(), 6, "markers, video, audio and one lane per camera");
    assert!(l.lane_rect(Lane::Missing).is_none());

    // A preset lands on its own camera's lane wherever it is let go.
    r.press("2 Centre");
    let from = r.preset_point("drums");
    let to = r.lane_point(Lane::Motion(0), 300.0);
    r.drag(from, to);
    let clip = r.song().motion[0].clone();
    assert_eq!(clip.camera, "cam2");
    assert_eq!(r.layout().clip(&clip.id).unwrap().lane, Lane::Motion(1));

    // Dragged down a lane it becomes the third camera's move, at the time it was dropped.
    let from = r.layout().clip(&clip.id).unwrap().rect.center();
    let to = pos2(from.x + r.layout().px_per_frame * 100.0, r.layout().lane_rect(Lane::Motion(2)).unwrap().center().y);
    r.drag(from, to);
    let moved = r.song().motion_clip(&clip.id).unwrap().clone();
    assert_eq!(moved.camera, "cam3");
    assert!((moved.start_frame - clip.start_frame - 100).abs() <= 2, "{}", moved.start_frame);
    assert_eq!(r.harness.state().session.undo.len(), 2, "one undo step for the drop and one for the drag");
    assert_eq!(r.harness.state().session.cam_id(), "cam2", "touching the move selected the camera it belonged to");

    // Twirling a camera open shows that camera's five axis lanes only.
    let twirl = r.layout().twirl(2).unwrap().center();
    r.click(twirl);
    let l = r.layout();
    assert_eq!(l.lanes.len(), 11);
    assert!(l.lane_rect(Lane::Axis(2, Axis::Pan)).is_some() && l.lane_rect(Lane::Axis(0, Axis::Pan)).is_none());

    // Clicking a camera's lane header selects it.
    let header = pos2(l.rect.left() + 70.0, l.lane_rect(Lane::Motion(0)).unwrap().center().y);
    r.click(header);
    assert_eq!(r.harness.state().session.cam_id(), "cam1");

    // A move whose camera has left the project is kept on a lane of its own and blocks Arm.
    {
        let s = &mut r.harness.state_mut().session;
        let mut project = s.project.clone();
        project.cameras.remove(2);
        s.put_project(project);
    }
    r.harness.run_steps(3);
    let l = r.layout();
    assert_eq!(l.clip(&clip.id).unwrap().lane, Lane::Missing);
    assert_eq!(r.blocking(), 1);
}

#[test]
fn cameras_are_added_named_and_removed_in_show_settings_up_to_twelve() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    let projects = |r: &Rig| -> Vec<Project> {
        r.cmds.try_iter().filter_map(|c| if let Cmd::PutProject(p) = c { Some(*p) } else { None }).collect()
    };
    r.settings(Page::Cameras);
    r.harness.get_by_label("1 of 12 cameras. Keys 1 to 9, 0, - and = select them.");

    r.press("Add a camera");
    let sent = projects(&r).pop().expect("the project was sent");
    assert_eq!(sent.cameras.len(), 2);
    let (first, second) = (&sent.cameras[0], &sent.cameras[1]);
    assert_eq!((second.id.as_str(), second.name.as_str()), ("cam2", "Camera 2"));
    assert_eq!(second.axes, first.axes, "it starts with the selected camera's limits");
    assert_eq!(second.rig.kind, ptz_core::pose::RigKind::Simulator);
    assert_eq!((second.mount.x - first.mount.x, second.mount.y), (2.0, first.mount.y), "two metres to its right");
    assert_eq!(r.harness.state().session.cam_id(), "cam2", "the new camera is selected");
    sent.validate().unwrap();

    // Removing a camera that has presets asks first, and says how many go with it.
    r.press("1 Camera 1");
    r.press("Remove camera");
    assert!(projects(&r).is_empty(), "nothing is removed on the first press");
    r.press("Remove it and its 3 presets");
    let sent = projects(&r).pop().expect("the project was sent");
    assert_eq!(sent.cameras.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["cam2"]);
    assert_eq!(r.harness.state().session.cam_id(), "cam2");

    // Twelve is the most, and the last one cannot be removed.
    {
        let s = &mut r.harness.state_mut().session;
        let mut project = s.project.clone();
        while project.cameras.len() < 12 {
            let id = project.free_camera_id();
            let name = format!("Camera {}", project.cameras.len() + 1);
            project.cameras.push(ptz_core::CameraConfig::new(&id, &name));
        }
        project.validate().unwrap();
        s.put_project(project);
    }
    r.harness.run_steps(3);
    let _ = projects(&r);
    r.harness.get_by_label("12 of 12 cameras. Keys 1 to 9, 0, - and = select them.");
    r.press("Add a camera");
    assert!(projects(&r).is_empty(), "a thirteenth camera is not offered");
    r.harness.key_press(egui::Key::Equals);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.cam_index(), 11, "= selects camera 12");

    // While armed the camera list is fixed.
    r.harness.state_mut().session.status.transport.output = Output::Armed;
    r.harness.run_steps(3);
    r.press("Remove camera");
    assert!(projects(&r).is_empty());
}

// ---------------------------------------------------------------- stage, lens, multiview

impl Rig {
    fn projects(&self) -> Vec<Project> {
        self.cmds.try_iter().filter_map(|c| if let Cmd::PutProject(p) = c { Some(*p) } else { None }).collect()
    }

    /// Where a point of the world is in the Stage view.
    fn stage_point(&self, p: [f32; 3]) -> Pos2 {
        let probe = self.harness.state().session.stage_view.probe.expect("the Stage view was drawn");
        let at = probe.screen(p).expect("the point is in front of the viewer");
        assert!(probe.rect.shrink(4.0).contains(at), "{p:?} is outside the Stage view at {at:?}");
        at
    }

    fn mount(&self, id: &str) -> ptz_core::camera::Mount {
        self.harness.state().session.project.camera(id).unwrap().mount
    }
}

#[test]
fn the_program_monitor_is_the_selected_cameras_view_through_its_lens() {
    use crate::stage::view_of;
    use egui_kittest::kittest::Queryable as _;
    let (mut project, presets) = three_cameras();
    // The second camera stands ahead and to the right of the first, inside its widest picture.
    project.cameras[1].mount.x = 3.0;
    project.cameras[1].mount.y = 6.0;
    let mut r = rig_with(project, presets, Song::new("s", "Test Song", "01:00:00:00", 1500));

    // Where the second camera's head lands in the first camera's picture, and how far away the picture says it is.
    let seen = |r: &Rig| {
        let s = &r.harness.state().session;
        let frame = s.world.frame("program").expect("the Program monitor was drawn");
        let view = view_of(&s.project.cameras[0], &s.monitor_pose().0);
        let head = s.project.cameras[1].mount.lens_position(0.0).map(|v| v as f32);
        let at = view.project(head, frame.width, frame.height);
        let inside = at.filter(|p| (0.0..frame.width as f32).contains(&p[0]) && (0.0..frame.height as f32).contains(&p[1]));
        (inside.and_then(|p| frame.depth_at(p[0] as usize, p[1] as usize)), frame.depth_at(frame.width / 2, frame.height / 2))
    };
    let (marker, centre) = seen(&r);
    let marker = marker.expect("at 28 mm the second camera is in the picture");
    assert!((5.6..6.1).contains(&marker), "its head is drawn 6 m away, in front of the wall: {marker}");
    assert!((15.9..16.1).contains(&centre.unwrap()), "the centre of the picture rests on the back wall: {centre:?}");
    r.settings(Page::Lens);
    r.harness.get_by_label("16.0 m away");
    r.harness.get_by_label("28 mm at zoom 0%");
    r.panels();

    // A longer lens on the first camera narrows its picture: the second camera is no longer in it.
    {
        let s = &mut r.harness.state_mut().session;
        let mut project = s.project.clone();
        project.cameras[0].lens = ptz_core::lens::Lens::presets().into_iter().find(|l| l.wide_mm == 70.0).unwrap();
        s.put_project(project);
    }
    r.harness.run_steps(3);
    assert_eq!(seen(&r).0, None, "at 70 mm the second camera is outside the picture");
    r.settings(Page::Lens);
    r.harness.get_by_label("70 mm at zoom 0%");
    r.harness.get_by_label("16.0 m away");
    r.panels();

    // The second camera keeps its own lens and its own place: from 6 m nearer, the wall is 10 m away.
    r.press("2 Centre");
    let s = &r.harness.state().session;
    let frame = s.world.frame("program").unwrap();
    let centre = frame.depth_at(frame.width / 2, frame.height / 2).unwrap();
    assert!((9.9..10.1).contains(&centre), "{centre}");
    r.settings(Page::Lens);
    r.harness.get_by_label("28 mm at zoom 0%");
    r.harness.get_by_label("10.0 m away");

    // Looking at nothing: the figures fall back to a typed distance and say so.
    {
        let s = &mut r.harness.state_mut().session;
        let mut project = s.project.clone();
        project.stage.show_builtin = false;
        s.put_project(project);
    }
    r.harness.run_steps(3);
    r.harness.get_by_label("looks at nothing on the stage");
}

#[test]
fn the_multiview_has_a_tile_for_each_of_twelve_cameras_and_a_click_selects() {
    let mut project = Project::default();
    while project.cameras.len() < 12 {
        let id = project.free_camera_id();
        let name = format!("Camera {}", project.cameras.len() + 1);
        let mut camera = ptz_core::CameraConfig::new(&id, &name);
        camera.mount.x = project.cameras.len() as f64 * 1.5 - 9.0;
        project.cameras.push(camera);
    }
    project.validate().unwrap();
    let mut r = rig_with(project, presets(), Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.harness.state_mut().show_tab(Tab::Multiview);
    r.harness.run_steps(6);
    let s = &r.harness.state().session;
    assert_eq!(s.probe_tiles.len(), 12);
    for (i, camera) in s.project.cameras.iter().enumerate() {
        assert_eq!(s.probe_tiles[i].0, camera.id);
        let frame = s.world.frame(&format!("multi:{}", camera.id)).unwrap_or_else(|| panic!("{} has no picture", camera.id));
        assert!(frame.depth_at(frame.width / 2, frame.height / 2).is_some(), "{} shows the stage", camera.id);
    }
    // The grid is four across and three down.
    let first_row: Vec<f32> = s.probe_tiles[..4].iter().map(|t| t.1.center().y).collect();
    assert!(first_row.iter().all(|y| (y - first_row[0]).abs() < 0.5));
    assert!(s.probe_tiles[4].1.center().y > first_row[0] + 20.0);

    let at = s.probe_tiles[6].1.center();
    r.click(at);
    assert_eq!(r.harness.state().session.cam_id(), "cam7");

    // Nothing is drawn again while nothing changes.
    let before: Vec<f32> = (1..=12).map(|n| r.harness.state().session.world.timings[&format!("multi:cam{n}")]).collect();
    r.harness.run_steps(5);
    let after: Vec<f32> = (1..=12).map(|n| r.harness.state().session.world.timings[&format!("multi:cam{n}")]).collect();
    assert_eq!(before, after);
}

#[test]
fn cameras_are_selected_moved_turned_placed_and_aimed_in_the_stage_view() {
    use crate::stage_ui::{Angle, Tool};
    let (project, presets) = three_cameras();
    let mut r = rig_with(project, presets, Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.harness.state_mut().show_tab(Tab::Stage);
    r.harness.run_steps(4);
    assert!(r.harness.state().session.world.frame("stage").is_some());

    // The whole stage and every camera are in view to start with, and a camera's name is a handle on it.
    let labels = r.harness.state().session.stage_view.labels.clone();
    let view = r.harness.state().session.stage_view.probe.unwrap().rect;
    assert_eq!(labels.len(), 3);
    assert!(labels.iter().all(|label| view.contains_rect(*label)), "{labels:?} in {view:?}");
    r.click(labels[2].center());
    assert_eq!(r.harness.state().session.cam_id(), "cam3");

    // Select: a click on a camera's marker selects it.
    let at = r.stage_point([4.0, 0.0, 0.6]);
    r.click(at);
    assert_eq!(r.harness.state().session.cam_id(), "cam2");
    assert!(r.projects().is_empty(), "selecting changes nothing in the project");

    // Move: the camera follows the pointer across the floor, and the project is sent once, on release.
    r.press("Move");
    assert_eq!(r.harness.state().session.stage_view.tool, Tool::Move);
    let (from, to) = (r.stage_point([8.0, 0.0, 0.0]), r.stage_point([6.0, 3.0, 0.0]));
    r.drag(from, to);
    let m = r.mount("cam3");
    assert!((m.x - 6.0).abs() < 0.06 && (m.y - 3.0).abs() < 0.06 && m.z == 0.0, "{m:?}");
    let sent = r.projects();
    assert_eq!(sent.len(), 1, "one drag is one project update");
    assert_eq!(sent[0].cameras[2].mount, m);
    assert_eq!(r.harness.state().session.cam_id(), "cam3", "the camera taken hold of is selected");

    // The same drag from above, in the Top view.
    r.press("Top");
    assert_eq!(r.harness.state().session.stage_view.angle, Angle::Top);
    let labels = r.harness.state().session.stage_view.labels.clone();
    assert!(labels.iter().all(|label| view.contains_rect(*label)), "the plan is fitted to the view: {labels:?} in {view:?}");
    let (from, to) = (r.stage_point([6.0, 3.0, 0.0]), r.stage_point([-5.0, 2.0, 0.0]));
    r.drag(from, to);
    let m = r.mount("cam3");
    assert!((m.x + 5.0).abs() < 0.08 && (m.y - 2.0).abs() < 0.08, "{m:?}");
    assert_eq!(r.projects().len(), 1);

    // Rotate: dragging from ahead of the camera round to its left turns its heading a quarter turn.
    r.press("Rotate");
    let (from, to) = (r.stage_point([-5.0, 4.0, 0.0]), r.stage_point([-7.0, 2.0, 0.0]));
    r.drag(from, to);
    let m = r.mount("cam3");
    assert!((m.heading_deg - 90.0).abs() < 1.5, "{m:?}");
    assert!((m.x + 5.0).abs() < 0.08, "turning does not move it");
    assert_eq!(r.projects().len(), 1);

    // Place: a click on the deck stands the selected camera there, at the deck's height.
    r.press("Perspective");
    r.press("Place");
    let at = r.stage_point([2.0, 11.0, crate::stage::DECK_HEIGHT]);
    r.click(at);
    let m = r.mount("cam3");
    assert!((m.x - 2.0).abs() < 0.08 && (m.y - 11.0).abs() < 0.08, "{m:?}");
    assert!((m.z - 0.5).abs() < 0.011, "it stands on the deck: {m:?}");
    assert_eq!(r.projects().len(), 1);

    // Aim: a click turns pan 0 to face the point.
    r.press("Aim");
    let at = r.stage_point([2.0, 15.0, crate::stage::DECK_HEIGHT]);
    r.click(at);
    assert!(r.mount("cam3").heading_deg.abs() < 1.0, "{:?}", r.mount("cam3"));
    let at = r.stage_point([-6.0, 11.0, crate::stage::DECK_HEIGHT]);
    r.click(at);
    assert!((r.mount("cam3").heading_deg - 90.0).abs() < 1.0, "{:?}", r.mount("cam3"));
    assert_eq!(r.projects().len(), 2);

    // Undo move puts the last change back, one step at a time.
    r.press("Undo move");
    assert!(r.mount("cam3").heading_deg.abs() < 1.0);
    r.press("Undo move");
    r.press("Undo move");
    let m = r.mount("cam3");
    assert!((m.x + 5.0).abs() < 0.08 && m.z == 0.0, "back on the floor where it stood before Place: {m:?}");
    assert_eq!(r.projects().len(), 3);

    // Measure: two clicks give the distance between two points of the stage.
    r.press("Measure");
    let (a, b) = (r.stage_point([-3.0, 2.0, 0.0]), r.stage_point([0.0, 6.0, 0.0]));
    r.click(a);
    r.click(b);
    let measure = r.harness.state().session.stage_view.measure.clone();
    assert_eq!(measure.len(), 2);
    let d = (measure[1][0] - measure[0][0]).hypot(measure[1][1] - measure[0][1]);
    assert!((d - 5.0).abs() < 0.1, "{measure:?}");
    assert!(r.projects().is_empty());

    // A station that is only watching can look, but cannot move a camera.
    r.harness.state_mut().session.status.you_hold_lease = false;
    r.press("Move");
    let before = r.mount("cam1");
    let (from, to) = (r.stage_point([0.0, 0.0, 0.0]), r.stage_point([1.0, 3.0, 0.0]));
    r.drag(from, to);
    assert_eq!(r.mount("cam1"), before);
    assert!(r.projects().is_empty());
    assert!(r.toasts().iter().any(|t| t.contains("lease")), "{:?}", r.toasts());
}

#[derive(Debug)]
struct Dropped(std::path::PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

impl Rig {
    /// Let go of a file over the window.
    fn drop_file(&mut self, path: &str) {
        self.harness.input_mut().dropped_files.push(std::sync::Arc::new(Dropped(path.into())));
    }
}

/// A hall 40 by 9 by 25 in millimetres, Y up, as an OBJ exporter would write it.
fn hall_mesh() -> ptz_scene::mesh::Mesh {
    let mut hall = ptz_scene::mesh::Mesh::default();
    hall.push_cuboid([0.0, 0.0, -25_000.0], [40_000.0, 9_000.0, 0.0], [90, 90, 100]);
    hall
}

fn hall_settings(revision: u64) -> ptz_core::camera::StageModel {
    ptz_core::camera::StageModel {
        name: "hall.obj".into(),
        revision,
        triangles: 12,
        min: [0.0, 0.0, -25_000.0],
        max: [40_000.0, 9_000.0, 0.0],
        unit: ptz_core::camera::Unit::Mm,
        up: ptz_core::camera::UpAxis::Y,
        ..Default::default()
    }
}

#[test]
fn a_dropped_model_is_imported_shown_with_its_size_and_placed_from_show_settings() {
    use crate::stage::Import;
    use egui_kittest::kittest::Queryable as _;
    use std::path::PathBuf;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.settings(Page::Stage);
    r.harness
        .get_by_label("The built-in stage is shown. Import the venue to see what each camera sees from where it will stand.");
    let _ = r.cmds.try_iter().count();

    // A model dropped on the window is imported; its material file is left alone; other files are media.
    for name in ["/venues/hall.obj", "/venues/hall.mtl", "/audio/walk-on.wav", "/venues/Rig Plot.3DS"] {
        r.drop_file(name);
    }
    r.harness.run_steps(3);
    let cmds: Vec<Cmd> = r.cmds.try_iter().collect();
    let imports: Vec<&PathBuf> = cmds.iter().filter_map(|c| if let Cmd::ImportStage(p) = c { Some(p) } else { None }).collect();
    assert_eq!(imports, vec![&PathBuf::from("/venues/hall.obj"), &PathBuf::from("/venues/Rig Plot.3DS")]);
    let uploads: Vec<&PathBuf> = cmds.iter().filter_map(|c| if let Cmd::Upload(p) = c { Some(p) } else { None }).collect();
    assert_eq!(uploads, vec![&PathBuf::from("/audio/walk-on.wav")]);

    // While it is read and sent the Stage model page says so.
    r.events.send(Event::StageImport(Import::Reading("hall.obj".into()))).unwrap();
    r.harness.run_steps(3);
    r.harness.get_by_label("Reading hall.obj");

    // It arrives: the mesh, then the project that names it. Nothing is fetched, because the app already holds it.
    let mut project = r.harness.state().session.project.clone();
    project.stage = ptz_core::camera::Stage { show_builtin: false, model: Some(hall_settings(1)) };
    r.events.send(Event::StageModel { revision: 1, mesh: std::sync::Arc::new(hall_mesh()) }).unwrap();
    r.events.send(Event::Project(Box::new(project))).unwrap();
    r.events.send(Event::StageImport(Import::Done("hall.obj".into()))).unwrap();
    r.harness.run_steps(4);
    assert!(!r.cmds.try_iter().any(|c| matches!(c, Cmd::FetchStageModel(_))));
    let s = &r.harness.state().session;
    assert_eq!(s.world.triangles(), 12, "the hall replaces the built-in stage");
    assert_eq!(s.world.bounds().unwrap(), ([0.0, 0.0, 0.0], [40.0, 25.0, 9.0]));
    r.harness.get_by_label("hall.obj   12 triangles");
    r.harness.get_by_label("40.0 m wide, 25.0 m deep, 9.0 m high");
    r.harness.get_by_label("It spans x 0.0 to 40.0, y 0.0 to 25.0, z 0.0 to 9.0 m");

    // Centre moves the footprint's middle to the origin; the picture follows without another fetch.
    r.press("Centre");
    let sent = r.projects().pop().expect("the project was sent");
    assert_eq!(sent.stage.model.as_ref().unwrap().offset, [-20.0, -12.5, 0.0]);
    assert_eq!(r.harness.state().session.world.bounds().unwrap(), ([-20.0, -12.5, 0.0], [20.0, 12.5, 9.0]));
    r.harness.get_by_label("It spans x -20.0 to 20.0, y -12.5 to 12.5, z 0.0 to 9.0 m");

    // The unit decides the size: read as centimetres the hall is ten times larger, and Sit on the floor still works.
    {
        let s = &mut r.harness.state_mut().session;
        let mut project = s.project.clone();
        let model = project.stage.model.as_mut().unwrap();
        model.unit = ptz_core::camera::Unit::Cm;
        model.offset = [0.0, 0.0, -3.0];
        s.put_project(project);
    }
    r.harness.run_steps(3);
    let _ = r.projects();
    r.harness.get_by_label("400.0 m wide, 250.0 m deep, 90.0 m high");
    r.press("Sit on the floor");
    assert_eq!(r.projects().pop().unwrap().stage.model.unwrap().offset, [0.0, 0.0, 0.0]);

    // A file that cannot be read says why, and changes nothing.
    let before = r.harness.state().session.project.clone();
    let why = "line 4127: face refers to vertex 90211, but the file has 88410";
    r.events.send(Event::StageImport(Import::Failed { name: "broken.obj".into(), why: why.into() })).unwrap();
    r.harness.run_steps(3);
    let said = format!("broken.obj was not imported: {why}");
    assert_eq!(r.harness.query_all_by_label(said.as_str()).count(), 2, "on the Stage model page and as a notice");
    assert_eq!(r.harness.state().session.project, before);
    assert_eq!(r.harness.state().session.world.triangles(), 12);

    // Removing asks twice, because it removes the model for every station.
    r.press("Remove the model");
    assert!(!r.cmds.try_iter().any(|c| matches!(c, Cmd::RemoveStageModel)));
    r.press("Remove it for every station");
    assert!(r.cmds.try_iter().any(|c| matches!(c, Cmd::RemoveStageModel)));

    // A project that names a model this window does not hold fetches it, once.
    let mut project = r.harness.state().session.project.clone();
    project.stage.model = Some(hall_settings(2));
    r.events.send(Event::Project(Box::new(project))).unwrap();
    r.harness.run_steps(6);
    let fetches: Vec<u64> =
        r.cmds.try_iter().filter_map(|c| if let Cmd::FetchStageModel(n) = c { Some(n) } else { None }).collect();
    assert_eq!(fetches, vec![2]);
    assert_eq!(r.harness.state().session.world.triangles(), 0, "the old model is not shown under the new one's settings");
    r.harness.get_by_label("Fetching the model from the server");
    r.events.send(Event::StageModel { revision: 2, mesh: std::sync::Arc::new(hall_mesh()) }).unwrap();
    r.harness.run_steps(4);
    assert_eq!(r.harness.state().session.world.triangles(), 12);

    // A station that is only watching cannot import.
    r.harness.state_mut().session.status.you_hold_lease = false;
    r.drop_file("/venues/other.obj");
    r.harness.run_steps(3);
    assert!(!r.cmds.try_iter().any(|c| matches!(c, Cmd::ImportStage(_))));
    assert!(r.toasts().iter().any(|t| t.contains("to import a stage model")), "{:?}", r.toasts());
}

#[test]
fn a_layout_saved_before_the_stage_tabs_existed_gains_them_beside_the_program_monitor() {
    use egui_dock::{DockState, NodeIndex};
    let mut old = DockState::new(vec![Tab::Reference, Tab::Program]);
    let surface = old.main_surface_mut();
    let [top, _] = surface.split_below(NodeIndex::root(), 0.6, vec![Tab::Timeline, Tab::Graph, Tab::Log]);
    let [centre, _] = surface.split_left(top, 0.25, vec![Tab::Presets, Tab::Media]);
    let [_, _] = surface.split_right(centre, 0.6, vec![Tab::Rig, Tab::Inspector]);
    let program = old.find_tab(&Tab::Program).unwrap();
    old.set_active_tab(program).unwrap();
    let before = serde_json::to_value(&old).unwrap();

    let (link, _cmds, _events) = Link::detached();
    let session = Session::offline(link, Project::default(), presets(), Song::new("s", "Test Song", "01:00:00:00", 1500));
    let shell = Shell::new(session, Some(old));
    for tab in Tab::ALL {
        assert!(shell.dock.find_tab(&tab).is_some(), "{tab:?} is in the layout");
    }
    let (program, stage, multiview) = (
        shell.dock.find_tab(&Tab::Program).unwrap(),
        shell.dock.find_tab(&Tab::Stage).unwrap(),
        shell.dock.find_tab(&Tab::Multiview).unwrap(),
    );
    assert_eq!((stage.surface, stage.node), (program.surface, program.node));
    assert_eq!((multiview.surface, multiview.node), (program.surface, program.node));
    let leaf = shell.dock[program.surface][program.node].get_leaf().unwrap();
    assert_eq!(leaf.active, program.tab, "the Program monitor stays in front");
    assert_eq!(leaf.tabs(), &[Tab::Reference, Tab::Program, Tab::Stage, Tab::Multiview]);
    // The set list joins the presets, behind them.
    let presets_at = shell.dock.find_tab(&Tab::Presets).unwrap();
    let leaf = shell.dock[presets_at.surface][presets_at.node].get_leaf().unwrap();
    assert_eq!(leaf.tabs(), &[Tab::Presets, Tab::Media, Tab::SetList]);
    assert_eq!(leaf.active, presets_at.tab);
    // The splits the operator made are untouched.
    let nodes = |v: &serde_json::Value| v["surfaces"][0]["Main"]["nodes"].as_array().map(Vec::len);
    assert_eq!(nodes(&before), nodes(&serde_json::to_value(&shell.dock).unwrap()));

    // A layout that already has every tab is kept as it is.
    let tabs = |dock: &DockState<Tab>| -> Vec<Tab> { dock.iter_all_tabs().map(|(_, tab)| *tab).collect() };
    let (link, _cmds, _events) = Link::detached();
    let session = Session::offline(link, Project::default(), presets(), Song::new("s", "Test Song", "01:00:00:00", 1500));
    let again = Shell::new(session, Some(crate::app::default_layout()));
    assert_eq!(tabs(&again.dock), tabs(&crate::app::default_layout()));
    assert_eq!(tabs(&again.dock).len(), Tab::ALL.len());
}

// ---------------------------------------------------------------- audit follow-ups

#[test]
fn take_over_asks_before_taking_the_lease_from_another_station() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    let takes = |r: &Rig| -> Vec<bool> {
        r.cmds.try_iter().filter_map(|c| if let Cmd::TakeLease { force, .. } = c { Some(force) } else { None }).collect()
    };
    {
        let s = &mut r.harness.state_mut().session;
        s.status.you_hold_lease = false;
        s.status.lease =
            Some(ptz_core::api::LeaseStatus { holder: "Front of house".into(), client_id: "other".into(), expires_in_ms: 9000 });
    }
    r.harness.run_steps(3);

    r.press("Take over");
    r.harness.get_by_label("Take the edit lease from Front of house?");
    assert!(takes(&r).is_empty(), "one click does not take the lease from someone");
    r.press("Cancel");
    assert!(r.harness.query_by_label("Take the edit lease from Front of house?").is_none());
    assert!(takes(&r).is_empty());

    r.press("Take over");
    r.press("Take it from Front of house");
    assert_eq!(takes(&r), vec![true]);
    assert!(r.harness.query_by_label("Take the edit lease from Front of house?").is_none());

    // A free lease is taken at once: there is nobody to take it from.
    r.harness.state_mut().session.status.lease = None;
    r.harness.run_steps(3);
    r.press("Take the lease");
    assert_eq!(takes(&r), vec![false]);
}

#[test]
fn a_station_that_loses_the_lease_is_told_who_took_it() {
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    assert!(r.harness.state().session.status.you_hold_lease);
    let mut status = r.harness.state().session.status.clone();
    status.you_hold_lease = false;
    status.lease = Some(ptz_core::api::LeaseStatus { holder: "Balcony".into(), client_id: "other".into(), expires_in_ms: 9000 });
    r.harness.state_mut().session.on_event(Event::Status(Box::new(status.clone())));
    assert_eq!(r.toasts().iter().filter(|t| t.starts_with("Balcony took the edit lease")).count(), 1, "{:?}", r.toasts());
    // It is said once, not on every status that follows.
    r.harness.state_mut().session.on_event(Event::Status(Box::new(status)));
    assert_eq!(r.toasts().iter().filter(|t| t.starts_with("Balcony took the edit lease")).count(), 1);
}

#[test]
fn a_move_whose_preset_was_stored_again_offers_to_update() {
    use egui_kittest::kittest::Queryable as _;
    let empty = Song::new("s", "Test Song", "01:00:00:00", 1500);
    let (song, id) = ptz_core::edit::insert_preset_move(&empty, &Project::default(), &presets()[1], 100).unwrap();
    let mut r = rig(song);
    r.harness.state_mut().session.timeline.selection.insert(id.clone());
    r.harness.state_mut().show_tab(Tab::Inspector);
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label("Update").is_none(), "nothing to update while the preset is as it was");

    let name = presets()[1].name.clone();
    r.harness.state_mut().session.presets[1].pose.pan += 7.5;
    let now = r.harness.state().session.presets[1].pose;
    r.harness.run_steps(3);
    r.harness.get_by_label(&format!("{name} has changed"));
    let pose_of = |r: &Rig| match &r.song().motion_clip(&id).unwrap().body {
        ptz_core::song::ClipBody::PresetMove { to, .. } => to.pose,
        other => panic!("{other:?}"),
    };
    assert_ne!(pose_of(&r), now, "the move keeps the pose it was made with");

    let undo = r.harness.state().session.undo.len();
    r.press("Update");
    assert_eq!(pose_of(&r), now);
    assert_eq!(r.harness.state().session.undo.len(), undo + 1, "one undo step");
    assert!(r.harness.query_by_label("Update").is_none());
}

#[test]
fn choosing_a_lens_from_the_list_sets_the_camera_lens_and_keeps_its_sensor() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.settings(Page::Lens);
    let _ = r.projects();
    let lenses = ptz_core::lens::Lens::presets();
    let before = r.harness.state().session.project.cameras[0].lens.clone();
    assert_eq!(before, lenses[0]);

    // A combo box carries what is chosen as its value, not as its label.
    r.harness.get_by_value(&before.name).scroll_to_me();
    r.harness.run_steps(40);
    r.harness.get_by_value(&before.name).click();
    r.harness.run_steps(3);
    r.press(&lenses[3].name);
    let sent = r.projects().pop().expect("the project was sent");
    let lens = &sent.cameras[0].lens;
    assert_eq!((lens.name.as_str(), lens.wide_mm, lens.tele_mm), (lenses[3].name.as_str(), 70.0, 200.0));
    assert_eq!(lens.sensor_width_mm, before.sensor_width_mm);
    sent.validate().unwrap();
}

#[test]
fn servers_this_station_reached_are_remembered_newest_first() {
    let mut recent = Vec::new();
    for url in ["http://a:8787", "http://b:8787", " http://a:8787 ", "", "http://c:8787"] {
        crate::app::remember_server(&mut recent, url);
    }
    assert_eq!(recent, vec!["http://c:8787", "http://a:8787", "http://b:8787"]);
    for n in 0..9 {
        crate::app::remember_server(&mut recent, &format!("http://host{n}:8787"));
    }
    assert_eq!(recent.len(), 5);
    assert_eq!(recent[0], "http://host8:8787");
}

#[test]
fn the_set_list_tab_arranges_the_show_and_loads_a_song_at_a_click() {
    use ptz_core::api::SongSummary;
    let summary = |id: &str, name: &str, start: &str| SongSummary {
        id: id.into(),
        name: name.into(),
        start_tc: start.into(),
        length_frames: 1500,
        revision: Some(1),
        dirty: false,
        loaded: false,
    };
    let project = Project { set_list: vec!["opener".into()], ..Project::default() };
    let mut r = rig_with(project, presets(), Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.harness.state_mut().session.songs =
        vec![summary("opener", "Opener", "01:00:00:00"), summary("closer", "Closer", "01:10:00:00")];
    r.harness.state_mut().show_tab(Tab::SetList);
    r.harness.run_steps(3);
    let _ = r.cmds.try_iter().count();

    // Until Arrange is pressed a row is only a song to load.
    {
        use egui_kittest::kittest::Queryable as _;
        assert!(r.harness.query_by_label("Add").is_none() && r.harness.query_by_label("Remove").is_none());
    }
    r.press("Arrange");
    // The one song outside the list is added at its end, and the show is sent.
    r.press("Add");
    let sent: Vec<Cmd> = r.cmds.try_iter().collect();
    let lists: Vec<&Vec<String>> =
        sent.iter().filter_map(|c| if let Cmd::PutProject(p) = c { Some(&p.set_list) } else { None }).collect();
    assert_eq!(lists, [&vec!["opener".to_string(), "closer".to_string()]]);

    // A click on a song loads it. Page Up from a song that is not in the list goes to the last one.
    r.press("Opener");
    r.harness.key_press(egui::Key::PageUp);
    r.harness.run_steps(2);
    let loads: Vec<String> = r.cmds.try_iter().filter_map(|c| if let Cmd::LoadSong(id) = c { Some(id) } else { None }).collect();
    assert_eq!(loads, ["opener", "closer"]);
}

#[test]
fn the_timeline_is_scrolled_by_the_bar_along_its_bottom() {
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    let scroll = |r: &Rig| r.harness.state().session.timeline.view.scroll;
    let bar = |r: &Rig| safety::rect_of(r, "Scroll the timeline");
    let body_w = |r: &Rig| r.layout().body.width();

    // The bar is under the lanes and beside the lane names, and the view starts at the start.
    let (at, l) = (bar(&r), r.layout());
    assert!(at.top() >= l.lanes.last().unwrap().rect.bottom() - 0.5, "the bar covers no lane: {at:?}");
    assert!((at.left() - l.body.left()).abs() < 0.5 && (at.right() - l.body.right()).abs() < 0.5, "{at:?}");
    assert_eq!(scroll(&r), 0.0);

    // Zoomed in, a press at the far end of the bar goes to the end of the song.
    for _ in 0..5 {
        r.press("+");
    }
    let view = r.harness.state().session.timeline.view;
    let max = view.scroll_max(1500, body_w(&r));
    assert!(max > 0.0);
    r.click(bar(&r).right_center() - vec2(2.0, 0.0));
    assert!((scroll(&r) - max).abs() < 1.0, "{} of {max}", scroll(&r));

    // The thumb is dragged: it stays under the pointer, so the view goes by that share of the track.
    let view = r.harness.state().session.timeline.view;
    let (thumb_at, thumb_len) = view.thumb(1500, body_w(&r), body_w(&r));
    let l = r.layout();
    let from = pos2(l.body.left() + thumb_at + thumb_len / 2.0, bar(&r).center().y);
    r.drag(from, from - vec2(120.0, 0.0));
    let want = max * (thumb_at - 120.0) as f64 / (body_w(&r) - thumb_len) as f64;
    assert!((scroll(&r) - want).abs() < max * 0.01, "{} for {want}", scroll(&r));

    // And the ruler and the lanes went with it; nothing in the song was touched.
    assert_eq!(r.layout().scroll, scroll(&r));
    assert!(r.song().motion.is_empty() && r.song().markers.is_empty());
    assert!(r.harness.state().session.timeline.selection.is_empty());

    // The graph editor shares the view and has the same bar, under its curves and beside the axis names.
    r.harness.state_mut().show_tab(Tab::Graph);
    r.harness.run_steps(3);
    let bar = safety::rect_of(&r, "Scroll the graph");
    let plot = r.harness.state().session.graph.probe.expect("the graph was drawn");
    assert!((bar.top() - plot.bottom()).abs() < 0.5, "the bar is under the curves: {bar:?} {plot:?}");
    assert!((bar.left() - plot.left() - crate::timeline::HEADER_W).abs() < 0.5);
    r.click(bar.left_center() + vec2(2.0, 0.0));
    assert_eq!(scroll(&r), 0.0);
    r.click(bar.right_center() - vec2(2.0, 0.0));
    let max = r.harness.state().session.timeline.view.scroll_max(1500, bar.width());
    assert!(max > 0.0 && (scroll(&r) - max).abs() < 1.0, "{} of {max}", scroll(&r));
}

#[test]
fn the_stage_view_is_turned_slid_and_zoomed_from_its_pad_with_no_wheel() {
    use crate::stage_ui::{Angle, Nav};
    let (project, presets) = three_cameras();
    let mut r = rig_with(project, presets, Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.harness.state_mut().show_tab(Tab::Stage);
    r.harness.run_steps(4);
    let view = |r: &Rig| {
        let v = &r.harness.state().session.stage_view;
        (v.yaw, v.pitch, v.distance, v.target)
    };
    let (yaw, pitch, distance, target) = view(&r);

    // Zoom: one press comes a fifth nearer, and the other button goes back.
    r.press("Zoom in");
    assert!((view(&r).2 / distance - 0.8).abs() < 1e-3, "{} from {distance}", view(&r).2);
    r.press("Zoom out");
    assert!((view(&r).2 / distance - 1.0).abs() < 1e-3);

    // The arrows turn the view to start with, and leave what it looks at alone.
    assert_eq!(r.harness.state().session.stage_view.nav, Nav::Turn);
    r.press("View right");
    r.press("View up");
    let turned = view(&r);
    assert!(((turned.0 - yaw).rem_euclid(360.0) - 15.0).abs() < 1e-3, "{} from {yaw}", turned.0);
    assert!((turned.1 - (pitch - 15.0).max(-89.0)).abs() < 1e-3, "{} from {pitch}", turned.1);
    assert_eq!(turned.3, target);

    // Set to slide, the arrows move it across the stage, and so does a plain drag on the picture.
    r.press("Slide the view");
    assert_eq!(r.harness.state().session.stage_view.nav, Nav::Slide);
    r.press("View left");
    let slid = view(&r);
    assert_eq!((slid.0, slid.1, slid.2), (turned.0, turned.1, turned.2), "sliding does not turn or zoom");
    assert_ne!(slid.3, target);
    let rect = r.harness.state().session.stage_view.probe.unwrap().rect;
    let from = rect.left_bottom() + vec2(rect.width() * 0.3, -rect.height() * 0.3);
    r.drag(from, from + vec2(60.0, 0.0));
    let dragged = view(&r);
    assert_eq!((dragged.0, dragged.1), (slid.0, slid.1), "a drag slides while the pad is set to slide");
    assert_ne!(dragged.3, slid.3);

    // A press on the pad is not a press on the picture under it: no camera is selected or moved by it.
    assert!(r.projects().is_empty());

    // The middle button shows everything again, and the flat views still answer to the pad.
    r.press("Top");
    r.press("Show everything");
    r.press("Turn the view");
    assert_eq!(r.harness.state().session.stage_view.angle, Angle::Top);
    r.press("View right");
    assert_eq!(r.harness.state().session.stage_view.angle, Angle::Perspective);
}

/// A newer version is offered in the View menu, and is not installed while the show is live:
/// installing closes the app.
#[test]
fn a_newer_version_is_offered_in_the_view_menu_and_waits_for_the_show_to_stop() {
    use crate::update::{Installer, Release, Step};
    use egui_kittest::kittest::{NodeT as _, Queryable as _};
    use ptz_core::transport::Output;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    r.harness.run_steps(2);
    assert!(r.toasts().is_empty(), "{:?}", r.toasts());

    let installer =
        Installer { name: "Motion-Choreographer-0.9.0-macos-arm64.pkg".into(), size: 13_000_000, sha256: "0".repeat(64) };
    let release = Release {
        version: "0.9.0".into(),
        platform: "macos-arm64".into(),
        installer,
        notes: "Scroll bars on the timeline.".into(),
    };
    r.harness.state().updater.set(Step::Available(release));
    r.harness.run_steps(3);
    assert_eq!(r.toasts().iter().filter(|t| t.contains("Version 0.9.0 is available")).count(), 1, "said once: {:?}", r.toasts());

    r.harness.state_mut().session.status.transport.output = Output::Armed;
    r.harness.get_by_label("View").click();
    r.harness.run_steps(3);
    r.harness.get_by_label("Update to 0.9.0").click();
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label("Version 0.9.0 is available").is_some());
    assert!(r.harness.query_by_label("Scroll bars on the timeline.").is_some(), "it says what is new");
    assert!(r.harness.get_by_label("Install and reopen").accesskit_node().is_disabled(), "not while the rig is armed");
    assert!(r.harness.query_by_label_contains("Stop the show and disarm it first").is_some());

    r.harness.state_mut().session.status.transport.output = Output::Rehearse;
    r.harness.run_steps(3);
    assert!(!r.harness.get_by_label("Install and reopen").accesskit_node().is_disabled());

    r.harness.get_by_label("Later").click();
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label("Install and reopen").is_none(), "Later closes it");
    assert!(matches!(r.harness.state().updater.step(), Step::Available(_)), "and the version is still on offer");
}

mod picture;
mod safety;
mod settings;
mod shots;
