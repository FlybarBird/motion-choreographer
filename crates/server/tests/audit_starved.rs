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
        // On connecting the server sends each device a stop. After that, nothing until Arm.
        self.fr7.settle_after_connect();
        self.pod.settle_after_connect();
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


/// Audit (scratch): after the rigs' deadman has fired once, can the show be armed again?
#[test]
fn a_rig_stopped_by_its_deadman_can_be_armed_again() {
    let t = TestServer::start("audit-starved");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    let project = rig.connect(&api);
    let mark = store(&api, "Far", Pose { pan: 120.0, tilt: 10.0, zoom: 0.4, focus: 0.4, elevation: 300.0 });
    song_with_move(&api, &project, "Stall", &mark);
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.frame > 60);
    {
        let show = t.running.as_ref().unwrap().show.clone();
        let _held = show.lock().unwrap();
        std::thread::sleep(Duration::from_millis(600));
    }
    let st = wait_for(&api, |s| s.telemetry.fault.is_some());
    println!("MEASURE fault after the stall: {:?}; output {:?}", st.telemetry.fault, st.transport.output);
    std::thread::sleep(Duration::from_millis(1500));
    assert!(!rig.is_moving());
    let st = api.status().unwrap();
    if st.transport.output != Output::Rehearse {
        do_act(&api, Action::Disarm);
    }
    let _ = api.act(Action::Stop, &TransportRequest::default());
    std::thread::sleep(Duration::from_millis(500));
    let arm = api.act(Action::Arm, &TransportRequest::default());
    println!("MEASURE Arm after the stall: {:?}", arm.as_ref().map(|s| s.transport.output));
    assert!(arm.is_ok(), "{arm:?}");
}
