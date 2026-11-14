//! The `ptz-panel` program itself: its options, the map check, and what it does
//! when it is told to stop.

mod common;

use common::*;
use ptz_panel::fake::FakePanel;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn program() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ptz-panel"))
}

fn said(args: &[&str]) -> (Option<i32>, String, String) {
    let out = program().args(args).env_remove("PTZ_ACCESS_KEY").output().expect("ptz-panel runs");
    (out.status.code(), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
}

struct Program(Child);

impl Drop for Program {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn the_version_the_help_and_a_wrong_option_are_answered_without_starting_anything() {
    let (code, out, _) = said(&["--version"]);
    assert_eq!((code, out.trim()), (Some(0), concat!("ptz-panel ", env!("CARGO_PKG_VERSION"))));
    let (code, out, _) = said(&["--help"]);
    assert_eq!(code, Some(0));
    for word in ["--server", "--panel", "--listen", "--access-key", "--station", "--map", "--lease-grace", "map", "learn", "fake"]
    {
        assert!(out.contains(word), "--help does not name {word}:\n{out}");
    }

    let (code, _, err) = said(&[]);
    assert_eq!(code, Some(2));
    assert!(err.contains("give the panel's address with --panel, or an address to wait on with --listen"), "{err}");
    let (code, _, err) = said(&["--panel", "127.0.0.1", "--sideways"]);
    assert_eq!((code, err.trim()), (Some(2), "ptz-panel: --sideways is not an option. Try --help"));
    let (code, _, err) = said(&["--panel"]);
    assert_eq!((code, err.trim()), (Some(2), "ptz-panel: --panel needs a value"));
    let (code, _, err) = said(&["--panel", "127.0.0.1", "--lease-grace", "soon"]);
    assert_eq!((code, err.trim()), (Some(2), "ptz-panel: --lease-grace needs a number of seconds, 0 or more"));
    let (code, _, err) = said(&["--panel", "127.0.0.1", "--map", "/nowhere/panel.json"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("/nowhere/panel.json does not exist"), "{err}");
}

#[test]
fn map_prints_the_layout_and_checks_a_map_file_against_a_panel() {
    let (code, out, _) = said(&["map"]);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains("The map fits this panel."), "{out}");
    let row = out.lines().find(|l| l.starts_with("estop ")).expect("a row for E-Stop");
    assert!(row.contains(" 56 ") && row.contains("CAM 9"), "{row}");

    let panel = FakePanel::start();
    let addr = panel.addr.to_string();
    let (code, out, err) = said(&["map", "--panel", &addr]);
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains(&format!(
        "Checked against: {} \"{}\" at {addr}",
        ptz_panel::fake::FAKE_MODEL,
        ptz_panel::fake::FAKE_NAME
    )));

    let dir = temp_dir("bin-map");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("panel.json");
    std::fs::write(&file, "{\"roles\": {\"estop\": 99, \"zoom_rocker\": 5}}").unwrap();
    let (code, out, _) = said(&["map", "--map", file.to_str().unwrap(), "--panel", &addr]);
    assert_eq!(code, Some(1), "{out}");
    assert!(out.contains("estop is bound to component 99, which this panel does not have"), "{out}");
    assert!(out.contains("zoom_rocker needs a joystick axis or fader, but component 5 (Focus) is an encoder with push"), "{out}");

    std::fs::write(&file, "{\"roles\": {\"estop\": \"the red one\"}}").unwrap();
    let (code, _, err) = said(&["map", "--map", file.to_str().unwrap()]);
    assert_eq!(code, Some(2));
    assert!(err.contains("panel.json") && err.contains("\"estop\" must be a component number"), "{err}");

    let (code, _, err) = said(&["map", "--panel", "127.0.0.1:1"]);
    assert_eq!(code, Some(2));
    assert!(err.contains("No panel answers at 127.0.0.1:1"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn told_to_stop_it_gives_the_lease_back_and_tells_the_panel() {
    for name in ["INT", "TERM", "HUP"] {
        let server = TestServer::start(&format!("bin-stop-{name}"));
        let panel = FakePanel::start();
        let mut child = program()
            .args(["--server", &server.url, "--panel", &panel.addr.to_string(), "--station", "Desk"])
            .env_remove("PTZ_ACCESS_KEY")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("ptz-panel runs");
        let mut out = BufReader::new(child.stdout.take().unwrap());
        let mut program = Program(child);
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        assert_eq!(
            line.trim(),
            format!("ptz-panel {}: station \"Desk\" on the show server at {}", env!("CARGO_PKG_VERSION"), server.url)
        );

        let api = server.client("observer");
        let limit = Duration::from_secs(8);
        wait_for("the panel to be connected", limit, || api.status().unwrap().panel.is_some_and(|p| p.connected));
        wait_for("the Take key", limit, || panel.text(K1).all().contains("Take"));
        panel.press(K1);
        wait_for("the panel to hold the lease", limit, || api.status().unwrap().lease.is_some_and(|l| l.holder == "Desk"));

        let ok = Command::new("kill").args([&format!("-{name}"), &program.0.id().to_string()]).status().unwrap().success();
        assert!(ok, "kill -{name}");
        let began = Instant::now();
        let status = program.0.wait().unwrap();
        assert!(began.elapsed() < Duration::from_secs(3), "{name}: it took {:?} to stop", began.elapsed());
        assert_eq!(status.code(), Some(0), "{name}");

        // Not left to run out ten seconds later: the lease is free as soon as the program has gone.
        assert!(api.status().unwrap().lease.is_none(), "{name}: the lease was not given back");
        let shown = panel.text(STATUS_TILE).all();
        assert!(shown.contains("Controller") && shown.contains("stopped"), "{name}: the panel shows {shown:?}");
        assert!(panel.text(ESTOP).is_empty(), "{name}: the E-Stop key still offers something: {:?}", panel.text(ESTOP));
    }
}
