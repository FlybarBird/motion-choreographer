//! Audit: what the rig is left doing when the server process is stopped while live.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::Api;
use ptz_core::edit;
use ptz_core::pose::{Pose, RigKind};
use ptz_core::transport::{Action, Output};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn launch(dir: &std::path::Path) -> (Child, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ptz-server"))
        .args(["--data", dir.to_str().unwrap(), "--bind", "127.0.0.1:0", "--no-audio"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("ptz-server binary");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    (child, line.split_whitespace().last().expect("listening line").to_string())
}

/// Start the server binary, go live on the stand-ins, and stop the process with `signal` in the middle of a move.
fn stop_the_server_mid_move(signal: &str) {
    let dir = temp_dir("audit-exit");
    let (mut child, url) = launch(&dir);
    let api = Api::new(&url, "a", None);
    api.take_lease("Luke-MacBook", false).unwrap();
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    let mut project = api.project().unwrap();
    project.cameras[0].rig.kind = RigKind::Hardware;
    project.cameras[0].rig.fr7_host = fr7.visca.to_string();
    project.cameras[0].rig.fr7_http_port = fr7.http.port();
    project.cameras[0].rig.fr7_user = fr7.user.clone();
    project.cameras[0].rig.fr7_password = fr7.password.clone();
    project.cameras[0].rig.autopod_host = pod.addr.to_string();
    project.cameras[0].rig.autopod_calibration = FakeAutoPod::height_map();
    let project = api.put_project(&project).unwrap();
    wait_for(&api, |s| s.first().adapters.len() == 2 && s.first().adapters.iter().all(|a| a.connected));

    let far = Pose { pan: 160.0, elevation: 600.0, ..Pose::home() };
    let mark: ptz_core::pose::Preset =
        api.post("/presets", &StorePreset { name: "Far".into(), pose: Some(far), camera: None }).unwrap();
    let created = api.new_song("Exit", None).unwrap();
    api.load_song(&created.song.id).unwrap();
    let (song, _) = edit::insert_preset_move(&created.song, &project, &mark, 25).unwrap();
    api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&created.song.id).unwrap();
    api.commit(&created.song.id).unwrap();
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.frame > 120);
    assert!(fr7.is_moving() && pod.is_moving(), "the rig is mid-move");
    drop(api);

    let sent = Instant::now();
    assert!(Command::new("kill").args([signal, &child.id().to_string()]).status().unwrap().success());
    let deadline = Instant::now() + Duration::from_secs(5);
    let exited = loop {
        if child.try_wait().unwrap().is_some() {
            break Some(sent.elapsed());
        }
        if Instant::now() > deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    std::thread::sleep(Duration::from_millis(1500));
    let (pan, ..) = fr7.pose();
    let result = (fr7.is_moving(), pod.is_moving());
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        result,
        (false, false),
        "(head moving, column moving) 1.5 s after {signal}; the server exited after {exited:?}; the head is at pan {pan:.0}, the column at {:.0} mm",
        pod.height_mm()
    );
}

#[test]
fn ctrl_c_while_live_stops_the_rig() {
    stop_the_server_mid_move("-INT");
}

#[test]
fn a_service_manager_stopping_the_server_while_live_stops_the_rig() {
    stop_the_server_mid_move("-TERM");
}
