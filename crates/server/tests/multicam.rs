//! Several cameras on one transport: each with its own rig, presets, clips,
//! limits and faults. Hardware is the FR7 and AutoPod+ protocol fakes.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::camera::{CameraConfig, OnFault, MAX_CAMERAS};
use ptz_core::client::Api;
use ptz_core::compile::Issue;
use ptz_core::edit;
use ptz_core::eval::pose_at;
use ptz_core::pose::{Axis, Pose, Preset, Project, RigKind};
use ptz_core::song::Song;
use ptz_core::transport::{Action, Output, Run};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::time::Duration;

struct Rig {
    fr7: FakeFr7,
    pod: FakeAutoPod,
}

impl Rig {
    fn start() -> Rig {
        Rig { fr7: FakeFr7::start(), pod: FakeAutoPod::start() }
    }

    fn settings(&self, camera: &mut CameraConfig) {
        camera.rig.kind = RigKind::Hardware;
        camera.rig.fr7_host = self.fr7.visca.to_string();
        camera.rig.fr7_http_port = self.fr7.http.port();
        camera.rig.fr7_user = self.fr7.user.clone();
        camera.rig.fr7_password = self.fr7.password.clone();
        camera.rig.autopod_host = self.pod.addr.to_string();
        camera.rig.autopod_calibration = FakeAutoPod::height_map();
    }

    fn pose(&self) -> Pose {
        let (pan, tilt, zoom, focus) = self.fr7.pose();
        Pose { pan, tilt, zoom, focus, elevation: self.pod.height_mm() }
    }

    fn is_moving(&self) -> bool {
        self.fr7.is_moving() || self.pod.is_moving()
    }
}

fn with_cameras(api: &Api, count: usize) -> Project {
    let mut project = api.project().unwrap();
    while project.cameras.len() < count {
        let id = project.free_camera_id();
        let name = format!("Camera {}", project.cameras.len() + 1);
        project.cameras.push(CameraConfig::new(&id, &name));
    }
    api.put_project(&project).unwrap()
}

/// Put every camera on a pair of fakes and wait until all of them answer.
fn connect(api: &Api, rigs: &[Rig]) -> Project {
    let mut project = with_cameras(api, rigs.len());
    for (camera, rig) in project.cameras.iter_mut().zip(rigs) {
        rig.settings(camera);
    }
    let project = api.put_project(&project).unwrap();
    let ready = |c: &CameraStatus| {
        c.hardware && c.adapters.len() == 2 && c.adapters.iter().all(|a| a.connected) && c.adapters[1].calibrated == Some(true)
    };
    wait_for(api, |s| s.cameras.len() == rigs.len() && s.cameras.iter().all(ready));
    // On connecting the server sends each device a stop. After that, nothing until Arm.
    for rig in rigs {
        rig.fr7.settle_after_connect();
        rig.pod.settle_after_connect();
    }
    project
}

fn store(api: &Api, camera: &str, name: &str, pose: Pose) -> Preset {
    api.post("/presets", &StorePreset { name: name.into(), pose: Some(pose), camera: Some(camera.into()) }).unwrap()
}

/// A saved, committed song with one move per preset, each on its own camera, all starting on frame 25.
fn song_with_moves(api: &Api, project: &Project, name: &str, presets: &[Preset]) -> Song {
    let created = api.new_song(name, None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let mut song = created.song;
    for preset in presets {
        song = edit::insert_preset_move(&song, project, preset, 25).unwrap().0;
    }
    let working = api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    working.song
}

fn close(a: &Pose, b: &Pose, tol: &Pose, share: f64) -> bool {
    Axis::ALL.iter().all(|x| (a.get(*x) - b.get(*x)).abs() <= tol.get(*x) * share)
}

fn log(api: &Api) -> Vec<LogRecord> {
    api.get("/log?since=0").unwrap()
}

fn seek(api: &Api, frame: i64) -> Status {
    api.act(Action::Seek, &TransportRequest { frame: Some(frame), ..Default::default() }).unwrap()
}

#[test]
fn a_project_holds_one_to_twelve_cameras_and_a_removed_camera_takes_its_presets_and_blocks_its_moves() {
    let t = TestServer::start("multicam-project");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();

    let st = api.status().unwrap();
    assert_eq!(st.cameras.len(), 1, "a new show has one camera");
    assert_eq!(st.cameras[0].id, "cam1");
    assert_eq!(st.record.camera, "cam1");

    let project = with_cameras(&api, MAX_CAMERAS);
    let st = api.status().unwrap();
    let ids: Vec<&str> = st.cameras.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, ["cam1", "cam2", "cam3", "cam4", "cam5", "cam6", "cam7", "cam8", "cam9", "cam10", "cam11", "cam12"]);
    assert!(st.cameras.iter().all(|c| !c.hardware && c.adapters.len() == 1 && c.adapters[0].name == "Simulator"));
    assert!(!st.telemetry.hardware);

    // What cannot be used is refused whole, in words.
    let mut thirteen = project.clone();
    thirteen.cameras.push(CameraConfig::new("cam13", "One too many"));
    assert_eq!(refused(api.put_project(&thirteen), 422), "A project can hold 12 cameras; this one has 13");
    let mut twice = project.clone();
    twice.cameras[4].id = "cam2".into();
    assert_eq!(refused(api.put_project(&twice), 422), "Camera 5 shares its id with another camera");
    let mut none = project.clone();
    none.cameras.clear();
    assert_eq!(refused(api.put_project(&none), 422), "A project needs at least one camera");
    let mut limits = project.clone();
    limits.cameras[6].name = "Balcony".into();
    limits.cameras[6].axes.tilt.max = -500.0;
    assert_eq!(
        refused(api.put_project(&limits), 422),
        "7 Balcony: Tilt limits are not usable: min must be below max, speeds above zero"
    );
    assert_eq!(api.project().unwrap(), project, "a refused project changes nothing");

    // A preset belongs to the camera it was stored for.
    let wide = store(&api, "cam1", "Wide", Pose { pan: 10.0, ..Pose::home() });
    let keys = store(&api, "cam3", "Keys", Pose { pan: -35.0, ..Pose::home() });
    assert_eq!((wide.camera.as_str(), keys.camera.as_str()), ("cam1", "cam3"));
    let missing = StorePreset { name: "Nowhere".into(), pose: None, camera: Some("cam40".into()) };
    assert_eq!(refused(api.post::<_, Preset>("/presets", &missing), 404), "No camera with id cam40");

    let song = song_with_moves(&api, &project, "Three Cameras", &[wide.clone(), keys.clone()]);
    assert_eq!(song.motion.iter().map(|c| c.camera.as_str()).collect::<Vec<_>>(), ["cam1", "cam3"]);
    assert_eq!(api.status().unwrap().song.unwrap().blocking, 0);

    // Cameras come and go only while the show is parked.
    let mut fewer = project.clone();
    fewer.cameras.retain(|c| c.id != "cam3");
    do_act(&api, Action::Play);
    assert_eq!(refused(api.put_project(&fewer), 409), "Stop before adding, removing or reordering cameras");
    do_act(&api, Action::Stop);
    api.act(Action::RecordOn, &TransportRequest { camera: Some("cam3".into()), ..Default::default() }).unwrap();
    assert_eq!(api.status().unwrap().record.camera, "cam3");
    do_act(&api, Action::RecordOff);

    api.put_project(&fewer).unwrap();
    let st = api.status().unwrap();
    assert_eq!(st.cameras.len(), 11);
    assert!(st.camera("cam3").is_none());
    assert_eq!(st.record.camera, "cam1", "Record falls back to a camera that exists");
    let presets: Vec<Preset> = api.presets().unwrap();
    assert!(presets.iter().any(|p| p.id == wide.id) && !presets.iter().any(|p| p.id == keys.id), "{presets:?}");

    // Its moves stay on the timeline, and stop the show from being armed until they are dealt with.
    let working = api.working(&song.id).unwrap();
    assert_eq!(working.song.motion.len(), 2);
    let stranded: Vec<&Issue> = working.issues.iter().filter(|i| matches!(i, Issue::MissingCamera { .. })).collect();
    assert_eq!(stranded.len(), 1);
    assert_eq!(stranded[0].message(), "This move is for a camera that is no longer in the project");
    assert_eq!(refused(api.act(Action::Arm, &TransportRequest::default()), 409), "Fix or accept 1 timeline issue first");
    let records = log(&api);
    let changed = records.iter().rfind(|r| r.kind == "cameras_changed").expect("logged");
    assert_eq!(changed.data["removed"], serde_json::json!(["cam3"]));
    assert!(records.iter().any(|r| r.kind == "presets_removed" && r.data["count"] == 1));

    // The settings survive a restart.
    let dir = t.stop_keeping_data();
    let t = TestServer::reopen(&dir);
    let api = t.client("b");
    assert_eq!(api.project().unwrap(), fewer);
    assert_eq!(api.status().unwrap().cameras.len(), 11);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_show_from_before_cameras_opens_as_one_camera_with_its_rig_limits_and_presets() {
    let dir = temp_dir("multicam-migrate");
    std::fs::create_dir_all(&dir).unwrap();
    // project.json and presets.json as the single-rig version wrote them.
    let old_project = serde_json::json!({
        "rate": "29.97df",
        "routing": { "left": "program", "right": "ltc" },
        "freewheel_frames": 12,
        "ltc_level": 0.4,
        "axes": Project::default().first().axes,
        "keep_out": [],
        "rig": { "kind": "simulator", "positioning_speed": 0.25, "tracking_window_ms": 750 },
    });
    let mut old_project = old_project;
    old_project["axes"]["pan"]["max_speed"] = 33.0.into();
    std::fs::write(dir.join("project.json"), serde_json::to_vec_pretty(&old_project).unwrap()).unwrap();
    let old_presets = serde_json::json!([
        { "id": "wide", "name": "Wide", "pose": { "pan": 5.0, "tilt": 1.0, "zoom": 0.1, "focus": 0.5, "elevation": 100.0 } }
    ]);
    std::fs::write(dir.join("presets.json"), serde_json::to_vec(&old_presets).unwrap()).unwrap();

    let t = TestServer::reopen(&dir);
    let api = t.client("a");
    let project = api.project().unwrap();
    assert_eq!(project.cameras.len(), 1);
    let camera = project.first();
    assert_eq!((camera.id.as_str(), camera.name.as_str()), ("cam1", "Camera 1"));
    assert_eq!(camera.axes.pan.max_speed, 33.0);
    assert_eq!((camera.rig.positioning_speed, camera.rig.tracking_window_ms), (0.25, 750));
    assert_eq!((project.freewheel_frames, project.ltc_level), (12, 0.4));
    assert_eq!(project.rate, ptz_core::time::FrameRate::F2997Df);
    assert_eq!(project.on_fault, OnFault::HoldCamera);
    let presets = api.presets().unwrap();
    assert_eq!((presets.len(), presets[0].camera.as_str()), (1, "cam1"));
    drop(t);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn simulated_cameras_follow_their_own_clips_and_are_jogged_stored_synced_and_recorded_one_at_a_time() {
    let t = TestServer::start("multicam-sim");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let mut project = with_cameras(&api, 3);
    project.cameras[1].name = "Stage left".into();
    let project = api.put_project(&project).unwrap();

    let wide = store(&api, "cam1", "Wide", Pose { pan: 10.0, ..Pose::home() });
    let keys = store(&api, "cam2", "Keys", Pose { pan: -35.0, tilt: 4.0, ..Pose::home() });
    let song = song_with_moves(&api, &project, "Two Moves", &[wide.clone(), keys.clone()]);

    // The monitor pose of each camera is its own timeline.
    let st = seek(&api, 600);
    assert_eq!(st.cameras.iter().map(|c| c.pose.pan).collect::<Vec<_>>(), [10.0, -35.0, 0.0]);
    assert!(st.cameras.iter().all(|c| c.pose_source == PoseSource::Timeline));
    assert_eq!(st.first().id, "cam1");

    // Hand control moves the camera it is sent to and no other.
    let top = project.cameras[1].axes.pan.max_speed;
    for _ in 0..20 {
        api.jog("cam2", &Jog { pan: 10.0 / top, ..Jog::default() }).unwrap();
        std::thread::sleep(Duration::from_millis(25));
    }
    let st = api.jog("cam2", &Jog::default()).unwrap();
    let second = st.camera("cam2").unwrap();
    assert_eq!(second.pose_source, PoseSource::Rig);
    assert!(second.rig_pose.pan > -33.5, "jogged right from -35: {:?}", second.rig_pose);
    assert_eq!(st.camera("cam1").unwrap().pose_source, PoseSource::Timeline);
    assert_eq!(st.camera("cam1").unwrap().rig_pose.pan, 10.0);
    assert_eq!(refused(api.jog("cam9", &Jog::default()), 404), "No camera with id cam9");

    // With no pose given, a preset holds where that camera stands.
    wait_for(&api, |s| s.camera("cam2").unwrap().rig_pose == s.camera("cam2").unwrap().pose);
    let here: Preset =
        api.post("/presets", &StorePreset { name: "Here".into(), pose: None, camera: Some("cam2".into()) }).unwrap();
    assert_eq!(here.camera, "cam2");
    assert!(here.pose.pan > -33.5 && here.pose.tilt == 4.0, "{:?}", here.pose);

    // Sync reads one camera's presets. The same names on two cameras are two presets.
    let before = api.presets().unwrap().len();
    let after_first = api.sync_presets("cam2").unwrap();
    let from_second: Vec<&Preset> = after_first.iter().filter(|p| p.camera == "cam2" && p.camera_number.is_some()).collect();
    assert_eq!(from_second.len(), 3);
    assert_eq!(after_first.len(), before + 3);
    let after_again = api.sync_presets("cam2").unwrap();
    assert_eq!(after_again.len(), before + 3, "a second sync matches by the number on the device");
    let after_third = api.sync_presets("cam3").unwrap();
    assert_eq!(after_third.iter().filter(|p| p.camera == "cam3").count(), 3);
    let mut ids: Vec<&str> = after_third.iter().map(|p| p.id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), after_third.len(), "ids stay unique across cameras");
    assert_eq!(refused(api.sync_presets("cam7"), 404), "No camera with id cam7");

    // Record captures one camera. The other cameras play the timeline and cannot be hand-driven meanwhile.
    seek(&api, 300);
    api.act(
        Action::RecordOn,
        &TransportRequest { camera: Some("cam2".into()), axes: Some(vec![Axis::Tilt]), ..Default::default() },
    )
    .unwrap();
    let st = do_act(&api, Action::Play);
    assert_eq!((st.record.camera.as_str(), st.record.active), ("cam2", true));
    assert_eq!(
        refused(api.jog("cam1", &Jog { pan: 0.5, ..Jog::default() }), 409),
        "Record is capturing 2 Stage left. Stop to position another camera by hand"
    );
    wait_for(&api, |s| s.frame >= 310);
    while api.status().unwrap().frame < 360 {
        let st = api.jog("cam2", &Jog { tilt: 0.3, pan: 1.0, ..Jog::default() }).unwrap();
        assert_eq!(st.camera("cam2").unwrap().pose_source, PoseSource::Record);
        assert_eq!(st.camera("cam1").unwrap().pose_source, PoseSource::Timeline);
        std::thread::sleep(Duration::from_millis(20));
    }
    api.jog("cam2", &Jog::default()).unwrap();
    wait_for(&api, |s| s.frame >= 380);
    let st = do_act(&api, Action::Stop);
    assert!(!st.transport.record, "Stop ends the pass");

    let after = api.working(&song.id).unwrap().song;
    let recorded = after.motion.iter().find(|c| c.recorded_pass.is_some()).expect("the pass is on the timeline");
    assert_eq!((recorded.camera.as_str(), recorded.axes.as_slice()), ("cam2", &[Axis::Tilt][..]));
    assert!(pose_at(&after, "cam2", 365).tilt > pose_at(&song, "cam2", 365).tilt + 1.0, "the second camera's tilt was recorded");
    for f in (0..=after.length_frames).step_by(5) {
        assert_eq!(pose_at(&after, "cam1", f), pose_at(&song, "cam1", f), "the first camera's motion is untouched at {f}");
        assert_eq!(pose_at(&after, "cam2", f).pan, pose_at(&song, "cam2", f).pan, "pan was not record-enabled at {f}");
    }
    let pass = log(&api).into_iter().rfind(|r| r.kind == "record_pass").expect("logged");
    assert_eq!(pass.data["camera"], "cam2");
}

#[test]
fn twelve_stand_in_rigs_position_together_play_live_within_tolerance_and_estop_halts_every_one() {
    let t = TestServer::start("multicam-twelve");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs: Vec<Rig> = (0..MAX_CAMERAS).map(|_| Rig::start()).collect();
    for (n, rig) in rigs.iter().enumerate() {
        rig.fr7.place(n as f64 - 6.0, 2.0, 0.1, 0.5);
        rig.pod.place(10.0 + n as f64);
    }
    let project = connect(&api, &rigs);
    let tol = project.first().rig.tolerance;

    // Every camera gets a different mark, so a crossed wire would show.
    let marks: Vec<Preset> = project
        .cameras
        .iter()
        .enumerate()
        .map(|(n, camera)| {
            let n = n as f64;
            let pose = Pose { pan: 12.0 + 3.0 * n, tilt: -6.0 + n, zoom: 0.1 + 0.05 * n, focus: 0.4, elevation: 40.0 + 6.0 * n };
            store(&api, &camera.id, &format!("Mark {}", camera.id), pose)
        })
        .collect();
    let song = song_with_moves(&api, &project, "Twelve", &marks);
    let end = song.motion.iter().map(|c| c.start_frame + c.duration_frames).max().unwrap();
    assert!(rigs.iter().all(|r| r.fr7.commands().is_empty()), "nothing is sent before Arm");

    // Arm positions all twelve to the first frame; the show is armed when the last one is there.
    let st = do_act(&api, Action::Arm);
    assert_eq!(st.transport.output, Output::Positioning);
    let st = wait_for(&api, |s| s.transport.output == Output::Armed || s.telemetry.fault.is_some());
    assert_eq!(st.telemetry.fault, None);
    for (n, rig) in rigs.iter().enumerate() {
        assert!(close(&rig.pose(), &Pose::home(), &tol, 0.3), "rig {} positioned to the first frame: {:?}", n + 1, rig.pose());
    }
    let arm = log(&api).into_iter().rfind(|r| r.kind == "arm").expect("Arm is logged");
    assert_eq!(arm.data["cameras"].as_array().unwrap().len(), 12);
    assert_eq!(arm.data["cameras"][11]["camera"], "cam12");
    assert_eq!(arm.data["cameras"][11]["rig"], serde_json::json!(["FR7", "AutoPod+"]));

    do_act(&api, Action::Play);
    let mut worst = Pose::default();
    loop {
        let st = api.status().unwrap();
        assert_eq!(st.transport.output, Output::Armed, "fault: {:?}", st.telemetry.fault);
        if st.frame > end + 12 {
            break;
        }
        for camera in &st.cameras {
            assert!(!camera.held && camera.fault.is_none(), "{camera:?}");
            worst = Pose::from_fn(|a| worst.get(a).max(camera.error.get(a).abs()));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(Axis::ALL.iter().all(|a| worst.get(*a) <= tol.get(*a)), "worst error {worst:?} against tolerance {tol:?}");
    for (rig, mark) in rigs.iter().zip(&marks) {
        assert!(close(&rig.pose(), &mark.pose, &tol, 0.5), "{} ended at {:?}, wanted {:?}", mark.name, rig.pose(), mark.pose);
    }

    // Back to the top and into the move again, then E-Stop: all twelve are told to stop and do.
    do_act(&api, Action::Stop);
    let st = seek(&api, 0);
    assert_eq!(st.transport.output, Output::Positioning, "a moved playhead is positioned to, not jumped to");
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.frame > 40);
    assert!(rigs.iter().all(|r| r.is_moving()), "all twelve are mid-move");
    let st = api.post_empty::<Status>("/estop").unwrap();
    assert!(st.transport.estop);
    let st = wait_for(&api, |s| !s.telemetry.halting);
    assert!(st.cameras.iter().all(|c| !c.halting));
    assert!(rigs.iter().all(|r| !r.is_moving()));
    let halted: Vec<String> =
        log(&api).iter().filter(|r| r.kind == "estop_halted").map(|r| r.data["camera"].as_str().unwrap().to_string()).collect();
    assert_eq!(halted.len(), 12, "{halted:?}");
    let stood: Vec<Pose> = rigs.iter().map(|r| r.pose()).collect();
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(rigs.iter().map(|r| r.pose()).collect::<Vec<_>>(), stood);
}

#[test]
fn a_camera_that_faults_is_held_while_the_others_carry_on_unless_the_project_says_all_stop() {
    let t = TestServer::start("multicam-fault");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs = [Rig::start(), Rig::start(), Rig::start()];
    let mut project = connect(&api, &rigs);
    project.cameras[1].name = "Stage left".into();
    let project = api.put_project(&project).unwrap();
    assert_eq!(project.on_fault, OnFault::HoldCamera);
    let tol = project.first().rig.tolerance;

    let marks: Vec<Preset> = project
        .cameras
        .iter()
        .map(|camera| store(&api, &camera.id, &format!("Far {}", camera.id), Pose { pan: 120.0, ..Pose::home() }))
        .collect();
    let song = song_with_moves(&api, &project, "Fault", &marks);
    let end = song.motion.iter().map(|c| c.start_frame + c.duration_frames).max().unwrap();

    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.cameras.iter().all(|c| c.rig_pose.pan > 20.0));

    // In the middle of the pan the second camera's head starts delivering a third of the speed it is asked for.
    rigs[1].fr7.set_speed_factor(0.33);
    let st = wait_for(&api, |s| s.camera("cam2").unwrap().held);
    let why = st.camera("cam2").unwrap().fault.clone().expect("a held camera says why");
    assert!(why.starts_with("Tracking fault: pan is ") && why.ends_with("from the timeline. The rig is held"), "{why}");
    assert_eq!(st.telemetry.fault, Some(format!("2 Stage left: {why}")));
    assert_eq!((st.transport.output, st.transport.run), (Output::Armed, Run::Playing), "the show goes on");
    assert!(st.hardware_live);
    assert!(!st.camera("cam1").unwrap().held && !st.camera("cam3").unwrap().held);

    // The other two finish the move on their marks; the held one is not driven any further.
    let st = wait_for(&api, |s| s.frame > end + 12 || s.transport.output != Output::Armed);
    assert_eq!(st.transport.output, Output::Armed, "fault: {:?}", st.telemetry.fault);
    assert!(close(&rigs[0].pose(), &marks[0].pose, &tol, 0.5), "{:?}", rigs[0].pose());
    assert!(close(&rigs[2].pose(), &marks[2].pose, &tol, 0.5), "{:?}", rigs[2].pose());
    rigs[1].fr7.set_speed_factor(1.0);
    wait_for(&api, |_| !rigs[1].is_moving());
    let stood = rigs[1].pose();
    assert!(stood.pan < 110.0, "held where it stopped: {stood:?}");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rigs[1].pose(), stood, "a held camera is sent nothing more, even once it could follow again");
    let st = api.status().unwrap();
    assert!(st.camera("cam2").unwrap().held, "held until the show is armed again");

    let records = log(&api);
    let fault = records.iter().rfind(|r| r.kind == "fault").unwrap();
    assert_eq!((fault.data["camera"].as_str(), fault.data["reason"].as_str()), (Some("cam2"), Some(why.as_str())));
    let held = records.iter().rfind(|r| r.kind == "camera_held").expect("logged");
    assert_eq!((held.data["camera"].as_str(), held.data["still_running"].as_u64()), (Some("cam2"), Some(2)));
    assert!(!records.iter().any(|r| r.kind == "event" && r.data["event"] == "Fault"), "output was never cut");

    // Stop, Disarm and Arm again brings the held camera back with the others.
    do_act(&api, Action::Stop);
    do_act(&api, Action::Disarm);
    let st = api.status().unwrap();
    assert!(!st.camera("cam2").unwrap().held);
    seek(&api, 0);
    let st = do_act(&api, Action::Arm);
    assert_eq!(st.telemetry.fault, None);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    assert!(close(&rigs[1].pose(), &Pose::home(), &tol, 0.3), "{:?}", rigs[1].pose());

    // When the last camera standing faults there is no show left to run: output is cut.
    rigs[0].pod.set_silent(true);
    wait_for(&api, |s| s.camera("cam1").unwrap().held);
    rigs[1].fr7.set_silent(true);
    wait_for(&api, |s| s.camera("cam2").unwrap().held);
    assert_eq!(api.status().unwrap().transport.output, Output::Armed);
    rigs[2].fr7.set_silent(true);
    let st = wait_for(&api, |s| s.transport.output == Output::Rehearse);
    assert!(!st.hardware_live);
    assert!(st.cameras.iter().all(|c| c.fault.is_some() && !c.held), "{:?}", st.cameras);
    assert_eq!(
        st.telemetry.fault,
        Some(format!("The AutoPod+ is not answering at {}", rigs[0].pod.addr)).map(|w| format!("1 Camera 1: {w}"))
    );
    assert_eq!(
        refused(api.act(Action::Arm, &TransportRequest::default()), 409),
        format!("1 Camera 1: The AutoPod+ is not answering at {}", rigs[0].pod.addr)
    );
    rigs[0].pod.set_silent(false);
    rigs[1].fr7.set_silent(false);
    rigs[2].fr7.set_silent(false);
    wait_for(&api, |s| s.adapters().all(|a| a.connected));

    // "Stop all cameras": one fault cuts output for every camera.
    let mut all = api.project().unwrap();
    all.on_fault = OnFault::HoldAll;
    api.put_project(&all).unwrap();
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.cameras.iter().all(|c| c.rig_pose.pan > 20.0));
    rigs[2].fr7.set_speed_factor(0.33);
    let st = wait_for(&api, |s| s.telemetry.fault.is_some());
    let fault = st.telemetry.fault.clone().unwrap();
    assert!(fault.starts_with("3 Camera 3: Tracking fault: pan is "), "{fault}");
    assert_eq!((st.transport.output, st.transport.run), (Output::Rehearse, Run::Stopped));
    wait_for(&api, |_| rigs.iter().all(|r| !r.is_moving()));
    assert!(rigs.iter().all(|r| r.pose().pan < 110.0), "every camera was stopped where it was");
    assert!(st.cameras.iter().all(|c| !c.held));
    assert!(st.camera("cam1").unwrap().fault.is_none() && st.camera("cam3").unwrap().fault.is_some());
}

#[test]
fn a_project_set_to_carry_on_never_holds_a_camera_for_being_off_the_timeline() {
    let t = TestServer::start("multicam-carry-on");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs = [Rig::start(), Rig::start()];
    let mut project = connect(&api, &rigs);
    project.on_fault = OnFault::CarryOn;
    let project = api.put_project(&project).unwrap();
    assert_eq!(api.project().unwrap().on_fault, OnFault::CarryOn);
    let tol = project.first().rig.tolerance;

    let marks: Vec<Preset> = project
        .cameras
        .iter()
        .map(|camera| store(&api, &camera.id, &format!("Far {}", camera.id), Pose { pan: 60.0, ..Pose::home() }))
        .collect();
    let song = song_with_moves(&api, &project, "Carry on", &marks);
    let end = song.motion.iter().map(|c| c.start_frame + c.duration_frames).max().unwrap();

    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.cameras.iter().all(|c| c.rig_pose.pan > 10.0));

    // The second camera's head starts delivering a third of the speed it is asked for. It falls
    // well outside its tolerance, which would hold it under either of the other rules.
    rigs[1].fr7.set_speed_factor(0.33);
    let mut worst = 0.0f64;
    let st = wait_for(&api, |s| {
        worst = worst.max(s.camera("cam2").unwrap().error.pan.abs());
        s.frame > end + 12 || s.transport.output != Output::Armed || s.telemetry.fault.is_some()
    });
    assert!(worst > tol.pan * 2.0, "the head only fell {worst:.1} degrees behind");
    assert_eq!((st.transport.output, st.transport.run), (Output::Armed, Run::Playing), "fault: {:?}", st.telemetry.fault);
    assert_eq!(st.telemetry.fault, None);
    assert!(st.cameras.iter().all(|c| !c.held && c.fault.is_none()), "{:?}", st.cameras);
    // It is driven on and gets to its mark late.
    wait_for(&api, |_| close(&rigs[1].pose(), &marks[1].pose, &tol, 0.5));
    assert!(close(&rigs[0].pose(), &marks[0].pose, &tol, 0.5), "{:?}", rigs[0].pose());

    // Arm with a head that cannot get to the playhead in the time the move should take: the
    // show is armed all the same, the log says how far off the head was, and it is driven on.
    do_act(&api, Action::Stop);
    do_act(&api, Action::Disarm);
    rigs[1].fr7.set_speed_factor(0.05);
    seek(&api, 0);
    do_act(&api, Action::Arm);
    let st = wait_for(&api, |s| s.transport.output != Output::Positioning);
    assert_eq!(st.transport.output, Output::Armed, "fault: {:?}", st.telemetry.fault);
    assert_eq!(st.telemetry.fault, None);
    assert!(rigs[1].pose().pan > 20.0, "the slow head is nowhere near: {:?}", rigs[1].pose());
    let records = log(&api);
    let late: Vec<&LogRecord> = records.iter().filter(|r| r.kind == "positioning_late").collect();
    assert_eq!(late.iter().map(|r| r.data["camera"].as_str().unwrap()).collect::<Vec<_>>(), ["cam2"]);
    assert!(late[0].data["error"]["pan"].as_f64().unwrap().abs() > 20.0, "{:?}", late[0].data);
    assert!(!records.iter().any(|r| r.kind == "fault" || r.kind == "camera_held"), "nothing was held");
    rigs[1].fr7.set_speed_factor(1.0);
    wait_for(&api, |_| close(&rigs[1].pose(), &Pose::home(), &tol, 0.5));

    // A rig that stops answering is still a fault: there is nothing to drive.
    rigs[1].fr7.set_silent(true);
    let st = wait_for(&api, |s| s.camera("cam2").unwrap().held);
    assert_eq!(st.transport.output, Output::Armed, "the other camera carries on");
    assert!(!st.camera("cam1").unwrap().held);
    rigs[1].fr7.set_silent(false);
}

#[test]
fn rig_settings_rebuild_only_the_camera_they_belong_to() {
    let t = TestServer::start("multicam-rebuild");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs = [Rig::start(), Rig::start()];
    let project = connect(&api, &rigs);
    rigs[0].fr7.clear_seen();
    let first_before = api.status().unwrap().cameras[0].adapters[0].clone();

    // The second camera goes back to the simulator. The first keeps its connection.
    let mut next = project.clone();
    next.cameras[1].rig.kind = RigKind::Simulator;
    api.put_project(&next).unwrap();
    let st = api.status().unwrap();
    assert!(st.cameras[0].hardware && !st.cameras[1].hardware);
    assert!(st.telemetry.hardware, "one camera on hardware makes the show hardware");
    assert!(st.cameras[0].adapters[0].connected);
    assert_eq!(st.cameras[0].adapters[0].name, first_before.name);
    let changes: Vec<LogRecord> = log(&api).into_iter().filter(|r| r.kind == "rig_changed").collect();
    assert_eq!(changes.last().unwrap().data["camera"], "cam2");
    assert_eq!(changes.iter().filter(|r| r.data["camera"] == "cam1").count(), 1, "only when it was first set to hardware");

    // Limits are not the rig: changing them rebuilds nothing, live or not.
    let mut slower = next.clone();
    slower.cameras[0].axes.pan.max_speed = 20.0;
    api.put_project(&slower).unwrap();
    assert_eq!(log(&api).into_iter().filter(|r| r.kind == "rig_changed").count(), changes.len());

    // Arm checks every camera: the simulator has no problem, the hardware one must answer.
    rigs[0].fr7.set_silent(true);
    wait_for(&api, |s| !s.cameras[0].adapters[0].connected);
    let created = api.new_song("Empty", None).unwrap();
    api.load_song(&created.song.id).unwrap();
    api.save(&created.song.id).unwrap();
    api.commit(&created.song.id).unwrap();
    assert_eq!(
        refused(api.act(Action::Arm, &TransportRequest::default()), 409),
        format!("1 Camera 1: The FR7 is not answering at {}", rigs[0].fr7.visca)
    );
    // Sitting the camera out is setting it to the simulator.
    let mut out = slower.clone();
    out.cameras[0].rig.kind = RigKind::Simulator;
    api.put_project(&out).unwrap();
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    assert!(!api.status().unwrap().telemetry.hardware);
}
