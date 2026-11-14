//! Arm and live output over real HTTP, with the FR7 and AutoPod+ protocol fakes
//! standing in for the rig.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::Api;
use ptz_core::edit;
use ptz_core::pose::{Axis, Cmp, KeepOutRule, Pose, Preset, Project, RigKind};
use ptz_core::transport::{Action, Output, Run, TimeMode};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7, FAKE_PICTURE};
use ptz_server::hw::visca;
use ptz_server::runlog;
use std::time::{Duration, Instant};

const CAM: &str = ptz_core::FIRST_CAMERA;

struct Rig {
    fr7: FakeFr7,
    pod: FakeAutoPod,
}

impl Rig {
    fn start() -> Rig {
        Rig { fr7: FakeFr7::start(), pod: FakeAutoPod::start() }
    }

    /// Point the project at the fakes and wait until both answer.
    fn connect(&self, api: &Api) -> Project {
        let mut project = api.project().unwrap();
        project.cameras[0].rig.kind = RigKind::Hardware;
        project.cameras[0].rig.fr7_host = self.fr7.visca.to_string();
        project.cameras[0].rig.fr7_http_port = self.fr7.http.port();
        project.cameras[0].rig.fr7_user = self.fr7.user.clone();
        project.cameras[0].rig.fr7_password = self.fr7.password.clone();
        project.cameras[0].rig.autopod_host = self.pod.addr.to_string();
        project.cameras[0].rig.autopod_calibration = FakeAutoPod::height_map();
        let project = api.put_project(&project).unwrap();
        let st = wait_for(api, |s| {
            s.first().adapters.len() == 2
                && s.first().adapters.iter().all(|a| a.connected)
                && s.first().adapters[1].calibrated == Some(true)
        });
        assert!(st.telemetry.hardware);
        project
    }

    fn pose(&self) -> Pose {
        let (pan, tilt, zoom, focus) = self.fr7.pose();
        Pose { pan, tilt, zoom, focus, elevation: self.pod.height_mm() }
    }

    fn is_moving(&self) -> bool {
        self.fr7.is_moving() || self.pod.is_moving()
    }

    /// Motion commands the camera received: everything except inquiries.
    fn fr7_stops(&self) -> usize {
        self.fr7.commands().iter().filter(|c| **c == visca::pan_tilt_stop()).count()
    }
}

fn store(api: &Api, name: &str, pose: Pose) -> Preset {
    api.post("/presets", &StorePreset { name: name.into(), pose: Some(pose), camera: None }).unwrap()
}

/// A saved, committed song with one move to `preset` starting at frame 25.
fn song_with_move(api: &Api, project: &Project, name: &str, preset: &Preset) -> (String, String) {
    let created = api.new_song(name, None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let (song, clip) = edit::insert_preset_move(&created.song, project, preset, 25).unwrap();
    api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    (id, clip)
}

fn close(a: &Pose, b: &Pose, tol: &Pose, share: f64) -> bool {
    Axis::ALL.iter().all(|x| (a.get(*x) - b.get(*x)).abs() <= tol.get(*x) * share)
}

fn log_kinds(api: &Api) -> Vec<LogRecord> {
    api.get("/log?since=0").unwrap()
}

fn seek(api: &Api, frame: i64) -> Status {
    api.act(Action::Seek, &TransportRequest { frame: Some(frame), ..Default::default() }).unwrap()
}

#[test]
fn a_committed_song_arms_positions_plays_live_within_tolerance_and_estop_halts_it() {
    let t = TestServer::start("live-flow");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    rig.fr7.place(8.0, -3.0, 0.1, 0.5);
    rig.pod.place(30.0);
    let project = rig.connect(&api);
    let tol = project.cameras[0].rig.tolerance;

    let mark = store(&api, "Downstage", Pose { pan: 45.0, tilt: 10.0, zoom: 0.4, focus: 0.4, elevation: 60.0 });
    let (song_id, clip) = song_with_move(&api, &project, "Live Test", &mark);
    let song = api.working(&song_id).unwrap().song;
    let end = {
        let c = song.motion_clip(&clip).unwrap();
        c.start_frame + c.duration_frames
    };
    let revision = api.get::<Vec<RevisionInfo>>(&format!("/songs/{song_id}/revisions")).unwrap().pop().unwrap();

    // Nothing has been sent to the rig yet.
    assert!(rig.fr7.commands().is_empty());
    let st = api.status().unwrap();
    assert!(!st.hardware_live);
    assert!(close(&st.first().rig_pose, &rig.pose(), &tol, 0.1), "the server reads where the rig is: {:?}", st.first().rig_pose);

    // Arm moves the rig to the pose at the playhead, slowly, and only then is it armed.
    let st = do_act(&api, Action::Arm);
    assert_eq!(st.transport.output, Output::Positioning);
    assert!(st.hardware_live);
    assert_eq!(st.guidance.primary_label, "Disarm");
    assert_eq!(
        refused(api.act(Action::Play, &TransportRequest::default()), 409),
        "The rig is still moving to the playhead. Wait, or Disarm"
    );
    let began = Instant::now();
    let st = wait_for(&api, |s| s.transport.output == Output::Armed);
    assert!(began.elapsed() > Duration::from_millis(600), "30 mm at half of 43 mm/s takes over a second");
    assert!(close(&rig.pose(), &Pose::home(), &tol, 0.3), "positioned to the first frame: {:?}", rig.pose());
    assert_eq!(st.guidance.primary_label, "Play Live");
    assert_eq!(st.first().pose_source, PoseSource::Rig);

    let arm = log_kinds(&api).into_iter().rfind(|r| r.kind == "arm").expect("Arm is logged");
    assert_eq!(arm.data["song"], song_id.as_str());
    assert_eq!(arm.data["revision"], 1);
    assert_eq!(arm.data["sha256"], revision.sha256.as_str());
    assert_eq!(arm.data["by"], "Luke-MacBook");

    // Play Live: the rig follows the timeline inside tolerance.
    do_act(&api, Action::Play);
    let mut worst = Pose::default();
    let st = loop {
        let st = api.status().unwrap();
        assert_eq!(st.transport.output, Output::Armed, "fault: {:?}", st.telemetry.fault);
        assert!(st.hardware_live);
        worst = Pose::from_fn(|a| worst.get(a).max(st.first().error.get(a).abs()));
        if st.frame >= 25 + (end - 25) / 2 {
            break st;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(close(&worst, &Pose::default(), &tol, 1.0), "tracking error {worst:?} against tolerance {tol:?}");
    assert!(st.first().rig_pose.pan > 10.0 && rig.is_moving(), "mid-move at frame {}: {:?}", st.frame, st.first().rig_pose);

    // E-Stop from a station that does not hold the lease.
    rig.fr7.clear_seen();
    rig.pod.clear_seen();
    let watcher = t.client("watcher");
    let st = do_act(&watcher, Action::Estop);
    assert!(st.transport.estop && !st.hardware_live);
    assert_eq!((st.transport.run, st.transport.output), (Run::Stopped, Output::Rehearse));
    let st = wait_for(&api, |s| !s.telemetry.halting);
    assert!(!rig.is_moving(), "the rig stands still once the server says it has halted");
    let held = rig.pose();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.pose(), held);
    assert!(held.pan < 44.0, "stopped short of the end of the move: {held:?}");
    assert!(rig.fr7_stops() >= 2, "stop is repeated: {:02X?}", rig.fr7.commands());
    let commands = rig.fr7.commands();
    assert!(commands.contains(&visca::zoom_stop()) && commands.contains(&visca::focus_stop()));
    assert!(rig.pod.seen().iter().filter(|c| *c == "#PTS5050").count() >= 2, "{:?}", rig.pod.seen());

    // It latches: nothing else is accepted until it is reset.
    for action in Action::ALL {
        if !matches!(action, Action::Estop | Action::ResetEstop) {
            assert_eq!(refused(api.act(action, &TransportRequest::default()), 409), "Reset E-Stop to continue", "{action:?}");
        }
    }
    assert_eq!(st.guidance.primary_label, "Reset E-Stop");
    let st = do_act(&api, Action::ResetEstop);
    assert_eq!(st.transport, ptz_core::transport::TransportState::IDLE);

    // Arm again from where it stopped and run the move to its end.
    seek(&api, 25);
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.frame >= end + 12);
    let st = do_act(&api, Action::Stop);
    assert_eq!(st.transport.output, Output::Armed, "Stop holds the rig and stays armed");
    assert!(close(&rig.pose(), &mark.pose, &tol, 0.3), "ended at {:?}", rig.pose());
    rig.fr7.clear_seen();
    let st = do_act(&api, Action::Disarm);
    assert!(!st.hardware_live);
    wait_for(&api, |_| rig.fr7_stops() >= 1);

    let log = log_kinds(&api);
    let kinds: Vec<&str> = log.iter().map(|r| r.kind.as_str()).collect();
    for kind in ["rig_changed", "adapter", "arm", "pose", "estop_halted", "transport", "refused"] {
        assert!(kinds.contains(&kind), "no {kind} record in {kinds:?}");
    }
    let halted = log.iter().find(|r| r.kind == "estop_halted").unwrap();
    assert!(halted.data["after_ms"].as_u64().unwrap() < 1000, "{halted:?}");
    drop(api);
    let dir = t.stop_keeping_data();
    assert!(runlog::verify(&dir.join("log")).unwrap() > 20);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn arm_is_refused_for_each_missing_precondition_with_its_own_reason() {
    let t = TestServer::start("live-gate");
    let api = t.client("a");
    let rig = Rig::start();
    let arm = |api: &Api| refused(api.act(Action::Arm, &TransportRequest::default()), 409);

    assert_eq!(arm(&api), "Take the edit lease to control the show");
    api.take_lease("Luke-MacBook", false).unwrap();
    let project = rig.connect(&api);

    // An uncommitted song.
    let created = api.new_song("Gate", None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    assert_eq!(arm(&api), "Commit the song first");

    // A move squeezed until it is too fast: blocked until fixed or accepted.
    let mark = store(&api, "Far", Pose { pan: 90.0, ..Pose::home() });
    let (song, clip) = edit::insert_preset_move(&created.song, &project, &mark, 25).unwrap();
    let song = edit::set_custom_duration(&song, &project, &clip, 20).unwrap();
    api.put_working(&PutWorking { expected_counter: 0, song: song.clone() }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    assert_eq!(arm(&api), "Fix or accept 1 timeline issue first");
    let accepted = edit::accept_issues(&song, &project, &clip, "Luke-MacBook", "2026-11-14T10:00:00Z").unwrap();
    api.put_working(&PutWorking { expected_counter: 1, song: accepted }).unwrap();
    api.save(&id).unwrap();
    assert_eq!(arm(&api), "Commit the song first", "the acceptance is an edit: it has to be committed too");
    api.commit(&id).unwrap();

    // Each adapter in use has to answer, and the pedestal has to be calibrated.
    rig.pod.set_calibration("rER53");
    wait_for(&api, |s| s.first().adapters[1].calibrated == Some(false));
    assert_eq!(arm(&api), "The AutoPod+ is not calibrated. Run its calibration at the leg box");
    rig.pod.set_calibration("rER00");
    wait_for(&api, |s| s.first().adapters[1].calibrated == Some(true));
    rig.pod.set_power(false);
    wait_for(&api, |s| s.first().adapters[1].detail.ends_with("is in standby"));
    assert_eq!(arm(&api), "The AutoPod+ is in standby. Switch it on at the leg box");
    rig.pod.set_power(true);
    wait_for(&api, |s| s.first().adapters[1].detail.contains("answers in"));
    rig.fr7.set_silent(true);
    wait_for(&api, |s| !s.first().adapters[0].connected);
    assert_eq!(arm(&api), format!("The FR7 is not answering at {}", rig.fr7.visca));
    rig.fr7.set_silent(false);
    wait_for(&api, |s| s.first().adapters[0].connected);

    // Record and E-Stop.
    do_act(&api, Action::RecordOn);
    assert_eq!(arm(&api), "Turn Record off before arming");
    do_act(&api, Action::RecordOff);
    do_act(&api, Action::Estop);
    assert_eq!(arm(&api), "Reset E-Stop to continue");
    do_act(&api, Action::ResetEstop);

    // With everything in place it arms, and the log says who accepted what.
    let st = do_act(&api, Action::Arm);
    assert_eq!(st.transport.output, Output::Positioning);
    let log = log_kinds(&api);
    let arm_record = log.iter().rfind(|r| r.kind == "arm").unwrap();
    assert_eq!(arm_record.data["accepted"][0]["clip"], clip.as_str());
    assert_eq!(arm_record.data["accepted"][0]["by"], "Luke-MacBook");
    assert_eq!(arm_record.data["revision"], 2);
    let accepted = log.iter().find(|r| r.kind == "issue_accepted").expect("the acceptance is logged");
    assert!(accepted.data["issues"][0].as_str().unwrap().contains("too fast"), "{accepted:?}");
    let reasons: Vec<&str> = log.iter().filter(|r| r.kind == "refused").filter_map(|r| r.data["reason"].as_str()).collect();
    for reason in ["Commit the song first", "Fix or accept 1 timeline issue first", "Turn Record off before arming"] {
        assert!(reasons.contains(&reason), "{reason} is not in the log: {reasons:?}");
    }
    do_act(&api, Action::Disarm);
}

#[test]
fn rehearse_never_sends_a_motion_command_to_hardware() {
    let t = TestServer::start("live-rehearse");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    rig.connect(&api);

    // Walk-In has moves on every axis and an audio file to lock to.
    let walk_in = api.songs().unwrap().into_iter().find(|s| s.name == "Walk-In").expect("the seeded song").id;
    let song = api.load_song(&walk_in).unwrap().song;
    let file = format!("file:{}", song.media[0].id);
    let lock =
        |source: &str| api.act(Action::Lock, &TransportRequest { source: Some(source.into()), ..Default::default() }).unwrap();

    let quiet = |what: &str| {
        let st = api.status().unwrap();
        assert!(!st.hardware_live, "{what}: {:?}", st.transport);
        assert_eq!(st.transport.output, Output::Rehearse, "{what}");
        assert!(rig.fr7.commands().is_empty(), "{what}: the camera was sent {:02X?}", rig.fr7.commands());
        let pod = rig.pod.orders();
        assert!(pod.is_empty(), "{what}: the pedestal was sent {pod:?}");
    };

    // Every way of rolling: free-run, locked to a file, locked to code; with and without Record.
    for source in [None, Some(file.as_str()), Some("artnet")] {
        for record in [false, true] {
            let what = format!("source {source:?}, record {record}");
            if let Some(source) = source {
                lock(source);
            }
            if record {
                do_act(&api, Action::RecordOn);
            }
            let st = do_act(&api, Action::Play);
            let expect = if source == Some("artnet") { Run::WaitingForCode } else { Run::Playing };
            assert_eq!(st.transport.run, expect, "{what}");
            std::thread::sleep(Duration::from_millis(400));
            quiet(&format!("rolling, {what}"));
            do_act(&api, Action::Stop);
            if api.status().unwrap().transport.record {
                do_act(&api, Action::RecordOff);
            }
            // Scrubbing and cueing move the picture, never the rig.
            if st.transport.time != TimeMode::LockedExternal {
                seek(&api, 200);
                std::thread::sleep(Duration::from_millis(150));
                do_act(&api, Action::Cue);
            }
            if source.is_some() {
                do_act(&api, Action::Unlock);
            }
            quiet(&format!("after {what}"));
        }
    }
    assert!(refused(api.jog(CAM, &Jog { pan: 1.0, ..Jog::default() }), 409).contains("drives the simulator only"));
    quiet("after a refused jog");
    assert!(rig.fr7.seen().len() > 50, "the camera was polled throughout: {} messages", rig.fr7.seen().len());

    // E-Stop is the one thing Rehearse sends, and all it sends is stop.
    do_act(&api, Action::Estop);
    wait_for(&api, |s| !s.telemetry.halting);
    let commands = rig.fr7.commands();
    assert!(!commands.is_empty());
    assert!(
        commands.iter().all(|c| [visca::pan_tilt_stop(), visca::zoom_stop(), visca::focus_stop()].contains(c)),
        "{commands:02X?}"
    );
    assert!(rig.pod.orders().iter().all(|c| c == "#PTS5050"), "{:?}", rig.pod.orders());
}

#[test]
fn a_keep_out_rule_blocks_arm_when_the_song_breaks_it_and_holds_the_rig_when_the_way_there_does() {
    let t = TestServer::start("live-keepout");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    let mut project = rig.connect(&api);
    let rule = |below: f64| KeepOutRule {
        name: "Desk".into(),
        when_axis: Axis::Elevation,
        when: Cmp::Below,
        when_value: below,
        then_axis: Axis::Tilt,
        must_be: Cmp::Above,
        must_value: -10.0,
    };

    // The move tilts down as it rises. Below 250 mm it is already past -10 degrees: the song breaks the rule.
    let low = store(&api, "Low angle", Pose { tilt: -20.0, elevation: 300.0, ..Pose::home() });
    project.cameras[0].keep_out = vec![rule(250.0)];
    let project = api.put_project(&project).unwrap();
    let (song_id, clip) = song_with_move(&api, &project, "Keep Out", &low);
    let st = api.status().unwrap();
    assert_eq!(st.song.as_ref().unwrap().blocking, 1);
    assert_eq!(refused(api.act(Action::Arm, &TransportRequest::default()), 409), "Fix or accept 1 timeline issue first");
    let w = api.working(&song_id).unwrap();
    assert!(w.issues[0].message().contains("Keep-out rule broken: when elevation is below 250 mm, tilt must be above -10.0°"));
    // Accepting the move does not make a keep-out problem go away.
    let accepted = edit::accept_issues(&w.song, &project, &clip, "Luke-MacBook", "now").unwrap();
    api.put_working(&PutWorking { expected_counter: w.edit_counter, song: accepted }).unwrap();
    api.save(&song_id).unwrap();
    api.commit(&song_id).unwrap();
    assert_eq!(refused(api.act(Action::Arm, &TransportRequest::default()), 409), "Fix or accept 1 timeline issue first");

    // With the rule at 80 mm the song is clean: by the time it tilts past -10 it is above 150 mm.
    let mut relaxed = project.clone();
    relaxed.cameras[0].keep_out = vec![rule(80.0)];
    api.put_project(&relaxed).unwrap();
    let st = api.status().unwrap();
    assert_eq!(st.song.as_ref().unwrap().blocking, 0);

    // But arming at the end of the move, with the rig still at the bottom, tilts faster than it
    // rises. The straight way there breaks the rule, so the rig is held where the rule begins.
    let end = {
        let song = api.working(&song_id).unwrap().song;
        let c = song.motion_clip(&clip).unwrap();
        c.start_frame + c.duration_frames
    };
    seek(&api, end + 10);
    do_act(&api, Action::Arm);
    let st = wait_for(&api, |s| s.telemetry.fault.is_some());
    assert_eq!(
        st.telemetry.fault.as_deref(),
        Some("Keep-out rule broken: when elevation is below 80 mm, tilt must be above -10.0°. The rig is held")
    );
    assert_eq!((st.transport.output, st.transport.run), (Output::Rehearse, Run::Stopped));
    assert!(!st.hardware_live);
    wait_for(&api, |_| !rig.is_moving());
    let held = rig.pose();
    assert!(held.tilt > -10.6 && held.tilt < -8.0, "held at the edge of the rule, not past it: {held:?}");
    assert!(held.elevation < 80.0);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.pose(), held);
    let fault = log_kinds(&api).into_iter().rfind(|r| r.kind == "fault").expect("the fault is logged");
    assert!(fault.data["reason"].as_str().unwrap().starts_with("Keep-out rule broken"));

    // A rig standing outside its soft limits is not driven at all.
    rig.fr7.place(0.0, -29.0, 0.0, 0.5);
    let mut tight = relaxed.clone();
    tight.cameras[0].axes.tilt.min = -20.0;
    api.put_project(&tight).unwrap();
    seek(&api, 0);
    wait_for(&api, |s| s.first().rig_pose.tilt < -28.0);
    rig.fr7.clear_seen();
    do_act(&api, Action::Arm);
    let st = wait_for(&api, |s| s.telemetry.fault.is_some());
    let fault = st.telemetry.fault.unwrap();
    assert!(
        fault.starts_with("Limit reached: Tilt is at -2") && fault.ends_with("°, outside -20.0° to 195.0°. The rig is held"),
        "{fault}"
    );
    assert_eq!(st.transport.output, Output::Rehearse);
    assert!(rig.fr7.commands().iter().all(|c| !matches!(visca::parse_command(c), Some(visca::Command::Drive { pan, tilt, .. }) if pan != visca::Turn::Hold || tilt != visca::Turn::Hold)));
}

#[test]
fn a_rig_that_lags_or_stops_answering_is_held_and_disarmed_inside_the_tracking_window() {
    let t = TestServer::start("live-tracking");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    let project = rig.connect(&api);
    assert_eq!(project.cameras[0].rig.tracking_window_ms, 500);

    let far = store(&api, "Far left", Pose { pan: 120.0, ..Pose::home() });
    let (_, _) = song_with_move(&api, &project, "Tracking", &far);
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    // In the middle of the pan the head starts delivering a third of the speed it is asked for.
    wait_for(&api, |s| s.first().rig_pose.pan > 20.0);
    rig.fr7.set_speed_factor(0.33);
    let began = Instant::now();
    let st = wait_for(&api, |s| s.telemetry.fault.is_some());
    let took = began.elapsed();
    let fault = st.telemetry.fault.clone().unwrap();
    assert!(fault.starts_with("Tracking fault: pan is ") && fault.ends_with("from the timeline. The rig is held"), "{fault}");
    assert!(took > Duration::from_millis(500), "one bad reading is not a fault: {took:?}");
    assert!(took < Duration::from_millis(1500), "held inside the window once it was out of tolerance: {took:?}");
    assert_eq!((st.transport.output, st.transport.run), (Output::Rehearse, Run::Stopped));
    wait_for(&api, |_| !rig.is_moving());
    assert!(rig.pose().pan < 110.0, "held where it was, not driven on to the end");
    wait_for(&api, |_| rig.fr7_stops() >= 2);
    rig.fr7.set_speed_factor(1.0);

    // The fault stays on show until the operator arms again.
    let st = seek(&api, 0);
    assert!(st.telemetry.fault.is_some());
    let st = do_act(&api, Action::Arm);
    assert_eq!(st.telemetry.fault, None);
    wait_for(&api, |s| s.transport.output == Output::Armed);

    // A pulled cable while armed and standing still.
    rig.fr7.set_silent(true);
    let began = Instant::now();
    let st = wait_for(&api, |s| s.telemetry.fault.is_some());
    assert!(began.elapsed() < Duration::from_millis(1500), "{:?}", began.elapsed());
    assert_eq!(st.telemetry.fault, Some(format!("The FR7 is not answering at {}", rig.fr7.visca)));
    assert_eq!(st.transport.output, Output::Rehearse);
    assert_eq!(
        refused(api.act(Action::Arm, &TransportRequest::default()), 409),
        format!("The FR7 is not answering at {}", rig.fr7.visca)
    );
    rig.fr7.set_silent(false);
    wait_for(&api, |s| s.first().adapters[0].connected);
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);

    // The pedestal going away is the same.
    rig.pod.set_silent(true);
    let st = wait_for(&api, |s| s.telemetry.fault.is_some());
    assert_eq!(st.telemetry.fault, Some(format!("The AutoPod+ is not answering at {}", rig.pod.addr)));
    let log = log_kinds(&api);
    assert_eq!(log.iter().filter(|r| r.kind == "fault").count(), 3);
    assert!(log.iter().any(|r| r.kind == "adapter" && r.data["name"] == "FR7" && r.data["connected"] == false));
}

#[test]
fn syncing_presets_recalls_each_one_on_the_camera_and_is_refused_while_live() {
    let t = TestServer::start("live-presets");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    rig.pod.place(140.0);
    let project = rig.connect(&api);

    let mark = store(&api, "Mark", Pose { pan: 5.0, ..Pose::home() });
    song_with_move(&api, &project, "Presets", &mark);
    do_act(&api, Action::Play);
    assert_eq!(refused(api.sync_presets(CAM), 409), "Stop before reading presets from the camera, which moves it");
    do_act(&api, Action::Cue);
    do_act(&api, Action::Arm);
    assert_eq!(refused(api.sync_presets(CAM), 409), "Disarm before reading presets from the camera, which moves it");
    do_act(&api, Action::Disarm);
    wait_for(&api, |_| !rig.is_moving());

    rig.fr7.clear_seen();
    rig.pod.clear_seen();
    api.sync_presets(CAM).unwrap();
    let st = wait_for(&api, |s| s.telemetry.preset_sync.as_deref().is_some_and(|p| p.starts_with("Recalling preset")));
    assert_eq!(
        refused(api.act(Action::Arm, &TransportRequest::default()), 409),
        "Wait for the preset sync to finish before arming"
    );
    assert!(!st.hardware_live);
    wait_for(&api, |s| s.telemetry.preset_sync.is_none());
    let presets = api.presets().unwrap();

    // The three seeded presets carried camera numbers 1 to 3: they now hold what the camera holds.
    let by_number = |n: u32| presets.iter().find(|p| p.camera_number == Some(n)).unwrap();
    assert_eq!(by_number(1).name, "Wide");
    assert_eq!(by_number(2).name, "Lectern");
    assert_eq!(by_number(3).name, "Two-Shot");
    let lectern = by_number(2);
    let want = Pose { pan: -24.0, tilt: 1.5, zoom: 0.62, focus: 0.5, elevation: 140.0 };
    assert!(
        close(&lectern.pose, &want, &Pose { pan: 0.05, tilt: 0.05, zoom: 0.002, focus: 0.002, elevation: 1.0 }, 1.0),
        "{:?}",
        lectern.pose
    );
    assert!(lectern.has_thumbnail);
    assert_eq!(api.get_bytes(&format!("/presets/{}/thumb.jpg", lectern.id)).unwrap(), FAKE_PICTURE.to_vec());
    assert!(api.get_bytes(&format!("/presets/{}/thumb.jpg", mark.id)).is_err());
    assert!(presets.iter().any(|p| p.id == mark.id), "presets made by hand are kept");

    // The camera was recalled to each preset in turn, and that is all that moved it.
    let recalls: Vec<Vec<u8>> = rig.fr7.commands();
    assert_eq!(recalls, vec![visca::preset_recall(1), visca::preset_recall(2), visca::preset_recall(3)]);
    let pod = rig.pod.orders();
    assert!(pod.iter().all(|c| c == "#PTS5050"), "a sync does not move the pedestal: {pod:?}");
    assert_eq!(rig.pod.height_mm(), 140.0);

    // E-Stop during a sync stops the camera and abandons it.
    api.sync_presets(CAM).unwrap();
    wait_for(&api, |_| rig.fr7.is_moving());
    do_act(&api, Action::Estop);
    let st = wait_for(&api, |s| !s.telemetry.halting && s.telemetry.preset_sync.is_none());
    assert!(!rig.fr7.is_moving());
    assert_eq!(st.telemetry.fault.as_deref(), Some("Preset sync: The preset sync was stopped"));
    assert!(log_kinds(&api).iter().any(|r| r.kind == "presets_sync_failed"));
}

#[test]
fn with_absolute_moves_a_preset_move_is_one_command_and_the_other_axes_still_track() {
    let t = TestServer::start("live-absolute");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    let mut project = rig.connect(&api);
    project.cameras[0].rig.fr7_absolute_moves = true;
    let project = api.put_project(&project).unwrap();
    wait_for(&api, |s| s.first().adapters.len() == 2 && s.first().adapters.iter().all(|a| a.connected));

    let mark = store(&api, "Downstage", Pose { pan: 30.0, tilt: 8.0, zoom: 0.3, focus: 0.5, elevation: 40.0 });
    let (song_id, clip) = song_with_move(&api, &project, "Absolute", &mark);
    let end = {
        let song = api.working(&song_id).unwrap().song;
        let c = song.motion_clip(&clip).unwrap();
        c.start_frame + c.duration_frames
    };
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    rig.fr7.clear_seen();
    do_act(&api, Action::Play);
    let st = wait_for(&api, |s| s.frame >= end + 25 || s.telemetry.fault.is_some());
    assert_eq!(st.telemetry.fault, None);
    assert_eq!(st.transport.output, Output::Armed);
    let absolute: Vec<Vec<u8>> = rig.fr7.commands().into_iter().filter(|c| c.starts_with(&[0x81, 0x01, 0x06, 0x02])).collect();
    assert_eq!(absolute.len(), 1, "{absolute:02X?}");
    assert!(matches!(visca::parse_command(&absolute[0]), Some(visca::Command::Absolute { .. })));
    let tol = project.cameras[0].rig.tolerance;
    assert!(close(&rig.pose(), &mark.pose, &tol, 0.5), "ended at {:?}", rig.pose());
    do_act(&api, Action::Disarm);
}
