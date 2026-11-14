//! Whole-window tests: real egui input events through the real widgets, with
//! a detached link in place of the server.

use crate::app::Shell;
use crate::link::{Cmd, Event, Link};
use crate::session::Session;
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
}

pub fn rig(song: Song) -> Rig {
    let (link, cmds, events) = Link::detached();
    let session = Session::offline(link, Project::default(), presets(), song);
    let shell = Shell::new(session, None);
    let mut themed = false;
    let harness = Harness::builder().with_size(vec2(1500.0, 920.0)).build_ui_state(
        move |ui, shell: &mut Shell| {
            if !themed {
                crate::theme::apply(ui.ctx());
                themed = true;
            }
            shell.ui(ui);
        },
        shell,
    );
    let mut rig = Rig { harness, cmds, events };
    rig.harness.run_steps(4);
    rig
}

impl Rig {
    fn song(&self) -> &Song {
        self.harness.state().session.song.as_ref().unwrap()
    }

    fn layout(&self) -> timeline::Layout {
        let s = &self.harness.state().session;
        timeline::layout(self.song(), &s.timeline.view, s.probe_timeline.expect("the timeline was drawn"))
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
    let to = r.lane_point(Lane::Motion, 200.0);
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
    let to = r.lane_point(Lane::Motion, 700.0);
    r.drag(from, to);
    assert_eq!(r.song().motion.len(), 2);
    let label = timeline::move_label(r.song(), &r.song().motion[1].id, &Project::default());
    assert!(label.starts_with("Lectern MCU to Awards Two-Shot"), "{label}");
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
    let y = l.lane_rect(Lane::Motion).unwrap().center().y;
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
    let y = l.lane_rect(Lane::Motion).unwrap().center().y;
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
fn the_transport_bar_names_the_output_or_says_why_it_is_silent() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    // The Inspector shows project settings when nothing is selected, including the output and routing.
    r.harness.state_mut().session.status.audio.silent_reason =
        Some("There is no default audio output device. Running silent".into());
    r.harness.run_steps(2);
    r.harness.get_by_label("There is no default audio output device. Running silent");
    r.harness.state_mut().session.status.audio.output = Some("MacBook Pro Speakers".into());
    r.harness.state_mut().session.status.audio.device_rate = 48_000;
    r.harness.run_steps(2);
    r.harness.get_by_label("MacBook Pro Speakers, 48000 Hz");
}
