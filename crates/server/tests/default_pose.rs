//! A camera's stored default is where a new song starts that camera.
//! Songs that already exist, and a camera with no default, stay as they were.

mod common;

use common::*;
use ptz_core::api::StoreDefault;
use ptz_core::camera::CameraConfig;
use ptz_core::eval::pose_at;
use ptz_core::pose::Pose;
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
    let nan = StoreDefault { camera: Some("cam1".into()), pose: Some(Pose { pan: f64::NAN, ..pose_a }) };
    assert_eq!(refused(api.store_default(&nan), 400), "That position is not a usable number");
    assert!(api.project().unwrap().cameras.iter().all(|c| c.default_pose.is_none()));

    let project = api.store_default(&StoreDefault { camera: Some("cam1".into()), pose: Some(pose_a) }).unwrap();
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
    api.store_default(&StoreDefault { camera: Some("cam1".into()), pose: Some(pose_b) }).unwrap();
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
