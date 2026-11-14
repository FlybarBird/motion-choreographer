//! Crash safety: kill the server binary in the middle of a write loop, restart,
//! and every file still parses.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::Api;
use ptz_core::edit;
use ptz_core::pose::{Pose, RigKind};
use ptz_core::song::Song;
use ptz_core::transport::{Action, Output, Run};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// The server process, killed when a test ends early so that it does not outlive the run.
struct Server(Child);

impl std::ops::Deref for Server {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Server {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn launch(dir: &std::path::Path) -> (Server, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ptz-server"))
        .args(["--data", dir.to_str().unwrap(), "--bind", "127.0.0.1:0", "--no-audio"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("ptz-server binary");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let url = line.split_whitespace().last().expect("listening line").to_string();
    assert!(url.starts_with("http://127.0.0.1:"), "unexpected first line: {line}");
    (Server(child), url)
}

#[test]
fn killing_the_server_mid_write_never_leaves_a_broken_song() {
    let dir = temp_dir("crash");
    for round in 0..3 {
        let (mut child, url) = launch(&dir);
        let api = Api::new(&url, "writer", None);
        api.take_lease("writer", true).unwrap();
        let project = api.project().unwrap();
        let mut w = api.working("walk-in").unwrap();

        let writer = std::thread::spawn(move || {
            let mut n = 0;
            loop {
                let frame = 700 + (n % 200);
                let Ok(song) = edit::add_marker(&w.song, &project, frame, &format!("m{n}")) else { break };
                match api.put_working(&PutWorking { expected_counter: w.edit_counter, song }) {
                    Ok(next) => w = next,
                    Err(_) => break,
                }
                if n % 3 == 0 && api.save("walk-in").is_err() {
                    break;
                }
                n += 1;
            }
            n
        });

        std::thread::sleep(Duration::from_millis(250 + 90 * round));
        child.kill().unwrap();
        child.wait().unwrap();
        let writes = writer.join().unwrap();
        assert!(writes > 5, "the write loop barely ran ({writes} writes)");

        // Every JSON file in the data folder parses, and no song is half-written.
        let mut stack = vec![dir.clone()];
        let mut checked = 0;
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "json") {
                    let _: serde_json::Value = read_json(&p);
                    checked += 1;
                }
            }
        }
        assert!(checked >= 8);
        let working: Song = read_json(&dir.join("songs/walk-in/working.json"));
        working.validate(ptz_core::FrameRate::F25).unwrap();
        let _: Song = read_json(&dir.join("songs/walk-in/draft.json"));
    }

    // The server restarts on the same folder and serves the song.
    let (mut child, url) = launch(&dir);
    let api = Api::new(&url, "reader", None);
    let deadline = Instant::now() + Duration::from_secs(5);
    let songs = loop {
        match api.songs() {
            Ok(s) => break s,
            Err(e) => assert!(Instant::now() < deadline, "{e}"),
        }
    };
    assert_eq!(songs.len(), 3);
    assert!(api.working("walk-in").unwrap().song.markers.len() > 2);
    child.kill().unwrap();
    child.wait().unwrap();

    // The run log survived three kills with its chain intact.
    let out =
        Command::new(env!("CARGO_BIN_EXE_ptz-server")).args(["--data", dir.to_str().unwrap(), "--verify-log"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The server binary on a fresh folder, live on the protocol fakes, part-way through a slow move.
fn live_on_the_fakes(name: &str) -> (std::path::PathBuf, Server, FakeFr7, FakeAutoPod) {
    let dir = temp_dir(name);
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    fr7.place(0.0, 0.0, 0.05, 0.5);
    pod.place(100.0);
    let (child, url) = launch(&dir);
    let api = Api::new(&url, "a", None);
    api.take_lease("Operator", true).unwrap();
    let mut project = api.project().unwrap();
    let rig = &mut project.cameras[0].rig;
    rig.kind = RigKind::Hardware;
    rig.fr7_host = fr7.visca.to_string();
    rig.fr7_http_port = fr7.http.port();
    rig.fr7_user = fr7.user.clone();
    rig.fr7_password = fr7.password.clone();
    rig.autopod_host = pod.addr.to_string();
    rig.autopod_calibration = FakeAutoPod::height_map();
    let project = api.put_project(&project).unwrap();
    wait_for(&api, |s| s.first().adapters.len() == 2 && s.first().adapters.iter().all(|a| a.connected));

    // Twenty seconds from where the rig stands to somewhere far off: it is moving the whole time.
    let there = Pose { pan: 120.0, tilt: 20.0, zoom: 0.9, focus: 0.5, elevation: 400.0 };
    let far: ptz_core::Preset =
        api.post("/presets", &StorePreset { name: "There".into(), pose: Some(there), camera: None }).unwrap();
    let created = api.new_song("Long Move", None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let (song, clip) = edit::insert_preset_move(&created.song, &project, &far, 30).unwrap();
    let song = edit::set_custom_duration(&song, &project, &clip, 500).unwrap();
    api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();

    api.act(Action::Arm, &TransportRequest::default()).unwrap();
    wait_for(&api, |s| s.transport.output == Output::Armed);
    api.act(Action::Play, &TransportRequest::default()).unwrap();
    let st = wait_for(&api, |s| s.frame >= 150 || s.transport.run == Run::Stopped);
    assert!(st.hardware_live && st.transport.run != Run::Stopped, "fault: {:?}", st.telemetry.fault);
    assert!(fr7.is_moving() && pod.is_moving(), "the rig is part-way through the move");
    (dir, child, fr7, pod)
}

fn signal(child: &Child, name: &str) {
    let ok = Command::new("kill").args([&format!("-{name}"), &child.id().to_string()]).status().unwrap().success();
    assert!(ok, "kill -{name}");
}

#[cfg(unix)]
#[test]
fn a_server_told_to_stop_while_live_stops_the_rig_before_it_exits() {
    // Ctrl-C, then what `kill`, `pkill` and a system shutdown send, then a closed terminal.
    for name in ["INT", "TERM", "HUP"] {
        let (dir, mut child, fr7, pod) = live_on_the_fakes(&format!("halt-{name}"));
        signal(&child, name);
        let began = Instant::now();
        child.wait().unwrap();
        assert!(began.elapsed() < Duration::from_secs(4), "{name}: the server took {:?} to exit", began.elapsed());

        // By the time the process is gone the rig has been stopped, and it stays where it is.
        assert!(!fr7.is_moving() && !pod.is_moving(), "{name}: the rig was left moving");
        let at = (fr7.pose(), pod.height_mm());
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!((fr7.pose(), pod.height_mm()), at, "{name}: the rig moved after the server had gone");
        assert!(at.0 .0 > 5.0 && at.0 .0 < 110.0, "{name}: it stopped part-way, at {at:?}");

        let log: String =
            std::fs::read_dir(dir.join("log")).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
        assert!(log.contains("\"server_stopping\""), "{name}: the run log says why the show stopped");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(unix)]
#[test]
fn a_rig_left_moving_by_a_killed_server_is_stopped_when_the_next_one_connects() {
    let (dir, mut child, fr7, pod) = live_on_the_fakes("halt-kill");
    // Killed outright, the server cannot send anything: the rig keeps its last speed command.
    child.kill().unwrap();
    child.wait().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(fr7.is_moving() && pod.is_moving(), "a real head keeps moving too; the stand-ins must model that");

    let (mut child, url) = launch(&dir);
    let api = Api::new(&url, "b", None);
    let deadline = Instant::now() + Duration::from_secs(5);
    while fr7.is_moving() || pod.is_moving() {
        assert!(Instant::now() < deadline, "the restarted server did not stop the rig");
        std::thread::sleep(Duration::from_millis(20));
    }
    let at = (fr7.pose(), pod.height_mm());
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!((fr7.pose(), pod.height_mm()), at);
    // It comes back in Rehearse, sending nothing more.
    let st = wait_for(&api, |s| s.first().adapters.len() == 2 && s.first().adapters.iter().all(|a| a.connected));
    assert_eq!(st.transport.output, Output::Rehearse);
    assert!(!st.hardware_live);
    child.kill().unwrap();
    child.wait().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
}
