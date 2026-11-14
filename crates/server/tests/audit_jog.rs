//! Audit (scratch): the jog deadman is per camera.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::camera::CameraConfig;
use std::time::{Duration, Instant};

#[test]
fn a_camera_stops_within_the_deadman_when_the_station_goes_on_to_jog_another() {
    let t = TestServer::start("audit-jog");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let mut project = api.project().unwrap();
    let id = project.free_camera_id();
    project.cameras.push(CameraConfig::new(&id, "Camera 2"));
    let project = api.put_project(&project).unwrap();
    let (one, two) = (project.cameras[0].id.clone(), project.cameras[1].id.clone());
    let pan = |api: &ptz_core::client::Api, i: usize| api.status().unwrap().cameras[i].rig_pose.pan;

    let full = Jog { pan: 1.0, ..Jog::default() };
    for _ in 0..4 {
        api.jog(&one, &full).unwrap();
        std::thread::sleep(Duration::from_millis(40));
    }
    let last_for_one = Instant::now();
    assert!(pan(&api, 0) > 0.5, "camera 1 is being jogged");

    // The station moves on to camera 2 and never says another word about camera 1.
    let mut samples = Vec::new();
    while last_for_one.elapsed() < Duration::from_millis(1200) {
        api.jog(&two, &full).unwrap();
        samples.push((last_for_one.elapsed(), pan(&api, 0)));
        std::thread::sleep(Duration::from_millis(20));
    }
    let end = samples.last().unwrap().1;
    let stopped_at = samples.iter().find(|(_, p)| (*p - end).abs() < 1e-9).unwrap().0;
    println!("MEASURE camera 1 stood still {stopped_at:?} after its last jog message; camera 2 is at {:.1}", pan(&api, 1));
    assert!(stopped_at < Duration::from_millis(330), "camera 1 ran on for {stopped_at:?}");
    assert!(pan(&api, 1) > 20.0, "camera 2 is the one moving");
}
