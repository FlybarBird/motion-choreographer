//! Smoothing recorded moves over real HTTP: takes shaped like the ones a real head gives are
//! brought inside the rig's limits in one step, what cannot be is accepted or reported, and
//! the raw samples and the rig's limits are left as they were.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::camera::CameraConfig;
use ptz_core::client::Api;
use ptz_core::compile::Issue;
use ptz_core::edit;
use ptz_core::eval::{ease_value, keys_value, pose_at};
use ptz_core::fit::overruns;
use ptz_core::pose::{Axis, Project};
use ptz_core::song::{Ease, Song};
use ptz_core::transport::{Action, Output};
use ptz_server::record::{fit_pass, RecordedPass};
use std::collections::BTreeMap;

/// A move as a real head reports it: read at uneven moments, each frame holding the last
/// reading, so the take stands for a frame and then jumps to catch up.
fn late_readings(seed: u32, frames: usize, truth: impl Fn(f64) -> f64) -> Vec<f64> {
    let mut seed = seed;
    let (mut next, mut held) = (0.0, truth(0.0));
    (0..frames)
        .map(|f| {
            while next <= f as f64 {
                held = truth(next);
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                next += 0.4 + 1.6 * (seed >> 8) as f64 / (1 << 24) as f64;
            }
            held
        })
        .collect()
}

/// Put a take on the timeline the way the end of a record pass does: the raw samples beside
/// the song, and the keys fitted to them punched in as a recorded clip. Returns the clip's id.
fn take(
    t: &TestServer,
    project: &Project,
    song: &mut Song,
    camera: &str,
    start: i64,
    samples: BTreeMap<Axis, Vec<f64>>,
) -> String {
    let passes = t.dir.join("songs").join(&song.id).join("passes");
    std::fs::create_dir_all(&passes).unwrap();
    let id = format!("pass-{:04}", std::fs::read_dir(&passes).unwrap().count() + 1);
    let frames = samples.values().next().unwrap().len() as i64;
    let pass = RecordedPass {
        id: id.clone(),
        song_id: song.id.clone(),
        camera: camera.to_string(),
        recorded_at: "2026-11-16T16:23:23Z".into(),
        rate: project.rate,
        start_frame: start,
        query_delay_s: 0.02,
        samples,
    };
    std::fs::write(passes.join(format!("{id}.json")), serde_json::to_vec_pretty(&pass).unwrap()).unwrap();
    let (next, clip) =
        edit::punch_keys(song, project, camera, start, frames - 1, fit_pass(&pass.samples, 1.0), Some(id)).unwrap();
    *song = next;
    clip
}

fn put(api: &Api, song: &Song) -> WorkingSong {
    let counter = api.working(&song.id).unwrap().edit_counter;
    api.put_working(&PutWorking { expected_counter: counter, song: song.clone() }).unwrap()
}

fn smooth(api: &Api, camera: Option<&str>) -> Smoothed {
    api.post("/record/smooth", &Smooth { camera: camera.map(str::to_string) }).unwrap()
}

fn two_cameras(api: &Api) -> Project {
    let mut project = api.project().unwrap();
    project.cameras.push(CameraConfig::new("cam2", "Camera 2"));
    api.put_project(&project).unwrap()
}

/// The shape of a real session: two heads recorded together from a few seconds into the song,
/// each already off home, each take read late and unevenly from the head.
#[test]
fn takes_read_late_from_real_heads_are_smoothed_inside_the_limits_and_the_show_arms_on_them() {
    let t = TestServer::start("smooth-takes");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let project = two_cameras(&api);
    let fps = project.rate.fps();
    let mut song = api.new_song("Here You Come", None).unwrap().song;
    api.load_song(&song.id).unwrap();
    song = edit::set_song_length(&song, &project, 700).unwrap();

    // Each head holds, pans 40 degrees in two seconds, holds: well inside 60 degrees a second.
    let pan = |from: f64, by: f64| move |f: f64| from + by * ease_value(Ease::EaseInOut, (f - 200.0) / (2.0 * fps));
    let tilt = |f: f64| -1.1 + 6.0 * ease_value(Ease::EaseInOut, (f - 320.0) / (3.0 * fps));
    let first = take(
        &t,
        &project,
        &mut song,
        "cam1",
        34,
        [(Axis::Pan, late_readings(7, 600, pan(-13.3, -40.0))), (Axis::Tilt, late_readings(11, 600, tilt))].into(),
    );
    let second = take(
        &t,
        &project,
        &mut song,
        "cam2",
        35,
        [(Axis::Pan, late_readings(3, 600, pan(22.5, 30.0))), (Axis::Tilt, vec![0.37; 600])].into(),
    );
    let recorded = put(&api, &song);
    api.save(&song.id).unwrap();
    let raw = |n: u32| std::fs::read(t.dir.join("songs").join(&song.id).join(format!("passes/pass-{n:04}.json"))).unwrap();
    let raw_before = (raw(1), raw(2));

    // Before: nothing about getting to the first pose, only speed and sharpness, on both heads.
    assert!(!recorded.issues.is_empty());
    assert!(recorded.issues.iter().all(|i| i.acceptable()), "{:?}", recorded.issues);
    assert!(recorded.issues.iter().any(|i| matches!(i, Issue::TooFast { clip, axis: Axis::Pan, .. } if *clip == first)));
    assert!(recorded.issues.iter().any(|i| i.clip_id() == Some(second.as_str())));
    let blocking = api.status().unwrap().song.unwrap().blocking;
    assert_eq!(blocking, recorded.issues.len());

    let done = smooth(&api, None);
    let report = &done.report;
    assert_eq!((report.moves, report.blocking_before), (2, blocking));
    assert_eq!(report.smoothed, vec![first.clone(), second.clone()]);
    assert!(report.accepted.is_empty() && report.left.is_empty(), "{report:?}");
    assert!(done.working.issues.is_empty(), "{:?}", done.working.issues);
    assert!(report.departure[&Axis::Pan] > 0.1 && report.departure[&Axis::Pan] < 2.5, "{:?}", report.departure);
    assert!(report.text(project.units).starts_with("Smoothed 2 recorded moves to the rig's limits, by at most pan "));
    assert!(report.text(project.units).ends_with("No issue stops Arm"));

    // The moves are where they were: the same frames, the same ends, close to the take between.
    let smoothed = &done.working.song;
    for (clip, camera) in [(&first, "cam1"), (&second, "cam2")] {
        let (was, now) = (song.motion_clip(clip).unwrap(), smoothed.motion_clip(clip).unwrap());
        assert_eq!(
            (now.start_frame, now.duration_frames, &now.recorded_pass),
            (was.start_frame, was.duration_frames, &was.recorded_pass)
        );
        assert!(now.accepted.is_none());
        for f in [0, was.start_frame, was.end_frame(), 700] {
            assert_eq!(pose_at(smoothed, camera, f), pose_at(&song, camera, f), "{camera} frame {f}");
        }
        for f in 0..=700 {
            let off = pose_at(smoothed, camera, f).pan - pose_at(&song, camera, f).pan;
            assert!(off.abs() <= report.departure[&Axis::Pan] + 0.06, "{camera} frame {f} is {off} from the take");
        }
    }
    // The second head's tilt never moved: its keys are the ones it had.
    assert_eq!(smoothed.motion_clip(&second).unwrap().keys(Axis::Tilt), song.motion_clip(&second).unwrap().keys(Axis::Tilt));
    // Nothing else was touched: the raw samples, and the limits the moves were fitted to.
    assert_eq!((raw(1), raw(2)), raw_before);
    assert_eq!(api.project().unwrap(), project);

    // Again changes nothing, and Revert still has the take as it was recorded.
    let again = smooth(&api, None);
    assert_eq!((again.working.edit_counter, &again.working.song), (done.working.edit_counter, smoothed));
    assert!(again.report.smoothed.is_empty() && again.report.text(project.units).starts_with("No recorded move is over"));
    assert_eq!(api.revert(&song.id).unwrap().song, song);
    assert_eq!(smooth(&api, None).working.song, *smoothed, "smoothing from the raw samples gives the same moves each time");
    let log = api.get::<Vec<LogRecord>>("/log?since=0").unwrap();
    let said = log.iter().rfind(|r| r.kind == "record_smooth").expect("the run log says the song was smoothed");
    assert_eq!((said.data["by"].as_str(), said.data["left"].as_u64()), (Some("Luke-MacBook"), Some(0)));

    // The show arms, and Arm takes each head to where its take starts.
    api.save(&song.id).unwrap();
    api.commit(&song.id).unwrap();
    do_act(&api, Action::Arm);
    let armed = wait_for(&api, |st| st.transport.output == Output::Armed);
    for (camera, pan) in [("cam1", -13.3), ("cam2", 22.5)] {
        let at = armed.camera(camera).unwrap().rig_pose;
        assert!((at.pan - pan).abs() < 0.5, "{camera} stands at {} before its take, which starts at {pan}", at.pan);
    }
}

#[test]
fn a_move_still_over_a_limit_is_accepted_and_a_step_or_a_range_problem_is_reported_not_accepted() {
    let t = TestServer::start("smooth-left");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let mut project = two_cameras(&api);
    // The second head cannot pan past 50 degrees.
    project.cameras[1].axes.pan.max = 50.0;
    let project = api.put_project(&project).unwrap();
    let mut song = api.new_song("Hard Takes", None).unwrap().song;
    api.load_song(&song.id).unwrap();
    song = edit::set_song_length(&song, &project, 900).unwrap();

    // Two and a half seconds at one and a half times the head's top speed, in a three second
    // take: there is no room in it to make that move any slower, so smoothing cannot hide it.
    let sweep = |f: f64| -80.0 + 90.0 * ((f - 10.0) / 25.0).clamp(0.0, 2.5);
    let fast = take(&t, &project, &mut song, "cam1", 10, [(Axis::Pan, (0..90).map(|f| sweep(f as f64)).collect())].into());
    // A later take on the same head starts 45 degrees from where the first ended.
    let later =
        take(&t, &project, &mut song, "cam1", 500, [(Axis::Pan, late_readings(5, 200, |f| 100.0 - 0.8 * f.min(100.0)))].into());
    // The same sweep on the head that cannot reach it.
    let beyond = take(&t, &project, &mut song, "cam2", 10, [(Axis::Pan, (0..90).map(|f| sweep(f as f64)).collect())].into());
    let recorded = put(&api, &song);
    let of = |issues: &[Issue], clip: &str| issues.iter().filter(|i| i.clip_id() == Some(clip)).cloned().collect::<Vec<_>>();
    assert!(of(&recorded.issues, &later).iter().any(|i| i.acceptable()), "the late readings make the later take too sharp");

    // One camera's moves only.
    let done = smooth(&api, Some("cam2"));
    assert_eq!((done.report.moves, done.report.accepted.len()), (1, 0));
    assert!(done.working.song.motion_clip(&fast).unwrap().accepted.is_none());
    assert_eq!(done.working.song.motion_clip(&fast), song.motion_clip(&fast), "the first camera's take is as it was");
    assert!(refused(api.post::<_, Smoothed>("/record/smooth", &Smooth { camera: Some("cam9".into()) }), 404)
        .contains("no camera cam9"));

    let done = smooth(&api, None);
    let (report, after) = (&done.report, &done.working.song);
    assert_eq!(report.moves, 3);
    // Too fast for too long: smoothed at its corners, still too fast, and accepted in the lease holder's name.
    assert_eq!(report.accepted, vec![fast.clone()]);
    assert!(matches!(&of(&done.working.issues, &fast)[..], [Issue::TooFast { axis: Axis::Pan, .. }]));
    assert_eq!(after.motion_clip(&fast).unwrap().accepted.as_ref().map(|a| a.by.as_str()), Some("Luke-MacBook"));
    // A step in the middle of the song: the take is smoothed, the step is left for the operator.
    assert!(report.smoothed.contains(&later));
    assert!(matches!(&of(&done.working.issues, &later)[..], [Issue::Jump { frame: 500, .. }]));
    assert!(after.motion_clip(&later).unwrap().accepted.is_none());
    // Outside the head's travel: nothing about that move is accepted, its speed included.
    assert!(of(&done.working.issues, &beyond).iter().any(|i| matches!(i, Issue::OutOfRange { .. })));
    assert!(after.motion_clip(&beyond).unwrap().accepted.is_none());
    assert!(report.left.iter().any(|i| matches!(i, Issue::Jump { .. })));
    assert!(report.left.iter().any(|i| matches!(i, Issue::OutOfRange { .. })));
    assert!(report.left.iter().any(|i| matches!(i, Issue::TooFast { clip, .. } if *clip == beyond)));
    assert!(report.left.iter().all(|i| i.clip_id() != Some(fast.as_str())));
    let text = report.text(project.units);
    assert!(text.contains("Accepted 1 move still over a limit") && text.ends_with("3 issues are left to fix"), "{text}");
    assert_eq!(api.project().unwrap(), project, "no limit was raised to make an issue go away");

    // The acceptance is in the run log like any other, and Arm is still refused.
    let log = api.get::<Vec<LogRecord>>("/log?since=0").unwrap();
    let accepted = log.iter().find(|r| r.kind == "issue_accepted").expect("the acceptance is logged");
    assert_eq!((accepted.data["clip"].as_str(), accepted.data["by"].as_str()), (Some(fast.as_str()), Some("Luke-MacBook")));
    assert!(accepted.data["issues"][0].as_str().unwrap().contains("too fast"));
    api.save(&song.id).unwrap();
    api.commit(&song.id).unwrap();
    let why = refused(api.act(Action::Arm, &TransportRequest::default()), 409);
    assert!(why.contains("Fix or accept 3 timeline issues first"), "{why}");

    // A move that was accepted is left alone the next time.
    let again = smooth(&api, None);
    assert_eq!(again.working.song, *after);
    assert!(again.report.accepted.is_empty() && again.report.smoothed.is_empty());

    // A song with nothing recorded has nothing to smooth.
    api.load_song("walk-in").unwrap();
    assert!(refused(api.post::<_, Smoothed>("/record/smooth", &Smooth::default()), 409).contains("no recorded moves"));
}

/// Every axis is put right the same way: a take that drives all five, each read late and
/// each starting away from home, is clear to arm after one smooth.
#[test]
fn a_take_on_every_axis_is_smoothed_on_every_axis_and_waits_where_it_starts() {
    let t = TestServer::start("smooth-every-axis");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let project = api.project().unwrap();
    let fps = project.rate.fps();
    let mut song = api.new_song("Every Axis", None).unwrap().song;
    api.load_song(&song.id).unwrap();
    song = edit::set_song_length(&song, &project, 700).unwrap();

    // On each axis: hold a third of the way up its travel, make a two second move the rig
    // can easily do, hold.
    let limits = project.cameras[0].axes;
    let mut samples = BTreeMap::new();
    let mut starts = Vec::new();
    for (n, axis) in Axis::ALL.into_iter().enumerate() {
        let l = *limits.get(axis);
        let from = l.min + 0.3 * (l.max - l.min);
        // The column is slow enough that only a brisk move shows the late readings.
        let share = if axis == Axis::Elevation { 0.9 } else { 0.5 };
        let by = (share * l.max_speed).min(0.4 * share * l.max_accel).min(0.4 * (l.max - l.min));
        let truth = move |f: f64| from + by * ease_value(Ease::EaseInOut, (f - 200.0) / (2.0 * fps));
        samples.insert(axis, late_readings(3 + n as u32, 600, truth));
        starts.push((axis, from, l.max - l.min));
    }
    let clip = take(&t, &project, &mut song, "cam1", 40, samples);
    let recorded = put(&api, &song);
    assert!(!recorded.issues.is_empty() && recorded.issues.iter().all(|i| i.acceptable()), "{:?}", recorded.issues);
    // Which axes the late readings put over a limit, measured the way the compile measures it.
    let was = song.motion_clip(&clip).unwrap();
    let over: Vec<Axis> = Axis::ALL
        .into_iter()
        .filter(|axis| {
            let plays: Vec<f64> = (0..=was.duration_frames).filter_map(|f| keys_value(was.keys(*axis), f as f64)).collect();
            overruns(&plays, limits.get(*axis), fps)
        })
        .collect();
    assert_eq!(over, Axis::ALL, "the late readings put every axis over a limit");

    let done = smooth(&api, None);
    assert!(done.working.issues.is_empty(), "{:?}", done.working.issues);
    assert!(done.report.accepted.is_empty() && done.report.left.is_empty(), "{:?}", done.report);
    // Each axis that was over a limit was smoothed, and one that was not is as it was recorded.
    let now = done.working.song.motion_clip(&clip).unwrap();
    for axis in Axis::ALL {
        assert_eq!(done.report.departure.contains_key(&axis), over.contains(&axis), "{}", axis.label());
        assert_eq!(now.keys(axis) == was.keys(axis), !over.contains(&axis), "{}", axis.label());
    }
    // Before the take the camera waits where the take starts, on every axis.
    let waits = pose_at(&done.working.song, "cam1", 0);
    for (axis, from, travel) in starts {
        assert!((waits.get(axis) - from).abs() < 1e-6 * travel, "{} waits at {}, not {from}", axis.label(), waits.get(axis));
    }
    assert_eq!(api.project().unwrap(), project);
}
