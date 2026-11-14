//! Record over real HTTP: a hand-driven move on the simulator becomes keys on
//! the frames it happened on, replaces only what it covered, and can be refitted.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::{Api, ApiErr};
use ptz_core::eval::{pose_at, pose_at_f};
use ptz_core::pose::Axis;
use ptz_core::song::Song;
use ptz_core::transport::{Action, Run};
use std::time::{Duration, Instant};

const CAM: &str = ptz_core::FIRST_CAMERA;

fn refusal(result: Result<impl std::fmt::Debug, ApiErr>, code: u16) -> String {
    match result {
        Err(ApiErr::Refused { code: got, msg }) if got == code => msg,
        other => panic!("expected a {code} refusal, got {other:?}"),
    }
}

fn act(api: &Api, action: Action) -> Status {
    api.act(action, &TransportRequest::default()).unwrap()
}

fn record_on(api: &Api, axes: &[Axis]) -> Result<Status, ApiErr> {
    api.act(Action::RecordOn, &TransportRequest { axes: Some(axes.to_vec()), ..Default::default() })
}

fn working(api: &Api, id: &str) -> Song {
    api.working(id).unwrap().song
}

/// Value of `axis` at `frame`, read off (frame, value) points by straight lines.
fn lookup(points: &[(f64, f64)], frame: f64) -> Option<f64> {
    let i = points.partition_point(|p| p.0 <= frame);
    if i == 0 || i == points.len() {
        return None;
    }
    let (a, b) = (points[i - 1], points[i]);
    Some(a.1 + (b.1 - a.1) * (frame - a.0) / (b.0 - a.0).max(1e-9))
}

#[test]
fn a_jogged_pan_is_recorded_and_plays_back_on_the_frames_it_was_recorded_at() {
    let t = TestServer::start("record-pan");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let song_id = api.new_song("Record Test", None).unwrap().song.id;
    api.load_song(&song_id).unwrap();
    let project = api.project().unwrap();
    let fps = project.rate.fps();

    assert!(refusal(record_on(&api, &[]), 409).contains("Record-enable at least one axis lane"));
    let st = record_on(&api, &[Axis::Pan]).unwrap();
    assert!(st.transport.record && !st.record.active, "Record alone captures nothing until Play");
    assert_eq!(st.record.axes, vec![Axis::Pan]);
    act(&api, Action::Play);

    // The scripted move: pan right at 10 degrees per second from frame 100 to frame 200.
    let jog = Jog { pan: 10.0 / project.cameras[0].axes.get(Axis::Pan).max_speed, tilt: 0.5, ..Jog::default() };
    let mut seen: Vec<(f64, f64)> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let st = api.status().unwrap();
        assert!(Instant::now() < deadline, "the pass never reached frame 240: {st:?}");
        assert_eq!(st.transport.run, Run::Playing);
        assert!(st.record.active);
        assert_eq!(st.first().pose_source, PoseSource::Record);
        assert_eq!(st.first().rig_pose.tilt, 0.0, "tilt is not record-enabled, so the controller does not move it");
        if seen.last().is_none_or(|l| st.frame_f > l.0) {
            seen.push((st.frame_f, st.first().rig_pose.pan));
        }
        if st.frame_f >= 240.0 {
            break;
        }
        if (100.0..200.0).contains(&st.frame_f) {
            api.jog(CAM, &jog).unwrap();
        } else {
            api.jog(CAM, &Jog::default()).unwrap();
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    let frames_seen = api.status().unwrap().record.frames;
    assert!(frames_seen >= 240, "{frames_seen} frames captured");
    let st = act(&api, Action::Stop);
    assert!(!st.transport.record && !st.record.active);

    // The pass is on the timeline as one recorded keys clip on pan only.
    let song = working(&api, &song_id);
    assert_eq!(song.motion.len(), 1);
    let clip = &song.motion[0];
    assert_eq!(clip.axes, vec![Axis::Pan]);
    assert_eq!(clip.start_frame, 0);
    assert!(clip.duration_frames >= 239);
    let pass_id = clip.recorded_pass.clone().expect("the clip names its raw samples");
    let issues = api.status().unwrap().song.map(|s| s.issues);
    assert_eq!(issues, Some(0), "a move the rig made by hand passes the feasibility check");
    let keys = clip.keys(Axis::Pan).len();
    assert!((2..=12).contains(&keys), "a still, a ramp and a still should be a few keys, not {keys}");
    let pass: serde_json::Value = read_json(&t.dir.join("songs").join(&song_id).join("passes").join(format!("{pass_id}.json")));
    assert_eq!(pass["start_frame"], 0);
    assert_eq!(pass["samples"]["pan"].as_array().unwrap().len() as i64, clip.duration_frames + 1);

    // Every reading taken during the pass is on the curve.
    assert!(seen.len() > 200, "{} readings", seen.len());
    for (frame, pan) in &seen {
        let played = pose_at_f(&song, CAM, *frame).pan;
        assert!((played - pan).abs() < 0.3, "frame {frame}: the rig was at {pan}, the timeline plays {played}");
    }
    // And it is the scripted move: still, then 10 degrees per second, then still.
    assert_eq!(pose_at(&song, CAM, 90).pan, 0.0);
    let per_second = pose_at(&song, CAM, 150 + fps as i64).pan - pose_at(&song, CAM, 150).pan;
    assert!((per_second - 10.0).abs() < 0.3, "{per_second} degrees per second");
    let travelled = pose_at(&song, CAM, 230).pan;
    assert!((travelled - 40.0).abs() < 2.5, "100 frames at 10 degrees per second is 40 degrees, not {travelled}");

    // Play it back: the rig is where it was on each frame of the pass.
    act(&api, Action::Cue);
    act(&api, Action::Play);
    let mut compared = 0;
    loop {
        let st = api.status().unwrap();
        assert_eq!(st.first().pose_source, PoseSource::Timeline);
        if st.frame_f >= 235.0 {
            break;
        }
        if let Some(recorded) = lookup(&seen, st.frame_f) {
            assert!(
                (st.first().rig_pose.pan - recorded).abs() < 0.3,
                "frame {}: recorded at {recorded}, played back at {}",
                st.frame_f,
                st.first().rig_pose.pan
            );
            compared += 1;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    assert!(compared > 200, "{compared} frames compared");
    act(&api, Action::Stop);

    // Refit from the raw samples: looser gives no more keys, tighter no fewer, the move stays put.
    let clip_id = clip.id.clone();
    let loose = api.post::<_, WorkingSong>(&format!("/record/{clip_id}/refit"), &Refit { scale: 20.0 }).unwrap().song;
    let tight = api.post::<_, WorkingSong>(&format!("/record/{clip_id}/refit"), &Refit { scale: 0.1 }).unwrap().song;
    let count = |s: &Song| s.motion_clip(&clip_id).unwrap().keys(Axis::Pan).len();
    assert!(count(&loose) <= keys && keys <= count(&tight), "{} / {keys} / {}", count(&loose), count(&tight));
    for f in 0..240 {
        assert!((pose_at(&tight, CAM, f).pan - pose_at(&song, CAM, f).pan).abs() < 0.06, "refit moved frame {f}");
        assert!((pose_at(&loose, CAM, f).pan - pose_at(&song, CAM, f).pan).abs() < 1.1, "loose refit moved frame {f}");
    }
    let log = api.get::<Vec<LogRecord>>("/log?since=0").unwrap();
    for kind in ["record_start", "record_pass", "record_refit"] {
        assert!(log.iter().any(|r| r.kind == kind), "the run log has no {kind} record");
    }
}

#[test]
fn a_pass_over_the_middle_of_a_move_replaces_only_its_axis_and_range() {
    let t = TestServer::start("record-punch");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let before = working(&api, "walk-in");
    // The second Walk-In move drives all five axes from frame 300.
    let target = before.motion.iter().find(|c| c.start_frame == 300).expect("the move at 12 s").clone();
    assert_eq!(target.axes.len(), 5);
    assert!(target.duration_frames > 60, "{}", target.duration_frames);
    let not_recorded = target.id.clone();
    assert!(refusal(api.post::<_, WorkingSong>(&format!("/record/{not_recorded}/refit"), &Refit { scale: 1.0 }), 409)
        .contains("was not recorded"));

    api.act(Action::Seek, &TransportRequest { frame: Some(310), ..Default::default() }).unwrap();
    record_on(&api, &[Axis::Tilt]).unwrap();
    act(&api, Action::Play);
    let seek = api.act(Action::Seek, &TransportRequest { frame: Some(0), ..Default::default() });
    assert!(refusal(seek, 409).contains("Stop to move the playhead while recording"));
    let mut end = 0.0;
    while end < 340.0 {
        let st = api.status().unwrap();
        end = st.frame_f;
        api.jog(CAM, &Jog { tilt: 0.25, zoom: 1.0, ..Jog::default() }).unwrap();
        std::thread::sleep(Duration::from_millis(8));
    }
    // Turning Record off ends the pass; the song keeps playing.
    let st = act(&api, Action::RecordOff);
    assert_eq!(st.transport.run, Run::Playing);
    assert!(!st.record.active);
    act(&api, Action::Stop);

    let after = working(&api, "walk-in");
    let recorded = after.motion.iter().find(|c| c.recorded_pass.is_some()).expect("a recorded clip");
    assert_eq!(recorded.axes, vec![Axis::Tilt]);
    let (start, stop) = (recorded.start_frame, recorded.end_frame());
    assert!((310..=312).contains(&start) && stop >= 340 && stop < target.end_frame(), "{start}..{stop}");
    after.validate(api.project().unwrap().rate).unwrap();

    for f in 0..=before.length_frames {
        let (o, n) = (pose_at(&before, CAM, f), pose_at(&after, CAM, f));
        for axis in [Axis::Pan, Axis::Zoom, Axis::Focus, Axis::Elevation] {
            assert!((o.get(axis) - n.get(axis)).abs() < 1e-6, "{axis:?} changed at frame {f}");
        }
        if !(start..stop).contains(&f) {
            assert!((o.tilt - n.tilt).abs() < 1e-6, "tilt changed outside the pass at frame {f}: {} then {}", o.tilt, n.tilt);
        }
    }
    // Inside the pass tilt is what the hand did: it rose from where the rig was.
    let rise = pose_at(&after, CAM, stop - 1).tilt - pose_at(&after, CAM, start).tilt;
    assert!(rise > 2.0, "tilt rose {rise} degrees");
}

#[test]
fn the_rig_stops_within_250_ms_of_the_last_jog_message() {
    let t = TestServer::start("jog-watchdog");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let start = api.status().unwrap().first().rig_pose.pan;
    let sent = Instant::now();
    api.jog(CAM, &Jog { pan: 1.0, ..Jog::default() }).unwrap();
    std::thread::sleep(Duration::from_millis(120));
    let moving = api.status().unwrap();
    assert!(moving.first().rig_pose.pan > start + 0.2, "the rig moves while jog is fresh");
    assert_eq!(moving.first().pose_source, PoseSource::Rig);

    // No more jog messages: a dropped connection looks exactly like this.
    std::thread::sleep(Duration::from_millis(300).saturating_sub(sent.elapsed()));
    let stopped = api.status().unwrap().first().rig_pose.pan;
    std::thread::sleep(Duration::from_millis(300));
    let later = api.status().unwrap().first().rig_pose.pan;
    assert_eq!(stopped, later, "the rig is still {} ms after the last jog", sent.elapsed().as_millis());
    // 60 degrees per second for at most 250 ms and one tick.
    let top = api.project().unwrap().cameras[0].axes.get(Axis::Pan).max_speed;
    assert!(later - start <= top * 0.30, "travelled {} degrees", later - start);

    // Jog is refused while armed and while the E-Stop is latched.
    api.post_empty::<Status>("/estop").unwrap();
    assert!(refusal(api.jog(CAM, &Jog { pan: 1.0, ..Jog::default() }), 409).contains("Reset E-Stop"));
    api.post_empty::<Status>("/estop/reset").unwrap();
    act(&api, Action::Arm);
    assert!(refusal(api.jog(CAM, &Jog { pan: 1.0, ..Jog::default() }), 409).contains("Disarm"));
}
