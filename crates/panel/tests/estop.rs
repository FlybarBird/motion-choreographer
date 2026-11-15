//! E-Stop from the panel: one press of one key, in any mode, whoever holds the
//! edit lease, and never behind anything else. Only a deliberate hold resets it.

mod common;

use common::*;
use ptz_core::api::TransportRequest;
use ptz_core::transport::Action;
use ptz_panel::codec::{Color, Lit};
use std::time::{Duration, Instant};

fn estops_logged(b: &Bench) -> Vec<String> {
    b.log()
        .iter()
        .filter(|r| r.kind == "transport" && r.data["action"] == "estop")
        .map(|r| r.data["by"].as_str().unwrap_or_default().to_string())
        .collect()
}

fn wait_latched(b: &Bench, when: &str) {
    b.wait(&format!("E-Stop to latch {when}"), || b.status().transport.estop);
    b.wait(&format!("the E-Stop key to offer the reset {when}"), || {
        b.line(ESTOP) == "Hold: reset" && b.light(ESTOP) == (Lit::Blink, Color::Red)
    });
}

/// A song is loaded, so that the server lets its lease holder reset E-Stop.
fn load_a_song(b: &Bench, holder: &ptz_core::client::Api) {
    if b.status().song.is_none() {
        let id = holder.songs().expect("the songs")[0].id.clone();
        holder.load_song(&id).expect("the first song loads");
    }
}

#[test]
fn estop_is_one_press_in_every_mode_and_needs_no_lease() {
    let b = Bench::start("estop-modes");
    // Another station holds the lease from start to end. It resets E-Stop between presses; the panel only presses.
    let desk = b.server.client("desk");
    desk.take_lease("Desk", false).expect("the desk takes the lease");
    load_a_song(&b, &desk);
    let reset = || {
        desk.act(Action::ResetEstop, &TransportRequest::default()).expect("the desk resets E-Stop");
        b.wait("the E-Stop key to offer a press again", || b.line(ESTOP) == "Press");
    };
    let offered = |when: &str| {
        assert_eq!((b.title(ESTOP).as_str(), b.line(ESTOP).as_str()), ("E-STOP", "Press"), "{when}");
        assert_eq!(b.light(ESTOP), (Lit::On, Color::Red), "{when}");
    };
    let mut pressed = 0;

    for mode in MODES {
        b.mode(mode);
        offered(&format!("in the mode of key {mode}"));
        b.press(ESTOP);
        pressed += 1;
        wait_latched(&b, &format!("in the mode of key {mode}"));
        assert_eq!(estops_logged(&b).len(), pressed, "one press is one E-Stop");
        reset();
    }

    // Shift changes every soft key. Not this one.
    b.mode(TIMELINE);
    b.panel.down(SHIFT);
    b.wait("Shift to light", || b.panel.lit(SHIFT) == Lit::On);
    offered("with Shift held");
    b.press(ESTOP);
    pressed += 1;
    wait_latched(&b, "with Shift held");
    b.panel.up(SHIFT);
    reset();

    // A list that has taken over the soft keys.
    b.mode(SHOW);
    let browse = KEYS.into_iter().find(|k| b.line(*k).starts_with("Browse")).expect("a Browse key");
    b.press(browse);
    b.wait_key("Back");
    offered("in the song list");
    b.press(ESTOP);
    pressed += 1;
    wait_latched(&b, "in the song list");
    reset();

    // A question waiting for Confirm. E-Stop is sent and the question is dropped unanswered.
    let presets = b.api.presets().expect("presets").len();
    b.mode(PRESETS);
    b.shift(|| b.press(K2));
    b.wait_key("Confirm");
    offered("while a question waits");
    b.press(ESTOP);
    pressed += 1;
    wait_latched(&b, "while a question waits");
    b.wait("the question to go", || b.line(K1) != "Confirm");
    assert_eq!(b.api.presets().expect("presets").len(), presets, "nothing was deleted");
    reset();

    let logged = estops_logged(&b);
    assert_eq!(logged.len(), pressed);
    assert!(logged.iter().all(|by| by.starts_with("watcher panel-")), "sent as a station without the lease: {logged:?}");
    assert_eq!(b.status().lease.expect("a lease").holder, "Desk", "the lease never moved");
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}

#[test]
fn only_a_deliberate_hold_resets_estop() {
    let b = Bench::start("estop-reset");
    b.take_lease();
    if b.status().song.is_none() {
        b.new_song();
    }

    // The press that latches it never resets it, however long it stays down.
    b.panel.down(ESTOP);
    wait_latched(&b, "on the press");
    b.wait_said("E-Stop sent. Output is held");
    std::thread::sleep(Duration::from_millis(2200));
    b.panel.up(ESTOP);
    std::thread::sleep(Duration::from_millis(300));
    assert!(b.status().transport.estop, "the press that latched E-Stop did not also reset it");

    // A tap says how to reset, and sends E-Stop again.
    let before = estops_logged(&b).len();
    b.press(ESTOP);
    b.wait_said("E-Stop is latched. Hold E-Stop for 1.5 seconds to reset it");
    assert!(b.status().transport.estop);
    assert_eq!(estops_logged(&b).len(), before + 1, "a press while latched is still sent");

    // Most of the hold is not the hold.
    b.hold(ESTOP, Duration::from_millis(1000));
    b.wait_said("E-Stop is latched. Hold E-Stop for 1.5 seconds to reset it");
    std::thread::sleep(Duration::from_millis(700));
    assert!(b.status().transport.estop, "a hold let go early does not reset later");

    // The primary key names the reset as the server does, and needs the same hold.
    assert_eq!((b.title(GO).as_str(), b.line(GO).as_str()), ("Hold", "Reset E-Stop"));
    assert_eq!(b.light(GO), (Lit::Blink, Color::Red));
    b.press(GO);
    b.wait_said_start("Hold Reset E-Stop for 1.5 seconds. ");
    std::thread::sleep(Duration::from_millis(400));
    assert!(b.status().transport.estop, "a tap of the primary key does not reset");

    // The hold.
    let began = Instant::now();
    b.panel.down(ESTOP);
    b.wait_said("Keep holding E-Stop to reset it");
    b.wait("E-Stop to be reset by the hold", || !b.status().transport.estop);
    let took = began.elapsed();
    b.panel.up(ESTOP);
    assert!(took >= Duration::from_millis(1450), "reset after only {took:?}");
    b.wait_said("E-Stop is reset");
    b.wait("the E-Stop key to offer a press again", || b.line(ESTOP) == "Press" && b.light(ESTOP) == (Lit::On, Color::Red));
    std::thread::sleep(Duration::from_millis(300));
    assert!(!b.status().transport.estop, "letting go after the reset does not press E-Stop again");

    // And by the primary key.
    b.press(ESTOP);
    wait_latched(&b, "again");
    let took = b.hold_until(GO, "E-Stop to be reset by holding the primary key", || !b.status().transport.estop);
    assert!(took >= Duration::from_millis(1450), "reset after only {took:?}");

    // Reset is the lease holder's. Without the lease the hold is refused in the server's words.
    b.mode(SHOW);
    b.press_key("Release");
    b.wait("the lease to be free", || b.status().lease.is_none());
    b.press(ESTOP);
    wait_latched(&b, "without the lease");
    b.hold(ESTOP, Duration::from_millis(1800));
    b.wait_refused("Take the edit lease to control the show");
    assert_eq!(
        b.last_refusal().map(|(action, _, reason)| (action, reason)),
        Some(("reset_estop".to_string(), "Take the edit lease to control the show".to_string()))
    );
    assert!(b.status().transport.estop);
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}

#[test]
fn estop_does_not_wait_behind_a_call_the_server_is_slow_to_answer() {
    let b = Bench::proxied("estop-busy");
    let proxy = b.proxy.as_ref().expect("the proxy");
    b.mode(SHOW);
    b.wait_key("Take");

    // The server stops answering the controller. Its next call, taking the lease, hangs.
    proxy.freeze(true);
    b.press_key("Take");
    std::thread::sleep(Duration::from_millis(300));
    assert!(b.status().lease.is_none(), "the controller is stuck in its call");

    let pressed = Instant::now();
    b.press(ESTOP);
    b.wait("E-Stop to reach the server past the stuck call", || b.status().transport.estop);
    let took = pressed.elapsed();
    assert!(took < Duration::from_millis(500), "E-Stop took {took:?}");
    assert!(b.status().lease.is_none(), "the call ahead of it is still stuck");

    proxy.freeze(false);
    b.wait("the stuck call to finish", || b.status().lease.is_some_and(|l| l.holder == "Panel"));
    wait_latched(&b, "once the server answers again");
}

#[test]
fn an_estop_pressed_while_the_server_is_away_is_delivered_when_it_returns() {
    let mut b = Bench::start("estop-retry");
    b.server.stop();
    b.wait("the E-Stop key to say there is no server", || {
        b.line(ESTOP) == "No server" && b.light(ESTOP) == (Lit::On, Color::Amber)
    });
    b.wait("the status display to say there is no server", || b.title(STATUS_TILE) == "No server");

    b.press(ESTOP);
    b.wait_shown("Refused", "E-Stop has not reached the server yet. Still trying.");
    std::thread::sleep(Duration::from_millis(1200));

    b.server.start_again();
    b.wait("the E-Stop to arrive at the server that came back", || b.api.status().is_ok_and(|s| s.transport.estop));
    wait_latched(&b, "once the server is back");
}
