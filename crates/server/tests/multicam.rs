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
use ptz_server::hw::visca;
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
    song_at(api, project, name, presets.iter().map(|preset| (25, preset)))
}

fn song_at<'a>(api: &Api, project: &Project, name: &str, cues: impl IntoIterator<Item = (i64, &'a Preset)>) -> Song {
    let created = api.new_song(name, None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let mut song = created.song;
    for (frame, preset) in cues {
        song = edit::insert_preset_move(&song, project, preset, frame).unwrap().0;
    }
    let working = api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    working.song
}

fn close(a: &Pose, b: &Pose, tol: &Pose, share: f64) -> bool {
    Axis::ALL.iter().all(|x| (a.get(*x) - b.get(*x)).abs() <= tol.get(*x) * share)
}

fn clips_of<'a>(song: &'a Song, camera: &str) -> Vec<&'a ptz_core::song::MotionClip> {
    song.motion.iter().filter(|c| c.camera == camera).collect()
}

fn log(api: &Api) -> Vec<LogRecord> {
    api.get("/log?since=0").unwrap()
}

fn seek(api: &Api, frame: i64) -> Status {
    api.act(Action::Seek, &TransportRequest { frame: Some(frame), ..Default::default() }).unwrap()
}

fn play_live(api: &Api) {
    do_act(api, Action::Arm);
    wait_for(api, |s| s.transport.output == Output::Armed);
    assert_eq!(do_act(api, Action::Play).transport.run, Run::Playing);
}

fn assert_playing(status: &Status) {
    assert_eq!((status.transport.output, status.transport.run), (Output::Armed, Run::Playing), "{:?}", status.telemetry);
}

#[test]
fn hand_control_takes_one_camera_and_release_or_watchdog_gives_it_back_to_the_show() {
    let t = TestServer::start("manual-preset-live");
    let api = t.client("operator");
    api.take_lease("Operator", false).unwrap();
    let project = with_cameras(&api, 2);
    let first = store(&api, "cam1", "First", Pose { pan: 38.0, ..Pose::home() });
    let then = store(&api, "cam1", "Then", Pose { pan: 54.0, ..Pose::home() });
    let other = store(&api, "cam2", "Other", Pose { pan: -28.0, ..Pose::home() });
    let song = song_at(&api, &project, "Hands and cues", [(15, &first), (20, &other), (90, &then)]);
    let end = song.motion.iter().map(|clip| clip.end_frame()).max().unwrap();
    play_live(&api);

    let state = wait_for(&api, |s| s.first().rig_pose.pan > 4.0);
    let touched_at = state.frame;
    let started_at = state.first().rig_pose.pan;
    let jog = Jog { pan: -0.4, ..Jog::default() };
    for _ in 0..12 {
        let state = api.jog("cam1", &jog).unwrap();
        assert_playing(&state);
        assert!(state.first().armed && state.first().manual && !state.first().catching_up);
        assert!(!state.camera("cam2").unwrap().manual);
        std::thread::sleep(Duration::from_millis(35));
    }
    let state = api.status().unwrap();
    assert!(state.frame > touched_at && state.first().rig_pose.pan < started_at - 1.0, "{state:?}");
    assert!(state.first().intended_pose.pan > state.first().rig_pose.pan + 10.0, "{state:?}");
    assert!(state.camera("cam2").unwrap().rig_pose.pan < -2.0, "the other camera made its cue: {state:?}");

    let released = api.jog("cam1", &Jog::default()).unwrap();
    assert!(!released.first().manual && released.first().catching_up, "{released:?}");
    let state = wait_for(&api, |s| s.frame > end && !s.first().catching_up);
    assert_playing(&state);
    assert_ne!(state.timecode, released.timecode);
    assert!((state.first().rig_pose.pan - then.pose.pan).abs() < 1.0, "{:?}", state.first());
    assert!((state.camera("cam2").unwrap().rig_pose.pan - other.pose.pan).abs() < 1.0);

    // A station that disappears can leave a nonzero jog in flight. The watchdog releases it too.
    let manually = api.jog("cam1", &jog).unwrap();
    assert!(manually.first().manual);
    let state = wait_for(&api, |s| !s.first().manual && !s.first().catching_up && s.frame > manually.frame + 6);
    assert_playing(&state);
    assert!(state.cameras.iter().all(|c| c.fault.is_none()));
    assert!((state.first().rig_pose.pan - then.pose.pan).abs() < 1.0);
}

#[test]
fn disarming_interrupts_an_absolute_move_and_rapid_rearm_only_targets_the_latest_cue() {
    let rigs = [Rig::start(), Rig::start()];
    let t = TestServer::start("camera-now-cue");
    let api = t.client("operator");
    api.take_lease("Operator", false).unwrap();
    let mut project = with_cameras(&api, rigs.len());
    project.cameras[0].rig.fr7_absolute_moves = true;
    api.put_project(&project).unwrap();
    let project = connect(&api, &rigs);

    let initial = store(&api, "cam1", "Opening", Pose { pan: 40.0, ..Pose::home() });
    let missed = store(&api, "cam1", "Missed", Pose { pan: -40.0, ..Pose::home() });
    let newest = store(&api, "cam1", "Current", Pose { pan: 57.0, ..Pose::home() });
    let other = store(&api, "cam2", "Other", Pose { pan: -26.0, ..Pose::home() });
    let song = song_at(&api, &project, "Interrupted ride", [(15, &initial), (70, &missed), (150, &newest), (90, &other)]);
    let end = song.motion.iter().map(|clip| clip.end_frame()).max().unwrap();
    let tolerance = project.first().rig.tolerance;
    play_live(&api);
    let mid = wait_for(&api, |s| s.first().rig_pose.pan > 6.0 && rigs[0].is_moving());
    assert!(mid.first().rig_pose.pan < initial.pose.pan - 5.0, "not mid-move: {:?}", mid.first());
    assert!(rigs[0].fr7.commands().iter().any(|c| matches!(visca::parse_command(c), Some(visca::Command::Absolute { .. }))));

    let disarmed = api.set_camera_armed("cam1", false).unwrap();
    assert!(!disarmed.first().armed && !disarmed.first().catching_up);
    assert_playing(&disarmed);
    for _ in 0..8 {
        assert!(api.set_camera_armed("cam1", true).unwrap().first().armed);
        assert!(!api.set_camera_armed("cam1", false).unwrap().first().armed);
    }
    assert!(!api.set_camera_armed("cam1", false).unwrap().first().armed);
    wait_for(&api, |_| !rigs[0].is_moving());
    let parked = rigs[0].pose();
    assert!(parked.pan < initial.pose.pan - 5.0, "the pending absolute move finished anyway: {parked:?}");
    rigs[0].fr7.clear_seen();

    let state = wait_for(&api, |s| s.frame > end + 8);
    assert_playing(&state);
    assert!(state.frame > disarmed.frame && state.timecode != disarmed.timecode);
    assert_eq!(state.first().intended_pose.pan, newest.pose.pan);
    assert!(!state.first().armed && !state.first().manual && state.first().fault.is_none());
    assert_eq!(rigs[0].pose(), parked, "a pending command moved the disarmed head");
    assert!(close(&rigs[1].pose(), &other.pose, &tolerance, 0.5), "the other rig missed its cue");

    let armed = api.set_camera_armed("cam1", true).unwrap();
    assert!(armed.first().armed && armed.first().catching_up);
    assert_playing(&armed);
    let mut lowest = parked.pan;
    let state = wait_for(&api, |s| {
        lowest = lowest.min(rigs[0].pose().pan);
        !s.first().catching_up && close(&rigs[0].pose(), &newest.pose, &tolerance, 0.5)
    });
    assert_playing(&state);
    assert!(state.first().fault.is_none(), "{:?}", state.first());
    assert!(lowest >= parked.pan - 0.5, "the old negative cue was replayed: {lowest}, parked at {}", parked.pan);
    assert!(
        rigs[0].fr7.commands().iter().all(|c| !matches!(visca::parse_command(c), Some(visca::Command::Absolute { .. }))),
        "old preset moves returned instead of tracking the current pose"
    );

    // Disarming while the head is under hand control must leave the joystick in charge.
    let jog = Jog { pan: -0.4, ..Jog::default() };
    assert!(api.jog("cam1", &jog).unwrap().first().manual);
    let status = api.set_camera_armed("cam1", false).unwrap();
    assert!(status.first().manual && !status.first().armed);
    for _ in 0..12 {
        assert!(api.jog("cam1", &jog).unwrap().first().manual);
        std::thread::sleep(Duration::from_millis(35));
    }
    let released = api.jog("cam1", &Jog::default()).unwrap();
    assert!(!released.first().manual && !released.first().catching_up);
    wait_for(&api, |_| !rigs[0].is_moving());
    let manual_pose = rigs[0].pose();
    assert!(manual_pose.pan < newest.pose.pan - 3.0, "the jog did not move the real-rig adapter: {manual_pose:?}");
    let state = wait_for(&api, |s| s.frame > released.frame + 8);
    assert_playing(&state);
    assert_eq!(rigs[0].pose(), manual_pose, "releasing manual control rearmed a disarmed camera");
    api.set_camera_armed("cam1", true).unwrap();
    let state = wait_for(&api, |s| !s.first().catching_up && close(&rigs[0].pose(), &newest.pose, &tolerance, 0.5));
    assert_playing(&state);
}

#[test]
fn a_disconnected_camera_keeps_its_intended_pose_and_reconnects_without_stopping_the_show() {
    let rigs = [Rig::start(), Rig::start()];
    let t = TestServer::start("camera-now-reconnect");
    let api = t.client("operator");
    api.take_lease("Operator", false).unwrap();
    let mut project = connect(&api, &rigs);
    // A lost device is a per-camera failure even when the show holds all on an unsafe trajectory.
    project.on_fault = OnFault::HoldAll;
    let project = api.put_project(&project).unwrap();
    let initial = store(&api, "cam1", "Before cable loss", Pose { pan: 12.0, ..Pose::home() });
    let newest = store(&api, "cam1", "While offline", Pose { pan: 48.0, ..Pose::home() });
    let other = store(&api, "cam2", "Other", Pose { pan: -30.0, ..Pose::home() });
    let song = song_at(&api, &project, "Camera loss", [(10, &initial), (90, &newest), (75, &other)]);
    let end = song.motion.iter().map(|clip| clip.end_frame()).max().unwrap();
    let tolerance = project.first().rig.tolerance;
    play_live(&api);
    let state = wait_for(&api, |s| s.frame > 37 && close(&rigs[0].pose(), &initial.pose, &tolerance, 0.25));
    assert_playing(&state);
    rigs[0].fr7.set_silent(true);

    let lost = wait_for(&api, |s| !s.first().adapters[0].connected && s.first().fault.is_some());
    assert_playing(&lost);
    assert!(lost.first().armed, "the operator's arm intent survived the disconnect");
    assert!(lost.first().fault.as_deref().unwrap().contains("FR7"));
    for _ in 0..3 {
        for armed in [false, true] {
            let status = api.set_camera_armed("cam1", armed).unwrap();
            assert_playing(&status);
            let camera = status.first();
            assert_eq!(camera.armed, armed);
            assert!(camera.held && !camera.catching_up);
            assert_eq!(camera.fault, lost.first().fault);
        }
    }
    let state = wait_for(&api, |s| s.frame > end + 5);
    assert_playing(&state);
    assert_ne!(state.timecode, lost.timecode);
    assert_eq!(state.first().intended_pose.pan, newest.pose.pan);
    assert!((state.first().rig_pose.pan - initial.pose.pan).abs() < 1.0, "an unsent command changed the reported actual pose");
    assert!(state.camera("cam2").unwrap().fault.is_none());
    assert!(close(&rigs[1].pose(), &other.pose, &tolerance, 0.5), "the other rig missed its cue");
    assert_eq!(log(&api).iter().filter(|r| r.kind == "fault" && r.data["camera"] == "cam1").count(), 1);

    rigs[0].fr7.set_silent(false);
    let recovered = wait_for(&api, |s| {
        s.first().adapters[0].connected
            && !s.first().catching_up
            && s.first().fault.is_none()
            && close(&rigs[0].pose(), &newest.pose, &tolerance, 0.5)
    });
    assert_playing(&recovered);
    assert!(recovered.first().armed && recovered.frame > state.frame);

    // An explicit disarm while offline survives reconnection and still allows a later rearm.
    rigs[0].fr7.set_silent(true);
    wait_for(&api, |s| !s.first().adapters[0].connected);
    api.set_camera_armed("cam1", false).unwrap();
    rigs[0].fr7.place(25.0, 0.0, newest.pose.zoom, newest.pose.focus);
    rigs[0].fr7.set_silent(false);
    let connected = wait_for(&api, |s| s.first().adapters[0].connected);
    let state = wait_for(&api, |s| s.frame > connected.frame + 10);
    assert_playing(&state);
    assert!(!state.first().armed && !state.first().catching_up);
    assert!((rigs[0].pose().pan - 25.0).abs() < 0.2);

    rigs[0].fr7.set_speed_factor(0.0);
    api.set_camera_armed("cam1", true).unwrap();
    let stuck = wait_for(&api, |s| s.first().fault.as_deref().is_some_and(|f| f.contains("timed out")));
    assert_playing(&stuck);
    assert!(!stuck.first().catching_up);
    assert_eq!(stuck.first().intended_pose.pan, newest.pose.pan);
    assert!((stuck.first().rig_pose.pan - 25.0).abs() < 0.2, "the failed catch-up was reported as a move");
    assert!(stuck.camera("cam2").unwrap().fault.is_none());
    let faults = log(&api);
    let fault = faults.iter().rev().find(|r| r.kind == "fault" && r.data["camera"] == "cam1").unwrap();
    assert!((fault.data["pose"]["pan"].as_f64().unwrap() - 25.0).abs() < 0.2);
    assert_eq!(fault.data["intended_pose"]["pan"], newest.pose.pan);
    assert!(fault.data["commanded_at_read"]["pan"].as_f64().unwrap() > 27.0);

    api.set_camera_armed("cam1", false).unwrap();
    // A slow head still reporting forward travel must outlive the stall timeout.
    rigs[0].fr7.set_speed_factor(0.003);
    let rearmed = api.set_camera_armed("cam1", true).unwrap();
    let slow = wait_for(&api, |s| s.frame >= rearmed.frame + 60);
    assert_playing(&slow);
    assert!(slow.first().catching_up && slow.first().fault.is_none(), "{:?}", slow.first());
    assert!(slow.first().rig_pose.pan > 25.05 && slow.first().rig_pose.pan < 26.0);
    // A connected head can decline position inquiries while continuing to answer commands.
    rigs[0].fr7.refuse_position(true);
    let blind =
        wait_for(&api, |s| s.first().fault.as_deref().is_some_and(|why| why.contains("no fresh") && why.ends_with("feedback")));
    assert_playing(&blind);
    assert!(blind.first().held && blind.first().adapters[0].connected);
    assert!(blind.camera("cam2").unwrap().fault.is_none());
    api.set_camera_armed("cam1", false).unwrap();
    assert_eq!(api.set_camera_armed("cam1", true).unwrap().first().fault, blind.first().fault);
    rigs[0].fr7.refuse_position(false);
    rigs[0].fr7.set_speed_factor(1.0);
    let state = wait_for(&api, |s| !s.first().catching_up && close(&rigs[0].pose(), &newest.pose, &tolerance, 0.5));
    assert_playing(&state);
    assert!(state.first().fault.is_none());
}

#[test]
fn rearming_on_an_invalid_live_edited_preset_reports_the_fault_and_keeps_playing() {
    let t = TestServer::start("camera-now-invalid");
    let api = t.client("operator");
    api.take_lease("Operator", false).unwrap();
    let project = with_cameras(&api, 2);
    let invalid = store(&api, "cam1", "Out of range", Pose { pan: 220.0, ..Pose::home() });
    let other = store(&api, "cam2", "Other", Pose { pan: -25.0, ..Pose::home() });
    let song = song_at(&api, &project, "Bad cue on one camera", [(30, &other)]);
    let baseline = api.working(&song.id).unwrap();
    play_live(&api);
    let disarmed = api.set_camera_armed("cam1", false).unwrap();
    api.confirm_live_edit(&song.id).unwrap();
    let (edited, clip) = edit::insert_preset_move_timed(&song, &project, &invalid, 10, edit::PresetTiming::Seconds(0.8)).unwrap();
    let end = edited.motion_clip(&clip).unwrap().end_frame();
    api.put_working(&PutWorking { expected_counter: baseline.edit_counter, song: edited }).unwrap();
    wait_for(&api, |s| s.frame > end);
    api.set_camera_armed("cam1", true).unwrap();
    let bad = wait_for(&api, |s| s.first().fault.is_some());
    assert_playing(&bad);
    assert_eq!(bad.first().intended_pose.pan, invalid.pose.pan);
    assert_eq!(bad.first().rig_pose, disarmed.first().rig_pose, "an invalid target was sent to the rig");
    let st = wait_for(&api, |s| (s.camera("cam2").unwrap().rig_pose.pan - other.pose.pan).abs() < 0.5);
    assert_playing(&st);
    assert!(st.frame > bad.frame && st.camera("cam2").unwrap().fault.is_none());
    assert!(log(&api).iter().any(|r| r.kind == "fault" && r.data["camera"] == "cam1"));
}

#[test]
fn an_elevation_overrun_blocks_show_arm_live_edits_and_camera_rearm() {
    let t = TestServer::start("elevation-motion-limit");
    let api = t.client("operator");
    api.take_lease("Operator", false).unwrap();
    let project = api.project().unwrap();
    let high = store(&api, "cam1", "High", Pose { elevation: 200.0, ..Pose::home() });
    let song = song_at(&api, &project, "Elevation cue", []);
    let baseline = api.working(&song.id).unwrap();
    let (mut edited, clip) =
        edit::insert_preset_move_timed(&song, &project, &high, 25, edit::PresetTiming::Seconds(1.0)).unwrap();
    assert!(edit::accept_issues(&edited, &project, &clip, "Operator", "now").is_err());
    edited.motion_clip_mut(&clip).unwrap().accepted =
        Some(ptz_core::song::Accepted { by: "Operator".into(), at: "earlier".into() });

    play_live(&api);
    api.confirm_live_edit(&song.id).unwrap();
    let put = PutWorking { expected_counter: baseline.edit_counter, song: edited };
    assert!(refused(api.put_working(&put), 409).contains("Fix the elevation move"));
    assert_eq!(api.working(&song.id).unwrap().edit_counter, baseline.edit_counter);

    api.set_camera_armed("cam1", false).unwrap();
    api.put_working(&put).unwrap();
    assert!(refused(api.set_camera_armed("cam1", true), 409).contains("Fix the elevation move"));
    let live = api.status().unwrap();
    assert_playing(&live);
    assert!(!live.first().armed);

    do_act(&api, Action::Stop);
    do_act(&api, Action::Disarm);
    api.set_camera_armed("cam1", true).unwrap();
    api.save(&song.id).unwrap();
    api.commit(&song.id).unwrap();
    assert!(refused(api.act(Action::Arm, &TransportRequest::default()), 409).contains("timeline issue"));
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
fn simulated_cameras_follow_their_own_clips_and_are_jogged_stored_synced_and_recorded_when_moved() {
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

    // Record watches every camera and keeps a pass for the ones that are moved. Here that is the second.
    seek(&api, 300);
    api.act(
        Action::RecordOn,
        &TransportRequest { camera: Some("cam2".into()), axes: Some(vec![Axis::Tilt]), ..Default::default() },
    )
    .unwrap();
    let st = do_act(&api, Action::Play);
    assert_eq!((st.record.camera.as_str(), st.record.active), ("cam2", true));
    // Pan is not record-enabled, so pushing it drives nothing: the first camera has not been moved.
    api.jog("cam1", &Jog { pan: 0.5, ..Jog::default() }).unwrap();
    wait_for(&api, |s| s.frame >= 310);
    while api.status().unwrap().frame < 360 {
        let st = api.jog("cam2", &Jog { tilt: 0.3, pan: 1.0, ..Jog::default() }).unwrap();
        assert_eq!(st.camera("cam2").unwrap().pose_source, PoseSource::Record);
        assert_eq!(st.camera("cam1").unwrap().pose_source, PoseSource::Record);
        assert_eq!(st.record.moved, ["cam2"]);
        std::thread::sleep(Duration::from_millis(20));
    }
    api.jog("cam2", &Jog::default()).unwrap();
    wait_for(&api, |s| s.frame >= 380);
    let st = do_act(&api, Action::Stop);
    assert!(!st.transport.record, "Stop ends the pass");

    let after = api.working(&song.id).unwrap().song;
    let passes: Vec<_> = after.motion.iter().filter(|c| c.recorded_pass.is_some()).collect();
    assert_eq!(passes.len(), 1, "only the camera that was moved has a pass");
    let recorded = passes[0];
    assert_eq!((recorded.camera.as_str(), recorded.axes.as_slice()), ("cam2", &[Axis::Tilt][..]));
    for camera in ["cam1", "cam3"] {
        assert_eq!(clips_of(&after, camera), clips_of(&song, camera), "{camera} was not moved, so its clips are as they were");
    }
    assert!(pose_at(&after, "cam2", 365).tilt > pose_at(&song, "cam2", 365).tilt + 1.0, "the second camera's tilt was recorded");
    for f in (0..=after.length_frames).step_by(5) {
        assert_eq!(pose_at(&after, "cam1", f), pose_at(&song, "cam1", f), "the first camera's motion is untouched at {f}");
        assert_eq!(pose_at(&after, "cam2", f).pan, pose_at(&song, "cam2", f).pan, "pan was not record-enabled at {f}");
    }
    let logged: Vec<_> = log(&api).into_iter().filter(|r| r.kind == "record_pass").collect();
    assert_eq!(logged.len(), 1);
    assert_eq!(logged[0].data["camera"], "cam2");
}

fn record_on(api: &Api, axes: &[Axis]) -> Status {
    api.act(Action::RecordOn, &TransportRequest { axes: Some(axes.to_vec()), ..Default::default() }).unwrap()
}

fn passes_on(song: &Song) -> Vec<&str> {
    let mut cameras: Vec<&str> = song.motion.iter().filter(|c| c.recorded_pass.is_some()).map(|c| c.camera.as_str()).collect();
    cameras.sort();
    cameras
}

#[test]
fn a_pass_is_kept_for_each_camera_that_is_moved_and_for_no_other() {
    let t = TestServer::start_with("multicam-record", |cfg| cfg.seed = false);
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let project = with_cameras(&api, 3);
    let marks = [
        store(&api, "cam1", "Wide", Pose { pan: 10.0, ..Pose::home() }),
        store(&api, "cam2", "Keys", Pose { pan: -35.0, tilt: 4.0, ..Pose::home() }),
        store(&api, "cam3", "Drums", Pose { pan: 22.0, tilt: -6.0, ..Pose::home() }),
    ];
    let song = song_with_moves(&api, &project, "Three Hands", &marks);

    // The third camera is put somewhere by hand while stopped, and then left alone.
    seek(&api, 300);
    for _ in 0..20 {
        api.jog("cam3", &Jog { pan: 1.0, ..Jog::default() }).unwrap();
        std::thread::sleep(Duration::from_millis(25));
    }
    api.jog("cam3", &Jog::default()).unwrap();
    // It ramps down to a stop; where it comes to rest is where it was put.
    std::thread::sleep(Duration::from_secs(1));
    let parked = api.status().unwrap().camera("cam3").unwrap().rig_pose.pan;
    assert!(parked > 30.0, "the third camera was panned away from 22 degrees, to {parked}");

    // Two cameras are moved in one pass.
    record_on(&api, &[Axis::Pan, Axis::Tilt]);
    let st = do_act(&api, Action::Play);
    assert!(st.record.active && st.record.moved.is_empty(), "{:?}", st.record);
    wait_for(&api, |s| s.frame >= 310);
    while api.status().unwrap().frame < 360 {
        api.jog("cam1", &Jog { pan: 0.4, ..Jog::default() }).unwrap();
        api.jog("cam2", &Jog { tilt: 0.3, ..Jog::default() }).unwrap();
        std::thread::sleep(Duration::from_millis(20));
    }
    api.jog("cam1", &Jog::default()).unwrap();
    let st = api.jog("cam2", &Jog::default()).unwrap();
    assert_eq!(st.record.moved, ["cam1", "cam2"]);
    assert_eq!(st.camera("cam3").unwrap().rig_pose.pan, parked, "a camera put somewhere by hand stays there until it is moved");
    wait_for(&api, |s| s.frame >= 380);
    do_act(&api, Action::Stop);

    let after = api.working(&song.id).unwrap();
    assert_eq!(passes_on(&after.song), ["cam1", "cam2"]);
    assert!(pose_at(&after.song, "cam1", 365).pan > pose_at(&song, "cam1", 365).pan + 1.0, "the first camera's pan was recorded");
    assert!(
        pose_at(&after.song, "cam2", 365).tilt > pose_at(&song, "cam2", 365).tilt + 1.0,
        "the second camera's tilt was recorded"
    );
    // Until it was touched, a camera played the timeline, and that is what its pass holds there.
    assert_eq!(pose_at(&after.song, "cam1", 305), pose_at(&song, "cam1", 305));
    // The camera nobody moved during the pass is as it was: no clip, and not one frame of where it stood.
    assert_eq!(clips_of(&after.song, "cam3"), clips_of(&song, "cam3"));
    for f in 0..=after.song.length_frames {
        assert_eq!(pose_at(&after.song, "cam3", f), pose_at(&song, "cam3", f), "the third camera at frame {f}");
    }
    assert_eq!(api.status().unwrap().camera("cam3").unwrap().rig_pose.pan, 22.0, "and after the pass it is back on the timeline");
    assert_eq!(log(&api).iter().filter(|r| r.kind == "record_pass").count(), 2);

    // A pass in which nothing is moved leaves the song exactly as it was, and the log says why.
    seek(&api, 400);
    record_on(&api, &[Axis::Pan, Axis::Tilt]);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.frame >= 430);
    let st = do_act(&api, Action::Stop);
    assert!(!st.record.active && st.record.moved.is_empty());
    assert_eq!(api.working(&song.id).unwrap(), after);
    let last = log(&api).into_iter().rfind(|r| r.kind.starts_with("record_")).unwrap();
    assert_eq!(
        (last.kind.as_str(), &last.data["reason"]),
        ("record_discarded", &serde_json::json!("No camera was moved during the pass"))
    );
}

#[test]
fn a_head_that_stands_still_is_not_recorded_and_one_moved_by_its_own_controller_is() {
    let t = TestServer::start_with("multicam-record-heads", |cfg| cfg.seed = false);
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs: Vec<Rig> = (0..3).map(|_| Rig::start()).collect();
    // In Rehearse nothing is sent to a head, so each stands where it was left: nowhere near the timeline.
    for (n, rig) in rigs.iter().enumerate() {
        rig.fr7.place(60.0 + n as f64, -15.0, 0.1, 0.5);
        rig.pod.place(10.0);
    }
    let project = connect(&api, &rigs);
    let marks = [
        store(&api, "cam1", "Wide", Pose { pan: 10.0, ..Pose::home() }),
        store(&api, "cam2", "Keys", Pose { pan: -35.0, tilt: 4.0, ..Pose::home() }),
        store(&api, "cam3", "Drums", Pose { pan: 22.0, tilt: -6.0, ..Pose::home() }),
    ];
    let song = song_with_moves(&api, &project, "Three Heads", &marks);

    seek(&api, 300);
    record_on(&api, &[Axis::Pan, Axis::Tilt]);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.frame >= 310);
    // The second head is panned from its own controller.
    let mut pan = 61.0;
    while api.status().unwrap().frame < 360 {
        pan -= 0.25;
        rigs[1].fr7.place(pan, -15.0, 0.1, 0.5);
        std::thread::sleep(Duration::from_millis(20));
    }
    let st = wait_for(&api, |s| s.frame >= 380);
    assert_eq!(st.record.moved, ["cam2"]);
    do_act(&api, Action::Stop);

    let after = api.working(&song.id).unwrap().song;
    assert_eq!(passes_on(&after), ["cam2"]);
    let travelled = pose_at(&after, "cam2", 305).pan - pose_at(&after, "cam2", 375).pan;
    assert!(travelled > 5.0, "the pass holds the pan the head made: {travelled} degrees");
    for camera in ["cam1", "cam3"] {
        assert_eq!(clips_of(&after, camera), clips_of(&song, camera), "{camera} stood still, so its clips are as they were");
    }
    assert!(rigs.iter().all(|r| r.fr7.commands().is_empty()), "recording sends a head nothing");
}

#[test]
fn a_column_is_recorded_with_no_height_map_entered_and_one_map_serves_every_camera() {
    let t = TestServer::start_with("multicam-record-column", |cfg| cfg.seed = false);
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs: Vec<Rig> = (0..2).map(|_| Rig::start()).collect();
    for rig in &rigs {
        rig.fr7.place(0.0, 0.0, 0.1, 0.5);
        rig.pod.place(10.0);
    }
    let mut project = connect(&api, &rigs);
    let answering = |s: &Status| s.cameras.iter().all(|c| c.adapters.len() == 2 && c.adapters.iter().all(|a| a.connected));

    // No height map anywhere: nothing was measured.
    for camera in &mut project.cameras {
        camera.rig.autopod_calibration.clear();
    }
    let mut project = api.put_project(&project).unwrap();
    wait_for(&api, answering);
    for rig in &rigs {
        rig.pod.settle_after_connect();
    }
    let marks = [
        store(&api, "cam1", "Wide", Pose { pan: 10.0, ..Pose::home() }),
        store(&api, "cam2", "Keys", Pose { pan: -35.0, ..Pose::home() }),
    ];
    let song = song_with_moves(&api, &project, "Two Columns", &marks);

    seek(&api, 300);
    record_on(&api, &[Axis::Elevation]);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.frame >= 310);
    // The second column is raised from its own controller.
    let mut mm = 10.0;
    while api.status().unwrap().frame < 360 {
        mm += 4.0;
        rigs[1].pod.place(mm);
        std::thread::sleep(Duration::from_millis(20));
    }
    let st = wait_for(&api, |s| s.frame >= 380);
    assert_eq!(st.record.moved, ["cam2"]);
    do_act(&api, Action::Stop);
    let after = api.working(&song.id).unwrap().song;
    assert_eq!(passes_on(&after), ["cam2"]);
    let rose = pose_at(&after, "cam2", 375).elevation - pose_at(&after, "cam2", 305).elevation;
    assert!(rose > 20.0, "the pass holds the rise the column made, on the assumed scale: {rose}");
    assert!(rigs.iter().all(|r| r.pod.orders().is_empty()), "recording sends a column nothing");

    // One map in the project is every camera's, and with it the heights are the measured ones.
    project.autopod_calibration = FakeAutoPod::height_map();
    api.put_project(&project).unwrap();
    wait_for(&api, answering);
    let measured = rigs[1].pod.height_mm();
    let st = wait_for(&api, |s| answering(s) && (s.cameras[1].rig_pose.elevation - measured).abs() < 1.0);
    assert!((st.cameras[0].rig_pose.elevation - 10.0).abs() < 1.0);
}

#[test]
fn columns_measured_from_the_floor_arm_and_stay_where_they_stand_in_a_song_with_no_elevation() {
    let t = TestServer::start_with("multicam-floor-heights", |cfg| cfg.seed = false);
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs: Vec<Rig> = (0..2).map(|_| Rig::start()).collect();
    // The show's height map: 52.25, 75.35 and 104 inches off the floor, the raw position
    // falling as the column rises, and one map for every column.
    let inch = 25.4;
    let map = [(4215.0, 104.0 * inch), (37059.0, 75.35 * inch), (65220.0, 52.25 * inch)];
    let per_mm = (4215.0 - 65220.0) / ((104.0 - 52.25) * inch);
    // The first column stands fully down and reads a little below the lowest height that was
    // measured; the second stands 33 mm up.
    rigs[0].pod.set_raw_scale(65514.0, per_mm);
    rigs[1].pod.set_raw_scale(65220.0, per_mm);
    rigs[1].pod.place(33.0);
    for rig in &rigs {
        rig.fr7.place(-20.0, -5.0, 0.3, 0.2);
    }
    let mut project = connect(&api, &rigs);
    for camera in &mut project.cameras {
        camera.rig.autopod_calibration.clear();
    }
    project.autopod_calibration = map.iter().map(|&(raw, mm)| ptz_core::pose::CalPoint { raw, mm }).collect();
    let project = api.put_project(&project).unwrap();
    let default = ptz_core::pose::AxisTable::default().elevation;
    assert!(project.cameras.iter().all(|c| (c.axes.elevation.min, c.axes.elevation.max) == (default.min, default.max)));
    let st = wait_for(&api, |s| {
        s.cameras.iter().all(|c| c.adapters.len() == 2 && c.adapters.iter().all(|a| a.connected) && c.rig_pose.elevation > 1000.0)
    });
    let stood: Vec<f64> = st.cameras.iter().map(|c| c.rig_pose.elevation).collect();
    assert!((stood[0] - 1321.0).abs() < 1.0 && (stood[1] - 1360.0).abs() < 1.5, "{stood:?}");
    for rig in &rigs {
        rig.pod.settle_after_connect();
    }

    // A song that pans each camera and says nothing about height.
    let marks = [
        store(&api, "cam1", "Wide", Pose { pan: 10.0, ..Pose::home() }),
        store(&api, "cam2", "Keys", Pose { pan: -35.0, ..Pose::home() }),
    ];
    let created = api.new_song("No Heights", None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let mut song = created.song;
    for mark in &marks {
        song = edit::insert_preset_move(&song, &project, mark, 25).unwrap().0;
    }
    for clip in &mut song.motion {
        clip.axes.retain(|a| *a != Axis::Elevation);
    }
    api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    let st = api.status().unwrap();
    assert_eq!(st.song.as_ref().unwrap().blocking, 0, "a column the song does not move is not judged at the song's home height");

    // Arm: neither column is held for standing outside 0 to 670 mm, and neither is moved.
    do_act(&api, Action::Arm);
    let st = wait_for(&api, |s| s.transport.output == Output::Armed || s.telemetry.fault.is_some());
    assert_eq!(st.telemetry.fault, None);
    assert!(st.cameras.iter().all(|c| !c.held && c.fault.is_none()), "{:?}", st.cameras);
    do_act(&api, Action::Play);
    let st = wait_for(&api, |s| s.frame > 120 || s.telemetry.fault.is_some());
    assert_eq!(st.telemetry.fault, None);
    assert!(st.cameras.iter().all(|c| !c.held && c.fault.is_none()), "{:?}", st.cameras);
    assert_eq!(st.transport.output, Output::Armed);
    do_act(&api, Action::Stop);
    assert!((rigs[0].fr7.pose().0 - 10.0).abs() < 1.0 && (rigs[1].fr7.pose().0 + 35.0).abs() < 1.0, "the heads made their moves");
    for (rig, at) in rigs.iter().zip([0.0, 33.0]) {
        assert_eq!(rig.pod.height_mm(), at, "the column stands where it stood");
        assert!(rig.pod.orders().iter().all(|c| c == "#PTS5050"), "a column is sent no speed: {:?}", rig.pod.orders());
    }

    // The travel a column is held to is what was measured on it, so a height from before the
    // map, on the assumed 0 to 670 mm scale, is reported and not played.
    do_act(&api, Action::Disarm);
    let mut song = api.working(&id).unwrap().song;
    let low = store(&api, "cam1", "Low", Pose { pan: 10.0, elevation: 400.0, ..Pose::home() });
    song = edit::insert_preset_move(&song, &project, &low, 200).unwrap().0;
    let counter = api.working(&id).unwrap().edit_counter;
    let working = api.put_working(&PutWorking { expected_counter: counter, song }).unwrap();
    assert!(
        working.issues.iter().any(|i| matches!(i, Issue::OutOfRange { camera, axis: Axis::Elevation, .. } if camera == "cam1")),
        "{:?}",
        working.issues
    );
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

    // Arm: each camera goes to its get-set target (stay put, or fade to rest when the first
    // move is near). The show is armed when the last one is there.
    let st = do_act(&api, Action::Arm);
    assert_eq!(st.transport.output, Output::Positioning);
    let st = wait_for(&api, |s| s.transport.output == Output::Armed || s.telemetry.fault.is_some());
    assert_eq!(st.telemetry.fault, None);
    assert_eq!(st.transport.output, Output::Armed);
    assert!(st.cameras.iter().all(|c| !c.held && c.fault.is_none()), "{:?}", st.cameras);
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

    // Losing every connection still leaves the show playing. Each camera reports its own failure.
    do_act(&api, Action::Play);
    rigs[0].pod.set_silent(true);
    wait_for(&api, |s| s.camera("cam1").unwrap().held);
    rigs[1].fr7.set_silent(true);
    wait_for(&api, |s| s.camera("cam2").unwrap().held);
    assert_eq!(api.status().unwrap().transport.output, Output::Armed);
    rigs[2].fr7.set_silent(true);
    let st = wait_for(&api, |s| s.cameras.iter().all(|c| c.fault.is_some() && c.held));
    assert_playing(&st);
    assert!(st.first().fault.as_deref().unwrap().contains("AutoPod+"));
    assert_playing(&wait_for(&api, |s| s.frame > st.frame + 10));
    rigs[0].pod.set_silent(false);
    rigs[1].fr7.set_silent(false);
    rigs[2].fr7.set_silent(false);
    wait_for(&api, |s| s.adapters().all(|a| a.connected) && s.cameras.iter().all(|c| !c.held));
    do_act(&api, Action::Stop);
    do_act(&api, Action::Disarm);
    seek(&api, 0);

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
