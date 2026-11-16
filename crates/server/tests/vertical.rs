//! Vertical PTZ poses travel through editing, validation and the simulated transport unchanged.

mod common;

use common::*;
use ptz_core::api::{PoseSource, PutWorking, StorePreset, TransportRequest};
use ptz_core::camera::CameraConfig;
use ptz_core::compile::Issue;
use ptz_core::edit;
use ptz_core::pose::{Axis, Pose, Preset};
use ptz_core::transport::{Action, Output};

#[test]
fn upright_and_inverted_cameras_reach_both_vertical_presets_with_configured_limits() {
    let t = TestServer::start("vertical-ptz");
    let api = t.client("vertical-editor");
    api.take_lease("Operator", false).unwrap();
    let mut project = api.project().unwrap();
    project.cameras.push(CameraConfig::new("cam2", "Ceiling"));
    for (index, camera) in project.cameras.iter_mut().enumerate() {
        camera.axes.tilt.min = -90.0;
        camera.axes.tilt.max = 90.0;
        camera.mount.z = if index == 0 { 0.0 } else { 6.0 };
        camera.mount.inverted = index == 1;
    }
    let project = api.put_project(&project).unwrap();
    let created = api.new_song("Vertical moves", None).unwrap();
    let id = created.song.id.clone();
    let mut song = created.song;
    let mut endpoints = Vec::new();
    for camera in &project.cameras {
        let mut start = 25;
        for tilt in [90.0, -90.0] {
            let preset: Preset = api
                .post(
                    "/presets",
                    &StorePreset {
                        name: format!("{} at {tilt}", camera.name),
                        pose: Some(Pose { tilt, ..Pose::home() }),
                        camera: Some(camera.id.clone()),
                    },
                )
                .unwrap();
            let (next, clip_id) = edit::insert_preset_move(&song, &project, &preset, start).unwrap();
            song = next;
            let clip = song.motion_clip(&clip_id).unwrap();
            let end = clip.start_frame + clip.duration_frames;
            endpoints.push((camera.id.clone(), end, tilt));
            start = end + 25;
        }
    }
    let working = api.put_working(&PutWorking { expected_counter: created.edit_counter, song }).unwrap();
    assert!(working.issues.is_empty(), "{:?}", working.issues);
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    api.load_song(&id).unwrap();

    for (camera_id, frame, tilt) in endpoints {
        let status = api.act(Action::Seek, &TransportRequest { frame: Some(frame), ..Default::default() }).unwrap();
        let camera = status.camera(&camera_id).unwrap();
        assert_eq!(camera.pose_source, PoseSource::Timeline);
        assert_eq!(camera.pose.tilt, tilt);
        let config = project.camera(&camera_id).unwrap();
        assert_eq!(config.mount.view(&camera.pose).1, tilt);
        assert!(status.cameras.iter().all(|camera| Axis::ALL.into_iter().all(|axis| camera.pose.get(axis).is_finite())));
        assert!(config.axes.out_of_range(&camera.pose).is_none());
    }
    let armed = do_act(&api, Action::Arm);
    assert!(matches!(armed.transport.output, Output::Positioning | Output::Armed));
    wait_for(&api, |status| status.transport.output == Output::Armed);
    do_act(&api, Action::Disarm);

    // A rig which cannot tilt down that far must still block the same timeline.
    let mut restricted = project.clone();
    restricted.cameras[0].axes.tilt.min = -30.0;
    api.put_project(&restricted).unwrap();
    let checked = api.working(&id).unwrap();
    assert!(checked
        .issues
        .iter()
        .any(|issue| { matches!(issue, Issue::OutOfRange { axis: Axis::Tilt, camera, .. } if camera == "cam1") }));
    refused(api.act(Action::Arm, &TransportRequest::default()), 409);
}
