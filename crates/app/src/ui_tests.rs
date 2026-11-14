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
    ptz_core::edit::punch_keys(&empty, &Project::default(), 100, 200, curves, None).unwrap()
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
        self.cmds.try_iter().filter_map(|c| if let Cmd::Jog(j) = c { Some(j) } else { None }).collect()
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
    assert!(cmds.iter().any(|c| matches!(c, Cmd::Jog(j) if (j.pan + speed).abs() < 1e-9)), "{}", cmds.len());
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
    r.harness.state_mut().session.timeline.view.motion_open = true;
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.record_axes, vec![Axis::Pan, Axis::Tilt]);

    let dot = |r: &Rig, axis: Axis| crate::timeline_ui::record_dot(&r.layout(), axis).expect("the axis lane is open");
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
    let name = pos2(l.rect.left() + 60.0, l.lane_rect(Lane::Axis(Axis::Tilt)).unwrap().center().y);
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
    let (recorded, clip) =
        ptz_core::edit::punch_keys(&before, &Project::default(), 0, 120, curves, Some("pass-0001".into())).unwrap();
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
    let shown = crate::graph::target_clip(r.song(), &s.timeline.selection, &s.timeline.key_selection, s.playhead());
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
        s.status.telemetry.error.pan = 1.4;
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
    let synced = |r: &Rig| r.cmds.try_iter().filter(|c| matches!(c, Cmd::SyncPresets)).count();

    // The simulator has nothing to move: one click syncs.
    r.harness.get_by_label("Sync from rig").click();
    r.harness.run_steps(2);
    assert_eq!(synced(&r), 1);

    // A real camera travels to every preset, so the click only asks.
    r.harness.state_mut().session.status.telemetry.hardware = true;
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
    r.harness.state_mut().session.status.telemetry.preset_sync = Some("Recalling preset 2 of 3: Lectern".into());
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
        s.status.telemetry.preset_sync = None;
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

    r.harness.state_mut().show_tab(Tab::Inspector);
    r.harness.run_steps(3);
    r.press("Add a keep-out rule");
    let sent = projects(&r);
    let rule = &sent.last().expect("the project was sent").keep_out[0];
    assert_eq!(rule.describe(), "when elevation is below 200 mm, tilt must be above -10.0°");
    assert_eq!(rule.name, "Rule 1");
    // The local compile already shows it: the move now breaks the rule, and that cannot be accepted.
    let s = &r.harness.state().session;
    assert_eq!(r.blocking(), 1);
    assert!(s.compiled.issues.iter().any(|i| i.message().contains("Keep-out rule broken") && !i.acceptable()));

    r.press("Remove");
    let sent = projects(&r);
    assert!(sent.last().unwrap().keep_out.is_empty());
    assert_eq!(r.blocking(), 0);
}

#[test]
fn rig_settings_switch_to_hardware_and_take_a_height_from_the_leg_box() {
    use egui_kittest::kittest::Queryable as _;
    let mut r = rig(Song::new("s", "Test Song", "01:00:00:00", 1500));
    let projects = |r: &Rig| -> Vec<Project> {
        r.cmds.try_iter().filter_map(|c| if let Cmd::PutProject(p) = c { Some(*p) } else { None }).collect()
    };
    r.harness.state_mut().show_tab(Tab::Inspector);
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label("FR7 address").is_none(), "the simulator has no addresses to set");
    {
        let s = &mut r.harness.state_mut().session;
        s.project.rig.kind = ptz_core::pose::RigKind::Hardware;
        s.project.rig.autopod_host = "10.0.0.31".into();
        s.status.adapters = vec![ptz_core::api::AdapterStatus {
            name: "AutoPod".into(),
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
    let map = &sent.last().expect("the project was sent").rig.autopod_calibration;
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
    fn blocking(&self) -> usize {
        let s = &self.harness.state().session;
        s.compiled.blocking_count(s.song.as_ref().unwrap())
    }
}
