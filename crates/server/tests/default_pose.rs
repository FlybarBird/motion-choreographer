//! A camera's stored default is where a new song starts that camera.
//! Songs that already exist, and a camera with no default, stay as they were.

mod common;

use common::*;
use ptz_core::api::StoreDefault;
use ptz_core::camera::CameraConfig;
use ptz_core::eval::pose_at;
use ptz_core::pose::{Pose, RigKind};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::path::Path;

fn file(dir: &Path, id: &str) -> Vec<u8> {
    std::fs::read(dir.join(format!("songs/{id}/working.json"))).unwrap()
}

#[test]
fn a_new_song_starts_at_each_cameras_stored_default_and_an_old_song_does_not() {
    let t = TestServer::start("default-pose");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();

    let walk_in = file(&t.dir, "walk-in");
    let mut project = api.project().unwrap();
    project.cameras.push(CameraConfig::new("cam2", "Centre"));
    api.put_project(&project).unwrap();

    let before = api.new_song("Before", None).unwrap().song;
    assert!(before.camera_homes.is_empty());
    assert_eq!(pose_at(&before, "cam1", 0), Pose::home());
    assert_eq!(pose_at(&before, "cam2", 0), Pose::home());
    let before_file = file(&t.dir, &before.id);

    let pose_a = Pose { pan: 18.5, tilt: -6.0, zoom: 0.5, focus: 0.25, elevation: 240.0 };
    let watcher = t.client("b");
    let no_lease = StoreDefault { camera: Some("cam1".into()), pose: Some(pose_a) };
    assert_eq!(refused(watcher.store_default(&no_lease), 409), "Take the edit lease to control the show");
    let missing = StoreDefault { camera: Some("cam9".into()), pose: Some(pose_a) };
    assert_eq!(refused(api.store_default(&missing), 404), "No camera with id cam9");
    let sim = StoreDefault { camera: Some("cam1".into()), pose: Some(pose_a) };
    assert_eq!(refused(api.store_default(&sim), 409), "Set this camera to a live FR7 to store its default");
    assert!(api.project().unwrap().cameras.iter().all(|c| c.default_pose.is_none()));

    let mut project = api.project().unwrap();
    project.cameras[0].default_pose = Some(pose_a);
    let project = api.put_project(&project).unwrap();
    assert_eq!(project.cameras[0].default_pose, Some(pose_a));
    assert_eq!(project.cameras[1].default_pose, None);
    // Saving the rest of the project does not drop the default.
    let mut renamed = project;
    renamed.cameras[0].name = "Stage left".into();
    let project = api.put_project(&renamed).unwrap();
    assert_eq!(project.cameras[0].default_pose, Some(pose_a));
    assert_eq!(project.camera("cam2").unwrap().default_pose, None);

    let after = api.new_song("After", None).unwrap().song;
    assert_eq!(pose_at(&after, "cam1", 0), pose_a);
    assert_eq!(pose_at(&after, "cam1", after.length_frames), pose_a);
    assert_eq!(pose_at(&after, "cam2", 0), Pose::home(), "a camera with no default stays at home");
    assert_eq!(after.home_pose, Pose::home());
    assert_eq!(after.camera_homes.get("cam1").copied(), Some(pose_a));
    assert!(!after.camera_homes.contains_key("cam2"));

    api.load_song(&after.id).unwrap();
    let st = wait_for(&api, |s| s.camera("cam1").unwrap().rig_pose == pose_a);
    assert_eq!(st.camera("cam2").unwrap().rig_pose, Pose::home());

    // Storing over replaces the default. The song already made keeps the first one.
    let pose_b = Pose { pan: -4.0, tilt: 2.5, zoom: 0.25, focus: 0.75, elevation: 80.0 };
    let mut project = api.project().unwrap();
    project.cameras[0].default_pose = Some(pose_b);
    api.put_project(&project).unwrap();
    assert_eq!(api.project().unwrap().cameras[0].default_pose, Some(pose_b));
    assert_eq!(api.working(&before.id).unwrap().song, before);
    assert_eq!(api.working(&after.id).unwrap().song, after);
    assert_eq!(api.status().unwrap().camera("cam1").unwrap().rig_pose, pose_a, "the loaded song was not moved");
    assert_eq!(file(&t.dir, &before.id), before_file);
    assert_eq!(file(&t.dir, "walk-in"), walk_in);

    let later = api.new_song("Later", None).unwrap().song;
    assert_eq!(pose_at(&later, "cam1", 0), pose_b);
    assert_eq!(pose_at(&later, "cam2", 40), Pose::home());
    api.load_song(&later.id).unwrap();
    let st = wait_for(&api, |s| s.camera("cam1").unwrap().rig_pose == pose_b);
    assert_eq!(st.camera("cam2").unwrap().rig_pose, Pose::home());
    let copied = api.copy_song(&after.id, "After copy").unwrap().song;
    assert_eq!(pose_at(&copied, "cam1", 0), pose_a, "a copy keeps the song it was copied from");
    let copied_old = api.copy_song(&before.id, "Before copy").unwrap().song;
    assert_eq!(pose_at(&copied_old, "cam1", 0), Pose::home());
    assert!(copied_old.camera_homes.is_empty());

    let dir = t.stop_keeping_data();
    let t = TestServer::reopen(&dir);
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    assert_eq!(api.project().unwrap().cameras[0].default_pose, Some(pose_b));
    let again = api.new_song("After restart", None).unwrap().song;
    assert_eq!(pose_at(&again, "cam1", 0), pose_b);
    assert_eq!(pose_at(&again, "cam2", 0), Pose::home());
    assert_eq!(api.working(&before.id).unwrap().song.camera_homes, before.camera_homes);
    assert_eq!(pose_at(&api.working(&after.id).unwrap().song, "cam1", 0), pose_a);
}

#[test]
fn store_default_reads_the_selected_live_camera_without_moving_it() {
    let t = TestServer::start_with("live-default", |cfg| cfg.seed = false);
    let api = t.client("a");
    api.take_lease("Desk", false).unwrap();
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    let mut project = api.project().unwrap();
    let mut camera = CameraConfig::new("cam2", "Stage left");
    camera.rig.kind = RigKind::Hardware;
    camera.rig.autopod_host = pod.addr.to_string();
    camera.rig.autopod_calibration = FakeAutoPod::height_map();
    project.cameras.push(camera);
    api.put_project(&project).unwrap();

    // Older desks send their last displayed pose. A column without a head cannot supply PTZ.
    let req = StoreDefault { camera: Some("cam2".into()), pose: Some(Pose::home()) };
    assert!(refused(api.store_default(&req), 409).contains("FR7 address"));
    let rig = &mut project.cameras[1].rig;
    rig.fr7_host = fr7.visca.to_string();
    rig.fr7_http_port = fr7.http.port();
    rig.fr7_user = fr7.user.clone();
    rig.fr7_password = fr7.password.clone();
    api.put_project(&project).unwrap();
    wait_for(&api, |s| {
        let c = s.camera("cam2").unwrap();
        c.adapters.len() == 2 && c.adapters.iter().all(|a| a.connected)
    });
    fr7.settle_after_connect();
    pod.settle_after_connect();
    let before = api.new_song("Before", None).unwrap().song;
    api.load_song(&before.id).unwrap();
    fr7.place(25.0, -6.0, 0.4, 0.75);
    pod.place(220.0);
    let st = wait_for(&api, |s| {
        let p = s.camera("cam2").unwrap().rig_pose;
        (p.pan - 25.0).abs() < 0.2
            && (p.tilt + 6.0).abs() < 0.2
            && (p.zoom - 0.4).abs() < 0.01
            && (p.focus - 0.75).abs() < 0.01
            && (p.elevation - 220.0).abs() < 2.0
    });
    let selected = st.camera("cam2").unwrap();
    assert_ne!(selected.pose, selected.rig_pose, "the rehearsal monitor has a different pose");
    let project = api.store_default(&req).unwrap();
    let stored = project.camera("cam2").unwrap().default_pose.unwrap();
    assert_eq!(stored, selected.rig_pose);
    assert!(project.camera("cam1").unwrap().default_pose.is_none());
    assert_eq!(api.working(&before.id).unwrap().song.rest_pose("cam2"), Pose::home());
    let after = api.new_song("After", None).unwrap().song;
    assert_eq!(after.rest_pose("cam2"), stored);
    assert!(fr7.commands().is_empty(), "capture sent a camera movement command");
    assert!(pod.orders().is_empty(), "capture sent a column movement command");

    // Native preset moves can refuse position inquiries even with a healthy connection.
    fr7.refuse_position(true);
    wait_for(&api, |s| s.camera("cam2").unwrap().adapters[0].detail.contains("will answer when it arrives"));
    assert!(refused(api.store_default(&req), 409).contains("current position"));
    fr7.refuse_position(false);
    pod.set_silent(true);
    wait_for(&api, |s| !s.camera("cam2").unwrap().adapters[1].connected);
    assert!(refused(api.store_default(&req), 409).contains("AutoPod+"));
    assert_eq!(api.project().unwrap().camera("cam2").unwrap().default_pose, Some(stored));

    // An FR7 with no column supplies every camera axis itself.
    let mut project = api.project().unwrap();
    project.cameras[1].rig.autopod_host.clear();
    api.put_project(&project).unwrap();
    wait_for(&api, |s| {
        let c = s.camera("cam2").unwrap();
        c.adapters.len() == 1 && c.adapters[0].connected
    });
    fr7.settle_after_connect();
    fr7.place(-40.0, 3.0, 0.1, 0.25);
    let st = wait_for(&api, |s| {
        let p = s.camera("cam2").unwrap().rig_pose;
        (p.pan + 40.0).abs() < 0.2 && (p.zoom - 0.1).abs() < 0.01 && (p.focus - 0.25).abs() < 0.01
    });
    let req = StoreDefault { camera: Some("cam2".into()), pose: None };
    let head_only = api.store_default(&req).unwrap().camera("cam2").unwrap().default_pose.unwrap();
    assert_eq!(head_only, Pose { elevation: Pose::home().elevation, ..st.camera("cam2").unwrap().rig_pose });
    fr7.set_silent(true);
    wait_for(&api, |s| !s.camera("cam2").unwrap().adapters[0].connected);
    assert!(refused(api.store_default(&req), 409).contains("FR7"));
    assert_eq!(api.project().unwrap().camera("cam2").unwrap().default_pose, Some(head_only));
}
