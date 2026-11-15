//! `ptz-server --panel`: the server binary runs a SKAARHOJ panel as one of its own
//! stations. The panel must not get in the way of the stop a server sends to the
//! rigs when it is told to exit.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::Api;
use ptz_core::edit;
use ptz_core::pose::{Pose, RigKind};
use ptz_core::transport::{Action, Output, Run};
use ptz_panel::fake::{FakePanel, FAKE_MODEL};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

const ESTOP: u32 = 56;
const GO: u32 = 13;
const STATUS_TILE: u32 = 15;

/// The server process, killed when a test ends early so that it does not outlive the run.
struct Server {
    child: Child,
    out: BufReader<ChildStdout>,
    url: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn line(&mut self) -> String {
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        line.trim_end().to_string()
    }
}

fn command(dir: &Path, more: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ptz-server"));
    command.args(["--data", dir.to_str().unwrap(), "--bind", "127.0.0.1:0", "--no-audio"]).args(more);
    command
}

fn launch(dir: &Path, more: &[&str]) -> Server {
    let mut child = command(dir, more).stdout(Stdio::piped()).spawn().expect("ptz-server binary");
    let mut out = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    let url = line.split_whitespace().last().expect("listening line").to_string();
    assert!(url.starts_with("http://127.0.0.1:"), "unexpected first line: {line}");
    let mut server = Server { child, out, url };
    assert!(server.line().starts_with("data folder: "));
    server
}

fn wait(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(15));
    }
}

fn signal(child: &Child, name: &str) {
    let ok = Command::new("kill").args([&format!("-{name}"), &child.id().to_string()]).status().unwrap().success();
    assert!(ok, "kill -{name}");
}

/// Camera 1 on the protocol fakes, live, part-way through a move that takes twenty seconds.
fn go_live_on_the_fakes(api: &Api, fr7: &FakeFr7, pod: &FakeAutoPod) {
    fr7.place(0.0, 0.0, 0.05, 0.5);
    pod.place(100.0);
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
    wait_for(api, |s| s.first().adapters.len() == 2 && s.first().adapters.iter().all(|a| a.connected));

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
    wait_for(api, |s| s.transport.output == Output::Armed);
    api.act(Action::Play, &TransportRequest::default()).unwrap();
    let st = wait_for(api, |s| s.frame >= 150 || s.transport.run == Run::Stopped);
    assert!(st.hardware_live && st.transport.run != Run::Stopped, "fault: {:?}", st.telemetry.fault);
    assert!(fr7.is_moving() && pod.is_moving(), "the rig is part-way through the move");
}

#[cfg(unix)]
#[test]
fn a_server_running_a_panel_still_stops_the_rig_first_and_exits_at_once() {
    for name in ["INT", "TERM", "HUP"] {
        let dir = temp_dir(&format!("panel-halt-{name}"));
        let panel = FakePanel::start();
        let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
        let mut server = launch(&dir, &["--panel", &panel.addr.to_string()]);
        assert_eq!(server.line(), format!("panel: connecting to {}", panel.addr));
        let api = Api::new(&server.url, "a", None);
        go_live_on_the_fakes(&api, &fr7, &pod);

        // The panel is a station of the server, following the show over its own connections.
        let st = wait_for(&api, |s| s.panel.as_ref().is_some_and(|p| p.connected));
        assert_eq!(st.panel.unwrap().model, FAKE_MODEL);
        wait("the panel to show that the show is live", || panel.text(STATUS_TILE).all().contains("LIVE"));
        // Something of the panel's is in flight when the server is told to stop.
        panel.press(GO);

        signal(&server.child, name);
        let began = Instant::now();
        server.child.wait().unwrap();
        assert!(began.elapsed() < Duration::from_secs(4), "{name}: the server took {:?} to exit", began.elapsed());

        assert!(!fr7.is_moving() && !pod.is_moving(), "{name}: the rig was left moving");
        let at = (fr7.pose(), pod.height_mm());
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!((fr7.pose(), pod.height_mm()), at, "{name}: the rig moved after the server had gone");
        assert!(at.0 .0 > 5.0 && at.0 .0 < 110.0, "{name}: it stopped part-way, at {at:?}");

        let log: String =
            std::fs::read_dir(dir.join("log")).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
        assert!(log.contains("\"server_stopping\""), "{name}: the run log says why the show stopped");

        // The panel is not left showing a live show with nobody behind it.
        let shown = panel.text(STATUS_TILE).all();
        assert!(shown.contains("Controller") && shown.contains("stopped"), "{name}: the panel shows {shown:?}");
        assert!(!panel.cells().values().any(|c| c.text.all().contains("LIVE")), "{name}: {:?}", panel.cells());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(unix)]
#[test]
fn a_panel_that_cannot_be_reached_does_not_keep_the_server_from_stopping() {
    // Nothing listens here: the controller is in the middle of dialling when the server is told to go.
    let nowhere = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap()
    };
    let dir = temp_dir("panel-halt-absent");
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    let mut server = launch(&dir, &["--panel", &nowhere.to_string()]);
    assert_eq!(server.line(), format!("panel: connecting to {nowhere}"));
    let api = Api::new(&server.url, "a", None);
    go_live_on_the_fakes(&api, &fr7, &pod);
    let st = wait_for(&api, |s| s.panel.is_some());
    assert!(!st.panel.unwrap().connected);

    signal(&server.child, "TERM");
    let began = Instant::now();
    server.child.wait().unwrap();
    assert!(began.elapsed() < Duration::from_secs(4), "the server took {:?} to exit", began.elapsed());
    assert!(!fr7.is_moving() && !pod.is_moving(), "the rig was left moving");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_panels_map_is_panel_json_in_the_data_folder() {
    let dir = temp_dir("panel-map");
    std::fs::create_dir_all(&dir).unwrap();
    // E-Stop and the primary key change places.
    std::fs::write(dir.join("panel.json"), format!("{{\"roles\": {{\"estop\": {GO}, \"go\": {ESTOP}}}}}")).unwrap();
    let panel = FakePanel::start();
    let mut server = launch(&dir, &["--panel", &panel.addr.to_string()]);
    assert_eq!(server.line(), format!("panel: connecting to {}", panel.addr));
    assert_eq!(server.line(), format!("panel map: {}", dir.join("panel.json").display()));
    let api = Api::new(&server.url, "a", None);
    wait_for(&api, |s| s.panel.as_ref().is_some_and(|p| p.connected));

    panel.press(GO);
    let st = wait_for(&api, |s| s.transport.estop);
    assert!(st.transport.estop);
    drop(server);

    // A map that cannot be read is said before the server starts, not after it owns the show.
    std::fs::write(dir.join("panel.json"), "{\"roles\": {\"big red button\": 56}}").unwrap();
    let out = command(&dir, &["--panel", &panel.addr.to_string()]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("panel.json") && said.contains("there is no role \"big red button\""), "{said}");
    assert!(out.stdout.is_empty(), "nothing was started: {}", String::from_utf8_lossy(&out.stdout));

    // Without --panel the file is not read and the server starts as it always did.
    let plain = launch(&dir, &[]);
    assert!(Api::new(&plain.url, "a", None).status().unwrap().panel.is_none());
    drop(plain);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_panel_can_dial_in_to_the_server() {
    let dir = temp_dir("panel-listen");
    let mut server = launch(&dir, &["--panel-listen", "127.0.0.1:0"]);
    let line = server.line();
    let on = line.strip_prefix("panel: waiting for one to dial in on ").unwrap_or_else(|| panic!("{line}"));
    let panel = FakePanel::dial(on.parse().expect("an address"));
    let api = Api::new(&server.url, "a", None);
    let st = wait_for(&api, |s| s.panel.as_ref().is_some_and(|p| p.connected));
    assert_eq!(st.panel.unwrap().model, FAKE_MODEL);

    panel.press(ESTOP);
    wait_for(&api, |s| s.transport.estop);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_panel_address_that_cannot_be_listened_on_stops_the_server_with_the_reason() {
    let dir = temp_dir("panel-taken");
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let out = command(&dir, &["--panel-listen", &taken.local_addr().unwrap().to_string()]).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    let said = String::from_utf8_lossy(&out.stderr);
    assert!(said.contains("Cannot listen for a panel on"), "{said}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_keyed_server_gives_its_panel_the_key() {
    let dir = temp_dir("panel-keyed");
    let panel = FakePanel::start();
    let mut server = launch(&dir, &["--access-key", "sesame", "--panel", &panel.addr.to_string()]);
    assert_eq!(server.line(), format!("panel: connecting to {}", panel.addr));
    let api = Api::new(&server.url, "a", Some("sesame"));
    wait_for(&api, |s| s.panel.as_ref().is_some_and(|p| p.connected));
    panel.press(ESTOP);
    wait_for(&api, |s| s.transport.estop);
    let _ = std::fs::remove_dir_all(&dir);
}
