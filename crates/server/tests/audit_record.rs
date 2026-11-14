//! Audit: Record passes.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::Api;
use ptz_core::eval::pose_at;
use ptz_core::pose::{Axis, RigKind};
use ptz_core::song::{ClipBody, Song};
use ptz_core::transport::Action;
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::time::Duration;

const CAM: &str = ptz_core::FIRST_CAMERA;

fn record_on(api: &Api, axes: &[Axis]) -> Status {
    api.act(Action::RecordOn, &TransportRequest { axes: Some(axes.to_vec()), ..Default::default() }).unwrap()
}

fn log_of(api: &Api, prefix: &str) -> Vec<String> {
    api.get::<Vec<LogRecord>>("/log?since=0")
        .unwrap()
        .into_iter()
        .filter(|r| r.kind.starts_with(prefix))
        .map(|r| format!("{} {}", r.kind, r.data))
        .collect()
}

#[test]
fn a_pass_from_the_top_of_the_song_on_a_rig_with_a_query_delay_is_kept() {
    let t = TestServer::start("audit-record-delay");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    let mut project = api.project().unwrap();
    project.cameras[0].rig.kind = RigKind::Hardware;
    project.cameras[0].rig.fr7_host = fr7.visca.to_string();
    project.cameras[0].rig.fr7_http_port = fr7.http.port();
    project.cameras[0].rig.fr7_user = fr7.user.clone();
    project.cameras[0].rig.fr7_password = fr7.password.clone();
    project.cameras[0].rig.autopod_host = pod.addr.to_string();
    project.cameras[0].rig.autopod_calibration = FakeAutoPod::height_map();
    // What bring-up measured for this head: a position is 100 ms old when it arrives (2.5 frames at 25 fps).
    project.cameras[0].rig.query_delay_ms = 100.0;
    api.put_project(&project).unwrap();
    wait_for(&api, |s| s.first().adapters.len() == 2 && s.first().adapters.iter().all(|a| a.connected));

    let id = api.new_song("Top", None).unwrap().song.id;
    api.load_song(&id).unwrap();
    record_on(&api, &[Axis::Pan]);
    do_act(&api, Action::Play);
    // The operator turns the head with the camera's own controller while the song plays.
    for step in 0..60 {
        fr7.place(step as f64 * 0.2, 0.0, 0.0, 0.5);
        std::thread::sleep(Duration::from_millis(20));
    }
    do_act(&api, Action::Stop);

    let w = api.working(&id).unwrap();
    let recorded: Vec<_> = w.song.motion.iter().filter(|c| matches!(c.body, ClipBody::Keys { .. })).collect();
    let valid = w.song.validate(project.rate);
    assert!(
        recorded.len() == 1 && valid.is_ok(),
        "after the pass the song has {} recorded clips (start frames {:?}) and validates as {valid:?}; the log says {:?}",
        recorded.len(),
        recorded.iter().map(|c| c.start_frame).collect::<Vec<_>>(),
        log_of(&api, "record_")
    );
    // And the song can still be edited and saved.
    let put = api.put_working(&PutWorking { expected_counter: w.edit_counter, song: w.song.clone() });
    assert!(put.is_ok(), "{put:?}");
}

#[test]
fn moving_the_playhead_during_a_pass_does_not_overwrite_what_lies_between() {
    let t = TestServer::start("audit-record-seek");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let before: Song = api.working("walk-in").unwrap().song;
    let moves_between = (300..900).filter(|f| pose_at(&before, CAM, *f).pan != pose_at(&before, CAM, 300).pan).count();
    assert!(moves_between > 0, "the example song pans somewhere between frames 300 and 900");

    record_on(&api, &[Axis::Pan]);
    do_act(&api, Action::Play);
    std::thread::sleep(Duration::from_millis(400));
    // The operator clicks further along the ruler while the pass is running.
    let seek = api.act(Action::Seek, &TransportRequest { frame: Some(1000), ..Default::default() });
    std::thread::sleep(Duration::from_millis(400));
    do_act(&api, Action::Stop);

    let after: Song = api.working("walk-in").unwrap().song;
    let changed: Vec<i64> =
        (300..900).filter(|f| (pose_at(&before, CAM, *f).pan - pose_at(&after, CAM, *f).pan).abs() > 1e-6).collect();
    assert!(
        changed.is_empty(),
        "Seek during the pass answered {:?}; the playhead was never on frames 300 to 900, yet pan changed on {} of them \
         (frame 600 was {:.2}, is now {:.2}); the log says {:?}",
        seek.as_ref().map(|s| s.frame),
        changed.len(),
        pose_at(&before, CAM, 600).pan,
        pose_at(&after, CAM, 600).pan,
        log_of(&api, "record_")
    );
}

#[test]
fn estop_in_the_middle_of_a_pass_keeps_the_pass() {
    let t = TestServer::start("audit-record-estop");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let id = api.new_song("Estop", None).unwrap().song.id;
    api.load_song(&id).unwrap();
    record_on(&api, &[Axis::Pan, Axis::Tilt]);
    do_act(&api, Action::Play);
    for _ in 0..40 {
        api.jog(CAM, &Jog { pan: 0.2, tilt: 0.1, ..Jog::default() }).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    let at = api.status().unwrap();
    api.post_empty::<Status>("/estop").unwrap();

    let song = api.working(&id).unwrap().song;
    assert_eq!(song.motion.len(), 1, "{:?}", log_of(&api, "record_"));
    let clip = &song.motion[0];
    assert_eq!((clip.camera.as_str(), clip.axes.clone(), clip.start_frame), (CAM, vec![Axis::Pan, Axis::Tilt], 0));
    assert!(clip.duration_frames >= at.frame - 2, "{} frames kept of {}", clip.duration_frames, at.frame);
    let end = pose_at(&song, CAM, clip.duration_frames);
    assert!((end.pan - at.first().rig_pose.pan).abs() < 1.0, "the pass ends where the rig was: {end:?}");
    let pass = clip.recorded_pass.clone().expect("raw samples kept");
    assert!(t.dir.join("songs").join(&id).join("passes").join(format!("{pass}.json")).is_file());
    println!(
        "MEASURE E-Stop mid-pass kept {} frames of {} with {} pan keys",
        clip.duration_frames,
        at.frame,
        clip.keys(Axis::Pan).len()
    );
}
