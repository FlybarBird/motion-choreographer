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
