//! What the server knows about the panel: its state on the status stream, the
//! run-log records, and a panel that dials in instead of being dialled.

mod common;

use common::*;
use ptz_panel::fake::{FakePanel, FAKE_MODEL, FAKE_NAME};
use ptz_panel::Options;
use std::time::{Duration, Instant};

fn panel_records(log: Vec<ptz_core::api::LogRecord>) -> Vec<serde_json::Value> {
    log.into_iter().filter(|r| r.kind == "panel").map(|r| r.data).collect()
}

#[test]
fn the_server_shows_the_panel_and_forgets_it_three_seconds_after_the_controller_stops() {
    let mut b = Bench::start("station-expiry");
    let p = b.status().panel.expect("the panel's state");
    assert_eq!((p.station.as_str(), p.connected, p.model.as_str(), p.name.as_str()), ("Panel", true, FAKE_MODEL, FAKE_NAME));
    assert_eq!((p.mode.as_str(), p.camera.as_str()), ("Show", "cam1"));
    b.mode(CAMERA);
    b.wait("the server to show the panel's mode", || b.status().panel.is_some_and(|p| p.mode == "Camera"));
    b.take_lease();

    // The controller stops. It lets go of the lease on the way out.
    let stopped = Instant::now();
    b.controller.take().expect("the controller").stop();
    assert!(b.status().lease.is_none(), "the lease was released when the controller stopped");
    assert!(b.status().panel.is_some(), "the panel is still shown just after the controller stopped");
    b.wait("the server to forget the panel", || b.status().panel.is_none());
    let took = stopped.elapsed();
    assert!(took > Duration::from_millis(1800) && took < Duration::from_millis(4500), "forgotten after {took:?}");

    let records = panel_records(b.log());
    let last = records.last().expect("a panel record");
    assert_eq!((&last["station"], &last["gone"]), (&"Panel".into(), &true.into()), "{records:?}");
    let connected: Vec<&serde_json::Value> = records.iter().filter(|r| r["connected"] == true).collect();
    assert_eq!(connected.len(), 1, "{records:?}");
    assert_eq!((&connected[0]["model"], &connected[0]["name"]), (&FAKE_MODEL.into(), &FAKE_NAME.into()));
    let lease: Vec<serde_json::Value> = b.log().into_iter().filter(|r| r.kind == "lease").map(|r| r.data).collect();
    assert_eq!(lease.last().map(|r| r["released_by"].clone()), Some("Panel".into()), "{lease:?}");
}

#[test]
fn a_panel_that_dials_in_is_served_like_one_that_is_dialled() {
    let server = TestServer::start("station-dial");
    let mut options = Options::new(&server.url);
    options.listen = Some("127.0.0.1:0".into());
    options.quiet = true;
    let mut controller = ptz_panel::start(options).expect("the controller starts");
    let listening = controller.listening.expect("the address the controller waits on");

    let panel = FakePanel::dial(listening);
    let api = server.client("observer");
    wait_for("the panel to be connected", Duration::from_secs(8), || {
        api.status().expect("status").panel.is_some_and(|p| p.connected && p.model == FAKE_MODEL)
    });
    wait_for("the panel to be drawn", Duration::from_secs(8), || panel.text(ESTOP).line1 == "Press");

    // A panel that dials in is told it is the active one before anything else (reference A.3).
    let lines: Vec<String> = panel.lines().into_iter().filter(|l| !l.is_empty()).collect();
    assert_eq!(lines.first().map(String::as_str), Some("ActivePanel=1"), "{:?}", &lines[..lines.len().min(6)]);
    assert!(lines.iter().any(|l| l == "list") && lines.iter().any(|l| l == "PanelTopology?"));

    // It works: the lease is taken from it, and E-Stop latches.
    panel.press(SHOW);
    wait_for("Show mode", Duration::from_secs(8), || panel.text(K1).line1 == "Take");
    panel.press(K1);
    wait_for("the lease to be taken", Duration::from_secs(8), || {
        api.status().expect("status").lease.is_some_and(|l| l.holder == "Panel")
    });
    panel.press(ESTOP);
    wait_for("E-Stop to latch", Duration::from_secs(8), || api.status().expect("status").transport.estop);

    // It loses power, comes back and dials again: served again, and drawn again.
    panel.power_off();
    wait_for("the server to hear the panel is gone", Duration::from_secs(8), || {
        api.status().expect("status").panel.is_some_and(|p| !p.connected)
    });
    panel.power_on();
    wait_for("the panel to be connected again", Duration::from_secs(8), || {
        api.status().expect("status").panel.is_some_and(|p| p.connected)
    });
    wait_for("the E-Stop key to show it is latched", Duration::from_secs(8), || {
        let t = panel.text(ESTOP);
        (t.line1.as_str(), t.line2.as_str()) == ("Hold:", "reset")
    });
    assert!(panel.unknown().is_empty(), "the panel understood every line: {:?}", panel.unknown());
    controller.stop();
}

#[test]
fn starting_without_a_panel_address_or_with_a_taken_port_says_what_is_wrong() {
    let server = TestServer::start("station-start");
    let err = ptz_panel::start(Options::new(&server.url)).err().expect("no panel address");
    assert_eq!(err, "Give the panel's address with --panel, or an address to wait on with --listen");

    let taken = std::net::TcpListener::bind("127.0.0.1:0").expect("a port");
    let addr = taken.local_addr().expect("its address").to_string();
    let mut options = Options::new(&server.url);
    options.listen = Some(addr.clone());
    let err = ptz_panel::start(options).err().expect("the port is taken");
    assert!(err.starts_with(&format!("Cannot listen for a panel on {addr}: ")), "{err}");

    let mut options = Options::new(&server.url);
    options.panel = Some("127.0.0.1:9".into());
    options.station = "  ".into();
    assert_eq!(ptz_panel::start(options).err().expect("no station name"), "The station name cannot be empty");
}
