//! Capturing where a rig stands as a preset to program to: a preset the camera has already when
//! the pose is within the project's range of one, otherwise a new one.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::camera::CameraConfig;
use ptz_core::client::Api;
use ptz_core::pose::{Pose, Preset, RigKind};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::time::Duration;

fn capture(api: &Api, camera: &str) -> Captured {
    api.post("/presets/capture", &CapturePreset { camera: Some(camera.into()), name: None }).unwrap()
}

fn names(api: &Api, camera: &str) -> Vec<String> {
    api.presets().unwrap().into_iter().filter(|p| p.camera == camera).map(|p| p.name).collect()
}

/// Drive a simulated camera to a pan, as a station's hand control does, and wait for it to stand there.
fn jog_to(api: &Api, camera: &str, pan: f64) -> Pose {
    let at = |api: &Api| api.status().unwrap().camera(camera).unwrap().pose;
    for _ in 0..400 {
        let off = pan - at(api).pan;
        if off.abs() < 0.02 {
            break;
        }
        api.jog(camera, &Jog { pan: (off / 20.0).clamp(-0.5, 0.5), ..Jog::default() }).unwrap();
        std::thread::sleep(Duration::from_millis(15));
    }
    api.jog(camera, &Jog::default()).unwrap();
    let mut last = at(api);
    loop {
        std::thread::sleep(Duration::from_millis(60));
        let now = at(api);
        if now == last {
            return now;
        }
        last = now;
    }
}

#[test]
fn a_capture_uses_a_preset_within_range_and_stores_a_new_one_otherwise() {
    let t = TestServer::start_with("capture-sim", |cfg| cfg.seed = false);
    let api = t.client("a");
    let watcher = t.client("b");
    assert_eq!(
        refused(watcher.post::<_, Captured>("/presets/capture", &CapturePreset::default()), 409),
        "Take the edit lease to control the show"
    );
    api.take_lease("Luke-MacBook", false).unwrap();
    let mut project = api.project().unwrap();
    project.cameras.push(CameraConfig::new("cam2", "Stage left"));
    let mut project = api.put_project(&project).unwrap();
    assert_eq!(
        refused(api.post::<_, Captured>("/presets/capture", &CapturePreset { camera: Some("cam9".into()), name: None }), 404),
        "No camera with id cam9"
    );

    // Nothing stored yet: the pose is a new preset, named for the camera.
    let here = jog_to(&api, "cam2", 12.0);
    let first = capture(&api, "cam2");
    assert!(!first.reused);
    assert_eq!((first.preset.name.as_str(), first.preset.camera.as_str()), ("Preset 1", "cam2"));
    assert_eq!((first.pose, first.preset.pose), (here, here));

    // Asked again where it stands, and again half a degree on: the same preset, and nothing stored.
    assert_eq!(capture(&api, "cam2"), Captured { preset: first.preset.clone(), reused: true, pose: here });
    let near = jog_to(&api, "cam2", 12.5);
    let again = capture(&api, "cam2");
    assert!(again.reused);
    assert_eq!(again.preset, first.preset, "the move goes to the pose the preset holds");
    assert_eq!(again.pose, near);
    assert_eq!(names(&api, "cam2"), ["Preset 1"]);

    // Past the range it is another framing.
    let far = jog_to(&api, "cam2", 15.0);
    let second = capture(&api, "cam2");
    assert!(!second.reused);
    assert_eq!((second.preset.name.as_str(), second.preset.pose), ("Preset 2", far));
    // Between two presets in range, the nearer one.
    project.preset_match.range.pan = 3.0;
    let mut project = api.put_project(&project).unwrap();
    jog_to(&api, "cam2", 14.0);
    assert_eq!(capture(&api, "cam2").preset.id, second.preset.id);
    jog_to(&api, "cam2", 13.0);
    assert_eq!(capture(&api, "cam2").preset.id, first.preset.id);

    // A preset is its camera's: the same pose on another camera is new there.
    jog_to(&api, "cam1", 13.0);
    let other = capture(&api, "cam1");
    assert!(!other.reused);
    assert_eq!((other.preset.name.as_str(), other.preset.camera.as_str()), ("Preset 1", "cam1"));
    assert_ne!(other.preset.id, first.preset.id);

    // Switched off in Show settings, every capture is a new preset. A name can be given.
    project.preset_match.enabled = false;
    api.put_project(&project).unwrap();
    let third = capture(&api, "cam2");
    assert!(!third.reused);
    assert_eq!(third.preset.name, "Preset 3");
    let named: Captured =
        api.post("/presets/capture", &CapturePreset { camera: Some("cam2".into()), name: Some(" Chorus wide ".into()) }).unwrap();
    assert_eq!(named.preset.name, "Chorus wide");
    assert_eq!(names(&api, "cam2"), ["Preset 1", "Preset 2", "Preset 3", "Chorus wide"]);

    // What was stored is there after a restart.
    let dir = t.stop_keeping_data();
    let t = TestServer::reopen(&dir);
    let stored: Vec<Preset> = t.client("a").presets().unwrap();
    assert_eq!(stored.len(), 5);
    assert!(stored.contains(&second.preset));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_capture_reads_a_real_rig_where_another_controller_left_it_and_moves_nothing() {
    let t = TestServer::start_with("capture-rig", |cfg| cfg.seed = false);
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    let mut project = api.project().unwrap();
    let rig = &mut project.cameras[0].rig;
    rig.kind = RigKind::Hardware;
    rig.fr7_host = fr7.visca.to_string();
    rig.fr7_http_port = fr7.http.port();
    rig.fr7_user = fr7.user.clone();
    rig.fr7_password = fr7.password.clone();
    rig.autopod_host = pod.addr.to_string();
    rig.autopod_calibration = FakeAutoPod::height_map();
    api.put_project(&project).unwrap();
    wait_for(&api, |s| {
        let c = s.first();
        c.hardware && c.adapters.len() == 2 && c.adapters.iter().all(|a| a.connected) && c.adapters[1].calibrated == Some(true)
    });
    fr7.settle_after_connect();
    pod.settle_after_connect();

    // The operator's own controller puts the head and the column somewhere. The server only watches.
    fr7.place(25.0, -6.0, 0.4, 0.5);
    pod.place(220.0);
    let near = |a: &Pose, b: &Pose| {
        (a.pan - b.pan).abs() < 0.2
            && (a.tilt - b.tilt).abs() < 0.2
            && (a.zoom - b.zoom).abs() < 0.01
            && (a.elevation - b.elevation).abs() < 2.0
    };
    let there = Pose { pan: 25.0, tilt: -6.0, zoom: 0.4, focus: 0.5, elevation: 220.0 };
    wait_for(&api, |s| near(&s.first().rig_pose, &there));
    let first = capture(&api, "cam1");
    assert!(!first.reused && near(&first.preset.pose, &there), "{first:?}");

    // Somewhere else, then back by hand to nearly the same place: the preset it already has.
    fr7.place(-40.0, 3.0, 0.1, 0.5);
    pod.place(60.0);
    wait_for(&api, |s| (s.first().rig_pose.pan + 40.0).abs() < 0.2 && (s.first().rig_pose.elevation - 60.0).abs() < 2.0);
    let second = capture(&api, "cam1");
    assert!(!second.reused);
    fr7.place(25.4, -6.3, 0.41, 0.5);
    pod.place(224.0);
    wait_for(&api, |s| (s.first().rig_pose.pan - 25.4).abs() < 0.2 && (s.first().rig_pose.elevation - 224.0).abs() < 2.0);
    let back = capture(&api, "cam1");
    assert!(back.reused, "{back:?}");
    assert_eq!(back.preset, first.preset);
    assert_eq!(names(&api, "cam1"), ["Preset 1", "Preset 2"]);

    // Capturing told the rig nothing: no move, no stop, no preset recall.
    assert!(!fr7.is_moving() && !pod.is_moving());
    assert!(fr7.commands().is_empty(), "the head was commanded: {:02x?}", fr7.commands());
    assert!(pod.orders().is_empty(), "the column was commanded: {:?}", pod.orders());

    // A head that has stopped answering is not read: its last position may be old.
    fr7.set_silent(true);
    wait_for(&api, |s| s.first().adapters.iter().any(|a| !a.connected));
    let why = refused(api.post::<_, Captured>("/presets/capture", &CapturePreset::default()), 409);
    assert!(why.starts_with("The rig's pose is not known: "), "{why}");
    assert_eq!(names(&api, "cam1").len(), 2);
}
