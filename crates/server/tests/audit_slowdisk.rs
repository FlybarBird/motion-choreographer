//! Audit: the run log is flushed to disk from inside the live loop. What that does on a
//! disk where a flush takes as long as it does on a spinning disk, an SD card or a USB stick.
//!
//! Needs `cc` and Linux: the server binary is started with a small preloaded library that
//! makes `fsync` and `fdatasync` take `SLOW_SYNC_MS` longer.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::camera::{CameraConfig, MAX_CAMERAS};
use ptz_core::client::Api;
use ptz_core::edit;
use ptz_core::pose::{Pose, Preset, RigKind};
use ptz_core::transport::{Action, Output};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SHIM: &str = r#"
#define _GNU_SOURCE
#include <dlfcn.h>
#include <stdlib.h>
#include <time.h>
static void wait_a_while(void) {
    const char *e = getenv("SLOW_SYNC_MS");
    long ms = e ? atol(e) : 0;
    struct timespec ts = { ms / 1000, (ms % 1000) * 1000000L };
    nanosleep(&ts, 0);
}
int fdatasync(int fd) {
    static int (*real)(int);
    if (!real) real = dlsym(RTLD_NEXT, "fdatasync");
    wait_a_while();
    return real(fd);
}
int fsync(int fd) {
    static int (*real)(int);
    if (!real) real = dlsym(RTLD_NEXT, "fsync");
    wait_a_while();
    return real(fd);
}
"#;

fn launch(dir: &std::path::Path, sync_ms: u32) -> (Child, String) {
    let shim_c = dir.with_extension("shim.c");
    let shim_so = dir.with_extension("shim.so");
    std::fs::write(&shim_c, SHIM).unwrap();
    let built = Command::new("cc").args(["-shared", "-fPIC", "-o"]).arg(&shim_so).arg(&shim_c).arg("-ldl").status().unwrap();
    assert!(built.success(), "cc could not build the slow-disk shim");
    let mut child = Command::new(env!("CARGO_BIN_EXE_ptz-server"))
        .args(["--data", dir.to_str().unwrap(), "--bind", "127.0.0.1:0", "--no-audio"])
        .env("LD_PRELOAD", &shim_so)
        .env("SLOW_SYNC_MS", sync_ms.to_string())
        .stdout(Stdio::piped())
        .spawn()
        .expect("ptz-server binary");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    (child, line.split_whitespace().last().expect("listening line").to_string())
}

/// Twelve cameras play a slow fifteen-second move live. Returns the first fault, if any, and when.
fn twelve_cameras_live(sync_ms: u32) -> Option<(String, Duration)> {
    let dir = temp_dir("audit-slowdisk");
    let (mut child, url) = launch(&dir, sync_ms);
    let api = Api::new(&url, "a", None);
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs: Vec<(FakeFr7, FakeAutoPod)> = (0..MAX_CAMERAS).map(|_| (FakeFr7::start(), FakeAutoPod::start())).collect();
    let mut project = api.project().unwrap();
    while project.cameras.len() < rigs.len() {
        let id = project.free_camera_id();
        let name = format!("Camera {}", project.cameras.len() + 1);
        project.cameras.push(CameraConfig::new(&id, &name));
    }
    let mut project = api.put_project(&project).unwrap();
    for (camera, (fr7, pod)) in project.cameras.iter_mut().zip(&rigs) {
        camera.rig.kind = RigKind::Hardware;
        camera.rig.fr7_host = fr7.visca.to_string();
        camera.rig.fr7_http_port = fr7.http.port();
        camera.rig.fr7_user = fr7.user.clone();
        camera.rig.fr7_password = fr7.password.clone();
        camera.rig.autopod_host = pod.addr.to_string();
        camera.rig.autopod_calibration = FakeAutoPod::height_map();
    }
    let project = api.put_project(&project).unwrap();
    let ready = |c: &CameraStatus| c.hardware && c.adapters.len() == 2 && c.adapters.iter().all(|a| a.connected);
    wait_for(&api, |s| s.cameras.len() == rigs.len() && s.cameras.iter().all(ready));

    let created = api.new_song("Slow disk", None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let mut song = created.song;
    for camera in &project.cameras {
        let pose = Pose { pan: 30.0, elevation: 400.0, ..Pose::home() };
        let mark: Preset = api
            .post(
                "/presets",
                &StorePreset { name: format!("Up {}", camera.id), pose: Some(pose), camera: Some(camera.id.clone()) },
            )
            .unwrap();
        song = edit::insert_preset_move(&song, &project, &mark, 25).unwrap().0;
    }
    api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    do_act(&api, Action::Arm);
    let st = wait_for(&api, |s| s.transport.output == Output::Armed || s.telemetry.fault.is_some());
    let mut outcome = st.telemetry.fault.clone().map(|why| (format!("while arming: {why}"), Duration::ZERO));
    if outcome.is_none() {
        do_act(&api, Action::Play);
        let began = Instant::now();
        while began.elapsed() < Duration::from_secs(6) {
            let st = api.status().unwrap();
            if let Some(why) = st.telemetry.fault {
                let held = st.cameras.iter().filter(|c| c.held || c.fault.is_some()).count();
                outcome =
                    Some((format!("{why} ({held} of 12 cameras faulted, output {:?})", st.transport.output), began.elapsed()));
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    if std::env::var("AUDIT_SHOW_LOG").is_ok() {
        let log: Vec<LogRecord> = api.get("/log?since=0").unwrap();
        let from = log.iter().rposition(|r| r.kind == "arm").unwrap_or(0);
        for r in &log[from..] {
            println!("LOG {} {} {}", r.seq, r.kind, r.data.to_string().chars().take(150).collect::<String>());
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&dir);
    outcome
}

#[test]
fn twelve_cameras_play_live_on_a_disk_that_takes_30_ms_to_flush() {
    let fast = twelve_cameras_live(0);
    assert_eq!(fast, None, "control: with the disk as it is, six seconds live pass without a fault");
    if let Ok(list) = std::env::var("AUDIT_SYNC_MS") {
        for ms in list.split(',').map(|v| v.parse::<u32>().unwrap()) {
            println!("MEASURE flush {ms} ms: {:?}", twelve_cameras_live(ms));
        }
    }
    let slow = twelve_cameras_live(30);
    assert!(
        slow.is_none(),
        "with a 30 ms flush the show faulted {:?} into Play Live: {}",
        slow.as_ref().unwrap().1,
        slow.as_ref().unwrap().0
    );
}
