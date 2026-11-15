//! Whatever the server refuses, the panel says why, in the server's words. A
//! key that can do nothing right now is dark, and says why when pressed.

mod common;

use common::*;
use ptz_core::transport::{Output, Run};
use ptz_panel::codec::Lit;
use std::time::Duration;

const NO_LEASE: &str = "Take the edit lease to control the show";

/// The panel was refused `action` by the server for `reason`, and shows that reason and no other.
fn refused(b: &Bench, action: &str, by_start: &str, reason: &str) {
    b.wait_refused(reason);
    let (was, by, why) = b.last_refusal().expect("the server logged the refusal");
    assert_eq!((was.as_str(), why.as_str()), (action, reason), "the panel shows what the server logged");
    assert!(by.starts_with(by_start), "refused to {by}");
}

#[test]
fn without_the_lease_everything_is_refused_in_the_servers_words() {
    let b = Bench::start("refuse-watching");
    assert!(b.status().lease.is_none());
    let songs = b.api.songs().expect("songs").len();

    // The primary key is dark and gives the server's hint.
    assert_eq!(b.panel.lit(GO), Lit::Off);
    b.press(GO);
    b.wait_said(&b.server.client("watcher").status().expect("status").guidance.hint);
    assert_eq!(b.message(), "Watching. Take the edit lease to control the show.");

    b.mode(SHOW);
    b.press_key("New");
    b.wait_refused(NO_LEASE);
    b.press_key("Save copy");
    b.wait_refused(NO_LEASE);
    assert_eq!(b.api.songs().expect("songs").len(), songs);

    b.mode(TRANSPORT);
    assert_eq!(b.panel.lit(K1), Lit::Off, "a transport key the server would refuse is dark");
    b.press(K1);
    refused(&b, "play", "watcher panel-", NO_LEASE);
    b.hold(K4, Duration::from_millis(1300));
    refused(&b, "arm", "watcher panel-", NO_LEASE);
    b.press_key("Cue");
    refused(&b, "cue", "watcher panel-", NO_LEASE);
    b.press(K7);
    refused(&b, "record_on", "watcher panel-", NO_LEASE);
    let s = b.status();
    assert_eq!((s.transport.run, s.transport.output, s.transport.record), (Run::Stopped, Output::Rehearse, false));

    // Hand control too: the joystick moves nothing, and the reason is the server's.
    b.mode(CAMERA);
    let before = pan(&b);
    b.panel.deflect(JOY_PAN, 400);
    b.wait_refused(NO_LEASE);
    assert!(stands_still(&b, Duration::from_millis(300)));
    assert_eq!(pan(&b), before);
    b.panel.deflect(JOY_PAN, 0);
    b.press_key("Store");
    b.wait_refused(NO_LEASE);

    b.mode(PRESETS);
    b.press(K2);
    b.wait_refused(NO_LEASE);
    b.mode(TIMELINE);
    b.press_key("Next");
    b.panel.down(SHIFT);
    b.press_key("Add");
    b.panel.up(SHIFT);
    b.wait_refused(NO_LEASE);
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}

#[test]
fn with_no_song_the_keys_that_need_one_say_so() {
    let b = Bench::empty("refuse-no-song");
    b.take_lease();
    for key in [K4, K5, K6, K7] {
        assert_eq!(b.panel.lit(key), Lit::Off);
        b.press(key);
        b.wait_said("Create or load a song first");
    }
    b.mode(TRANSPORT);
    b.press(K1);
    refused(&b, "play", "Panel", "Create or load a song first");
    b.mode(TIMELINE);
    b.press(K1);
    b.wait_said("Create or load a song first");
}

#[test]
fn the_order_of_work_is_enforced_by_the_server_and_explained_on_the_panel() {
    let b = Bench::empty("refuse-order");
    b.take_lease();
    b.new_song();
    b.mode(CAMERA);
    b.drive(JOY_PAN, 0, |p| p.pan);
    b.press_key("Store");
    b.wait_said_start("Stored Preset");
    b.mode(PRESETS);
    b.playhead_to(50);
    b.press(K2);
    b.wait("the move to be placed", || b.song().song.motion.len() == 1);

    // Unsaved: no commit, and so no arming.
    b.mode(SHOW);
    b.press_key("Commit");
    b.wait_refused("Save the song before committing");
    b.mode(TRANSPORT);
    b.hold(K4, Duration::from_millis(1300));
    refused(&b, "arm", "Panel", "Commit the song first");
    assert_eq!(b.status().transport.output, Output::Rehearse);

    // Saved, not committed: still no arming.
    b.mode(SHOW);
    b.press_key("Save");
    b.wait_said_start("Saved ");
    b.mode(TRANSPORT);
    b.hold(K4, Duration::from_millis(1300));
    refused(&b, "arm", "Panel", "Commit the song first");

    // Reverting with nothing changed since the save: the key is dark and nothing is asked.
    b.mode(SHOW);
    assert_eq!(b.panel.lit(K7), Lit::Off);

    // Record on, then Arm: the server wants Record off first.
    b.press_key("Commit");
    b.wait_said_start("Committed ");
    b.mode(TRANSPORT);
    b.press(K7);
    b.wait("Record to be on", || b.status().transport.record);
    b.hold(K4, Duration::from_millis(1300));
    refused(&b, "arm", "Panel", "Turn Record off before arming");
    b.press(K7);
    b.wait("Record to be off", || !b.status().transport.record);

    // Playing: no arming, no hand control, no other song.
    b.press(K1);
    b.wait("the show to play", || b.status().transport.run == Run::Playing);
    b.hold(K4, Duration::from_millis(1300));
    refused(&b, "arm", "Panel", "Stop before arming");
    b.mode(CAMERA);
    b.panel.deflect(JOY_PAN, 400);
    b.wait_refused("Stop to position the rig by hand, or turn Record on to capture the move");
    b.panel.deflect(JOY_PAN, 0);
    b.mode(SHOW);
    b.press_key("New");
    b.wait_refused("Song 2 is created but not loaded. Stop before loading another song");
    assert_eq!(b.status().song.expect("the song").name, "Song 1");
    b.mode(TRANSPORT);
    b.press_key("Stop");
    b.wait("the show to stop", || b.status().transport.run == Run::Stopped);

    // Armed: no hand control and no editing.
    let took = b.hold_until(K4, "the hold to arm", || b.status().transport.output != Output::Rehearse);
    assert!(took >= Duration::from_millis(950));
    b.wait("the rig to be armed", || b.status().transport.output == Output::Armed);
    b.mode(CAMERA);
    let before = pan(&b);
    b.panel.deflect(JOY_PAN, 400);
    b.wait_refused("Disarm to drive the rig by hand");
    b.panel.deflect(JOY_PAN, 0);
    assert_eq!(pan(&b), before);
    b.mode(TIMELINE);
    b.press_key("Next");
    b.wait_key("Delete");
    let clips = b.song().song.motion.len();
    b.press_key("Delete");
    b.wait_refused("Disarm to edit the song");
    assert_eq!(b.song().song.motion.len(), clips);

    // E-Stop latched: everything waits for the reset.
    b.press(ESTOP);
    b.wait("E-Stop to latch", || b.status().transport.estop);
    b.mode(TRANSPORT);
    b.press(K1);
    refused(&b, "play", "Panel", "Reset E-Stop to continue");
    b.mode(CAMERA);
    b.panel.deflect(JOY_PAN, 400);
    b.wait_refused("Reset E-Stop to continue");
    b.panel.deflect(JOY_PAN, 0);
    b.reset_estop();
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}
