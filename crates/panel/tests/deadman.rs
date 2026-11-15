//! A panel whose link goes silent without closing: a pulled cable, a switch
//! that died, a panel that froze. Nothing says the link is gone, so the
//! controller watches it closely whenever a control is deflected or a hold is
//! being counted, and these tests measure how long the rig can keep moving.

mod common;

use common::*;
use ptz_core::transport::Output;
use ptz_panel::link::{PING_CLOSE, QUIET_CLOSE, REST_FIRST};
use std::time::{Duration, Instant};

/// The longest the camera may keep moving after the panel goes silent with the
/// joystick over. Made of: `QUIET_CLOSE` (350 ms) without an answer, up to one
/// read of the link (15 ms) to notice, up to one 50 ms hand-control tick to
/// send the stop, and the server bringing the camera to rest.
const SILENT_STOP: Duration = Duration::from_millis(600);

#[test]
fn a_panel_that_goes_silent_mid_jog_stops_the_rig_within_the_stated_bound() {
    let b = Bench::start("deadman-silent");
    start_panning(&b);
    let mut worst = Duration::ZERO;
    let mut times = Vec::new();
    for round in 0..6 {
        if round > 0 {
            b.panel.deflect(JOY_PAN, 0);
            let at = pan(&b);
            b.panel.deflect(JOY_PAN, if round % 2 == 0 { 400 } else { -400 });
            b.wait("the camera to move under the joystick", || (pan(&b) - at).abs() > 0.2);
            // Each round goes silent at a different point between two pings.
            std::thread::sleep(PING_CLOSE.mul_f64(round as f64 / 6.0));
        }

        // No close, no reset: the panel just stops answering, the joystick still over.
        let lost = Instant::now();
        b.panel.set_silent(true);
        let took = stopped_after(&b, lost);
        times.push(took);
        worst = worst.max(took);
        assert!(took < SILENT_STOP, "round {round}: the camera moved for {took:?} after the panel went silent");
        assert!(
            took > QUIET_CLOSE - PING_CLOSE - Duration::from_millis(50),
            "round {round}: the camera stopped after {took:?}, sooner than the link can have been found silent"
        );

        // The link is given up as lost, and the server is told. The lease stays.
        b.wait("the server to hear the panel is gone", || b.status().panel.is_some_and(|p| !p.connected));
        assert_eq!(b.status().lease.expect("the lease").holder, "Panel");

        // It answers again. Nothing moves: the joystick has to come to rest first.
        b.panel.set_silent(false);
        b.wait("the panel to be connected again", || b.status().panel.is_some_and(|p| p.connected));
        b.wait("the E-Stop key to be drawn again", || b.line(ESTOP) == "Press");
        assert!(stands_still(&b, Duration::from_millis(700)), "round {round}: the camera started moving again by itself");
    }
    println!("silent link, joystick over: camera stopped after {times:?}; worst {worst:?}");
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}

#[test]
fn a_short_silence_lets_go_of_the_rig_and_a_stick_still_over_does_not_take_it_back() {
    let b = Bench::start("deadman-blip");
    start_panning(&b);

    // Silent for half a second: too short for the link to be given up, long enough to let go.
    let lost = Instant::now();
    b.panel.set_silent(true);
    let took = stopped_after(&b, lost);
    assert!(took < SILENT_STOP, "the camera moved for {took:?} after the panel went silent");
    b.panel.set_silent(false);
    assert_eq!(b.panel.connections(), 1, "the connection was kept");

    // The stick is still over, and says so again. The camera stays where it stopped.
    b.panel.deflect(JOY_PAN, 420);
    b.wait_said(REST_FIRST);
    b.panel.deflect(JOY_TILT, 200);
    assert!(stands_still(&b, Duration::from_millis(700)), "a stick that never came to rest moved the camera");

    // One axis at rest is not rest. Both are, and then it drives.
    b.panel.deflect(JOY_PAN, 0);
    assert!(stands_still(&b, Duration::from_millis(400)), "tilt alone moved the camera before the stick had been at rest");
    b.panel.deflect(JOY_TILT, 0);
    let at = pan(&b);
    b.panel.deflect(JOY_PAN, 400);
    b.wait("the camera to move once the stick has been at rest", || (pan(&b) - at).abs() > 0.2);
    b.panel.deflect(JOY_PAN, 0);
}

#[test]
fn a_stick_already_over_when_it_is_given_the_rig_drives_nothing() {
    let b = Bench::start("deadman-handover");
    b.setup_cameras(2);
    b.take_lease();
    b.new_song();

    // Timeline mode does not drive by hand. The stick goes over there, and then Camera is pressed.
    b.mode(TIMELINE);
    b.panel.deflect(JOY_PAN, 400);
    b.wait_said("Press Camera to drive by hand");
    assert!(stands_still(&b, Duration::from_millis(300)));
    b.mode(CAMERA);
    b.wait_said(REST_FIRST);
    assert!(stands_still(&b, Duration::from_millis(700)), "entering Camera mode with the stick over moved the camera");
    b.panel.deflect(JOY_PAN, 0);
    let at = pan(&b);
    b.panel.deflect(JOY_PAN, 400);
    b.wait("the camera to move once the stick has been at rest", || (pan(&b) - at).abs() > 0.2);

    // Another camera is chosen with the stick still over: it is not driven either.
    let second = || b.status().cameras[1].pose.pan;
    let was = second();
    b.press(CAM2);
    b.wait("camera 2 to be the panel's camera", || b.status().panel.is_some_and(|p| p.camera == "cam2"));
    b.wait_said(REST_FIRST);
    b.wait("camera 1 to stand still", || stands_still(&b, Duration::from_millis(200)));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(second(), was, "choosing camera 2 with the stick over moved it");
    b.panel.deflect(JOY_PAN, 0);
    b.panel.deflect(JOY_PAN, 400);
    b.wait("camera 2 to move once the stick has been at rest", || (second() - was).abs() > 0.2);
    b.panel.deflect(JOY_PAN, 0);
}

#[test]
fn a_stick_held_over_while_the_server_restarts_drives_nothing_when_it_returns() {
    let mut b = Bench::start("deadman-server");
    start_panning(&b);
    b.server.stop();
    b.wait_shown("No server", "E-Stop cannot reach the rig from this panel until the server answers");
    b.server.start_again();
    b.wait("the panel to reach the new server", || b.status().panel.is_some_and(|p| p.connected));

    // The lease went with the old server. The panel takes it again, the stick still over.
    b.take_lease();
    b.mode(CAMERA);
    b.panel.deflect(JOY_PAN, 410);
    b.wait_said(REST_FIRST);
    assert!(stands_still(&b, Duration::from_millis(700)), "the camera moved under a stick that was over all along");
    b.panel.deflect(JOY_PAN, 0);
    let at = pan(&b);
    b.panel.deflect(JOY_PAN, 400);
    b.wait("the camera to move once the stick has been at rest", || (pan(&b) - at).abs() > 0.2);
    b.panel.deflect(JOY_PAN, 0);
}

#[test]
fn a_hold_is_not_counted_when_the_panel_goes_silent_before_it_is_up() {
    let b = Bench::start("deadman-hold");
    b.program();
    b.mode(TRANSPORT);
    b.wait_key("Arm");

    // The key goes down and the link dies 0.9 s into a one-second hold. Whether the
    // operator let go cannot be known, so the rig is not armed: not now, not later.
    b.panel.down(K4);
    std::thread::sleep(Duration::from_millis(900));
    b.panel.set_silent(true);
    b.wait_reported("The panel stopped answering during the hold. Nothing was done");
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(b.status().transport.output, Output::Rehearse);
    b.panel.set_silent(false);
    b.wait("the panel to be connected again", || b.status().panel.is_some_and(|p| p.connected));
    b.wait("the Arm key to be drawn again", || b.line(K4) == "Arm");
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(b.status().transport.output, Output::Rehearse, "a hold from before the silence armed the rig");
    assert!(b.log().iter().all(|r| r.kind != "arm"), "nothing was asked of the server");

    // A full hold on a live link still arms.
    b.panel.up(K4);
    let took = b.hold_until(K4, "the hold to arm", || b.status().transport.output != Output::Rehearse);
    assert!(took >= Duration::from_millis(950) && took < Duration::from_millis(1600), "armed after {took:?} of holding");
}

#[test]
fn a_panel_that_never_answers_a_ping_is_not_given_the_rig() {
    let b = Bench::start("deadman-unproven");
    b.take_lease();
    b.controller_restart_with(|panel| panel.set_answers_ping(false));

    // Everything else works: its own heartbeat and its events show it is there.
    // The joystick reports rest, so only the missing answer keeps it from driving.
    b.mode(CAMERA);
    b.panel.deflect(JOY_PAN, 0);
    std::thread::sleep(Duration::from_millis(100));
    let before = pan(&b);
    b.panel.deflect(JOY_PAN, 400);
    b.wait_refused("This panel has not answered a ping, so its link cannot be watched while driving. Hand control stays off");
    assert!(stands_still(&b, Duration::from_millis(600)));
    assert_eq!(pan(&b), before);
    b.panel.deflect(JOY_PAN, 0);

    // A hold still counts, once the panel has been heard from after its time is up.
    b.press(ESTOP);
    b.wait("E-Stop to latch", || b.status().transport.estop);
    let took = b.hold_until(ESTOP, "the hold to reset E-Stop", || !b.status().transport.estop);
    assert!(took >= Duration::from_millis(1450), "reset after {took:?} of holding");

    // Once it answers, the joystick drives.
    b.panel.set_answers_ping(true);
    std::thread::sleep(Duration::from_millis(600));
    b.panel.deflect(JOY_PAN, 400);
    b.wait("the camera to move once the panel answers pings", || (pan(&b) - before).abs() > 0.2);
    b.panel.deflect(JOY_PAN, 0);
}

#[test]
fn a_hold_is_not_counted_when_the_connection_closes_before_it_is_up() {
    let b = Bench::start("deadman-hold-drop");
    b.program();
    b.mode(TRANSPORT);
    b.wait_key("Arm");

    // The panel loses power half way through the hold and comes straight back.
    b.panel.down(K4);
    std::thread::sleep(Duration::from_millis(500));
    b.panel.power_off();
    b.wait("the server to hear the panel is gone", || b.status().panel.is_some_and(|p| !p.connected));
    b.panel.power_on();
    b.wait("the panel to be connected again", || b.status().panel.is_some_and(|p| p.connected));
    b.wait("the Arm key to be drawn again", || b.line(K4) == "Arm");
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(b.status().transport.output, Output::Rehearse, "a hold from before the loss armed the rig");
    assert!(b.log().iter().all(|r| r.kind != "arm"), "nothing was asked of the server");
}
