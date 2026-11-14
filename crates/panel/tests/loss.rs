//! Losing the panel, and losing the server. Either way the rig is let go of at
//! once, nothing starts moving again by itself, and the panel comes back
//! showing what it showed.

mod common;

use common::*;
use ptz_core::transport::Run;
use ptz_panel::codec::{Color, Lit, Text};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

/// Everything the panel shows, less the two tiles whose text changes with time.
fn shown(b: &Bench) -> BTreeMap<u32, (Lit, Color, Text)> {
    let mut cells: BTreeMap<u32, (Lit, Color, Text)> =
        b.panel.cells().into_iter().map(|(hwc, c)| (hwc, (c.lit, c.color.unwrap_or_default(), c.text))).collect();
    cells.remove(&STATUS_TILE);
    cells.remove(&MESSAGE_TILE);
    cells.retain(|_, c| *c != (Lit::Off, Color::Default, Text::default()));
    cells
}

fn panel_records(b: &Bench) -> Vec<serde_json::Value> {
    b.log().into_iter().filter(|r| r.kind == "panel").map(|r| r.data).collect()
}

#[test]
fn a_panel_that_drops_mid_jog_lets_go_of_the_rig_and_comes_back_as_it_was() {
    let b = Bench::start("loss-drop");
    start_panning(&b);
    b.panel.deflect(JOY_PAN, 0);
    b.wait("the camera to stand still", || stands_still(&b, Duration::from_millis(200)));
    b.wait("the messages to clear", || b.message().is_empty());
    let before = shown(&b);
    assert!(before.len() >= 15, "the panel shows a full surface: {} cells", before.len());

    b.panel.deflect(JOY_PAN, 400);
    b.wait("the camera to move again", || !stands_still(&b, Duration::from_millis(60)));

    // The panel loses power with the joystick over: connection closed, displays gone.
    let lost = Instant::now();
    b.panel.power_off();
    let took = stopped_after(&b, lost);
    assert!(took < Duration::from_millis(600), "the camera moved for {took:?} after the panel was lost");
    assert!(b.panel.cells().is_empty());

    // The server is told, and logs it. The show and the lease are untouched.
    b.wait("the server to hear the panel is gone", || b.status().panel.is_some_and(|p| !p.connected));
    let records = panel_records(&b);
    assert!(
        records.iter().any(|r| r["station"] == "Panel" && r["connected"] == true && r["model"] == ptz_panel::fake::FAKE_MODEL),
        "{records:?}"
    );
    assert_eq!(records.last().map(|r| (&r["station"], &r["connected"])), Some((&"Panel".into(), &false.into())), "{records:?}");
    assert_eq!(b.status().lease.expect("the lease").holder, "Panel");

    // It comes back. Everything is drawn again, and nothing moves: the joystick has said nothing new.
    b.panel.power_on();
    b.wait("the panel to be connected again", || b.status().panel.is_some_and(|p| p.connected));
    b.wait("every display to return", || shown(&b) == before);
    assert!(stands_still(&b, Duration::from_millis(900)), "the camera started moving again by itself");
    assert_eq!(b.status().lease.expect("the lease").holder, "Panel");
    assert_eq!(panel_records(&b).last().map(|r| r["connected"].clone()), Some(true.into()));

    // Fresh input drives again.
    let at = pan(&b);
    b.panel.deflect(JOY_PAN, 0);
    b.panel.deflect(JOY_PAN, 400);
    b.wait("fresh input to move the camera", || (pan(&b) - at).abs() > 0.2);
    b.panel.deflect(JOY_PAN, 0);
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}

#[test]
fn a_connection_that_closes_mid_jog_lets_go_of_the_rig() {
    let b = Bench::start("loss-close");
    start_panning(&b);
    let lost = Instant::now();
    b.panel.drop_connections();
    let took = stopped_after(&b, lost);
    assert!(took < Duration::from_millis(600), "the camera moved for {took:?} after the connection closed");
    b.wait("the panel to be connected again", || b.panel.connections() == 1 && b.status().panel.is_some_and(|p| p.connected));
    assert!(stands_still(&b, Duration::from_millis(900)), "the camera started moving again by itself");
}

#[test]
fn a_panel_that_stays_away_gives_the_lease_back_and_does_not_take_it_again() {
    let b = Bench::with("loss-grace", true, |o| o.lease_grace = Duration::from_millis(1500));
    b.take_lease();
    if b.status().song.is_none() {
        b.new_song();
    }
    b.press_key("Rehearse");
    b.wait("the show to play", || b.status().transport.run == Run::Playing);

    let lost = Instant::now();
    b.panel.power_off();
    b.wait("the server to hear the panel is gone", || b.status().panel.is_some_and(|p| !p.connected));
    std::thread::sleep(Duration::from_millis(900).saturating_sub(lost.elapsed()));
    assert_eq!(b.status().lease.map(|l| l.holder), Some("Panel".to_string()), "the lease is kept through the grace period");

    b.wait("the lease to be given back", || b.status().lease.is_none());
    assert!(lost.elapsed() >= Duration::from_millis(1500), "given back after only {:?}", lost.elapsed());
    let released = b.log().into_iter().rfind(|r| r.kind == "lease").expect("a lease record");
    assert_eq!(released.data["released_by"], "Panel", "{released:?}");
    assert_eq!(b.status().transport.run, Run::Playing, "the show kept running");

    // Another station can carry on, and the panel does not take the lease back when it returns.
    let desk = b.server.client("desk");
    desk.take_lease("Desk", false).expect("the desk takes the free lease");
    desk.release_lease().expect("and lets go");
    b.panel.power_on();
    b.wait("the panel to be connected again", || b.status().panel.is_some_and(|p| p.connected));
    b.mode(SHOW);
    b.wait_key("Take");
    std::thread::sleep(Duration::from_millis(500));
    assert!(b.status().lease.is_none(), "the lease is taken only by a press");
}

#[test]
fn a_server_that_restarts_is_shown_as_gone_and_then_recovered_from() {
    let mut b = Bench::start("loss-server");
    start_panning(&b);

    b.server.stop();
    b.wait("the E-Stop key to say it cannot reach the rig", || {
        (b.title(ESTOP).as_str(), b.line(ESTOP).as_str()) == ("E-STOP", "No server") && b.light(ESTOP) == (Lit::On, Color::Amber)
    });
    b.wait("the status display to say there is no server", || b.title(STATUS_TILE) == "No server");
    b.wait("the keys to go dark", || KEYS.into_iter().chain([GO]).all(|k| b.line(k).is_empty() && b.panel.lit(k) == Lit::Off));
    b.wait_shown("No server", "E-Stop cannot reach the rig from this panel until the server answers");
    // Nothing is asked of a server that is not there.
    let hand = std::net::TcpListener::bind(("127.0.0.1", b.server.port)).expect("the server's port is free");
    hand.set_nonblocking(true).expect("non-blocking");
    let mut jogs = 0;
    let until = Instant::now() + Duration::from_millis(1500);
    while Instant::now() < until {
        if let Ok((mut stream, _)) = hand.accept() {
            use std::io::Read;
            let mut head = [0u8; 64];
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let n = stream.read(&mut head).unwrap_or(0);
            if head[..n].starts_with(b"POST /api/v1/rig/jog") {
                jogs += 1;
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    drop(hand);
    assert_eq!(jogs, 0, "hand control is not sent while the server is away, though the joystick is still over");

    b.server.start_again();
    b.wait("the panel to recover by itself", || {
        b.line(ESTOP) == "Press" && b.light(ESTOP) == (Lit::On, Color::Red) && b.title(STATUS_TILE) != "No server"
    });
    b.wait("the server to hear from the panel again", || b.api.status().is_ok_and(|s| s.panel.is_some_and(|p| p.connected)));
    // A restarted server has given no one the lease. The panel does not take it by itself.
    b.mode(SHOW);
    b.wait_key("Take");
    assert!(b.status().lease.is_none());
    b.panel.deflect(JOY_PAN, 0);
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}
