//! Program mode: the timeline wound like a tape, and a move marked with two presses of a
//! camera's number. The poses here are set with the panel's own joystick on the simulator; with
//! another controller on the rig the capture reads the same pose (see the server's capture tests).

mod common;

use std::time::Duration;

use common::*;
use ptz_core::song::ClipBody;
use ptz_core::transport::{Output, Run};
use ptz_panel::codec::{Color, Lit};
use ptz_panel::controller::TAP;

const ARMED: &str = "Disarm to program. Moving the playhead while armed moves the rig";

/// Shift and Presets: the PTZ Pro has no button left for Program.
fn program_mode(b: &Bench) {
    b.shift(|| b.press(PRESETS));
    b.wait("the panel to be in Program", || b.status().panel.is_some_and(|p| p.mode == "Program"));
    b.wait("the Presets button to light for Program", || b.panel.lit(PRESETS) == Lit::On);
}

fn bench(name: &str) -> Bench {
    let b = Bench::empty(name);
    b.setup_cameras(2);
    b.take_lease();
    b.new_song();
    program_mode(&b);
    b.wait("the tape keys", || b.title(K1) == "Back");
    b
}

fn frame(b: &Bench) -> i64 {
    b.status().frame
}

fn tap(b: &Bench, key: u32, to: i64) {
    b.press(key);
    b.wait(&format!("the playhead to reach {to}"), || frame(b) == to);
}

fn stands(b: &Bench) -> i64 {
    let mut last = -1;
    b.wait("the playhead to stand still", || {
        let now = frame(b);
        let still = now == last;
        last = now;
        std::thread::sleep(Duration::from_millis(250));
        still
    });
    last
}

#[test]
fn back_and_forward_jump_on_a_tap_and_wind_on_a_hold() {
    let b = bench("program-tape");
    let length = b.song().song.length_frames;
    assert_eq!(
        KEYS.map(|k| (b.title(k), b.line(k))).to_vec(),
        [
            ("Back", "5 s"),
            ("", "Play"),
            ("Forward", "5 s"),
            ("Playhead", "To start"),
            ("", ""),
            ("Song", "Save"),
            ("Edit", "Undo")
        ]
        .map(|(t, l)| (t.to_string(), l.to_string()))
    );

    // A tap is five seconds; at 25 frames a second, 125 frames.
    tap(&b, K3, 125);
    tap(&b, K3, 250);
    tap(&b, K1, 125);

    // With Shift a tap is one frame, and the keys say so.
    b.shift(|| {
        b.wait("the keys to show a frame", || b.line(K1) == "1 frame" && b.line(K3) == "1 frame");
        tap(&b, K3, 126);
        tap(&b, K3, 127);
        tap(&b, K1, 126);
    });
    b.wait("the keys to show seconds again", || b.line(K1) == "5 s");

    // Most of a tap's time is still a tap.
    b.hold(K3, TAP - Duration::from_millis(120));
    b.wait("the jump", || frame(&b) == 251);
    assert_eq!(stands(&b), 251);

    // A hold winds, at about eight times speed, and says so. It stops when the key is let go.
    b.panel.down(K3);
    b.wait_reported("Winding forward at 8 times speed. Let go to stop");
    b.wait("the main display to say it winds", || b.title(MESSAGE_TILE).starts_with("Wind"));
    std::thread::sleep(Duration::from_millis(1000));
    b.panel.up(K3);
    let at = stands(&b);
    // 1 s and more at 200 frames a second, less what the bench itself takes.
    assert!((251 + 150..251 + 600).contains(&at), "a hold of over a second wound from 251 to {at}");
    assert_ne!((at - 251) % 125, 0, "the wind is no jump");

    // And back.
    b.panel.down(K1);
    b.wait_reported("Winding back at 8 times speed. Let go to stop");
    b.wait("the tape to wind back", || frame(&b) < at - 100);
    b.panel.up(K1);
    let back = stands(&b);
    assert!(back < at - 100 && back > 0, "wound back from {at} to {back}");

    // Shift and a hold: frame by frame, about ten a second.
    b.panel.down(SHIFT);
    b.wait("the keys to show a frame", || b.line(K3) == "1 frame");
    b.panel.down(K3);
    b.wait_reported("Stepping forward frame by frame. Let go to stop");
    std::thread::sleep(Duration::from_millis(1000));
    b.panel.up(K3);
    b.panel.up(SHIFT);
    let stepped = stands(&b) - back;
    assert!((5..=25).contains(&stepped), "a second of stepping moved {stepped} frames");

    // The ends of the song stop it.
    b.press_key("To start");
    b.wait("the playhead at the start", || frame(&b) == 0);
    b.press(K1);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(frame(&b), 0);
    b.panel.down(K3);
    b.wait("the tape to reach the end", || frame(&b) == length);
    std::thread::sleep(Duration::from_millis(400));
    b.panel.up(K3);
    assert_eq!(stands(&b), length, "the wind stops at the end of the song");
}

#[test]
fn one_key_plays_and_pauses_and_the_playhead_stays() {
    let b = bench("program-play");
    tap(&b, K3, 125);
    assert_eq!((b.line(K2).as_str(), b.light(K2).0), ("Play", Lit::Dim));
    b.press(K2);
    b.wait("the show to play", || b.status().transport.run != Run::Stopped);
    b.wait("the key to offer Pause", || b.line(K2) == "Pause" && b.light(K2) == (Lit::On, Color::Green));
    b.wait("the playhead to run", || frame(&b) > 135);

    // A jump while it plays: it plays on from there.
    let before = frame(&b);
    b.press(K3);
    b.wait("the jump while playing", || frame(&b) >= before + 125);
    assert_ne!(b.status().transport.run, Run::Stopped);

    b.press(K2);
    b.wait("the show to pause", || b.status().transport.run == Run::Stopped);
    let at = stands(&b);
    assert!(at >= before + 125, "Pause keeps the playhead where it was: {at}");
    b.wait("the key to offer Play", || b.line(K2) == "Play");
    assert_eq!(b.status().transport.output, Output::Rehearse);
}

#[test]
fn a_wind_stops_when_the_panel_is_lost() {
    let b = bench("program-lost");

    // The connection closes with the key down.
    b.panel.down(K3);
    b.wait("the tape to wind", || frame(&b) > 100);
    b.panel.drop_connections();
    let at = stands(&b);
    assert!(at < b.song().song.length_frames, "the wind stopped short of the end: {at}");
    b.wait_ready();
    b.wait("the tape keys again", || b.title(K1) == "Back");
    assert_eq!(stands(&b), at, "it does not start again when the panel is back");

    // The link goes silent with the key down: no close, the panel just stops answering.
    b.panel.down(K3);
    b.wait("the tape to wind", || frame(&b) > at + 100);
    b.panel.set_silent(true);
    let silent = stands(&b);
    assert!(silent < b.song().song.length_frames, "the wind stopped short of the end: {silent}");
    b.wait_reported("The panel stopped answering. Winding stopped");
    b.panel.set_silent(false);
    b.panel.up(K3);
    assert_eq!(stands(&b), silent);

    // A release that never arrives is not taken for a hold: the key goes down and the link goes
    // silent within the time of a tap.
    b.panel.down(K1);
    b.panel.set_silent(true);
    std::thread::sleep(Duration::from_millis(1200));
    assert_eq!(frame(&b), silent, "nothing winds on a press the panel was not heard from after");
    b.panel.set_silent(false);
}

#[test]
fn a_camera_is_marked_where_it_stands_and_its_second_press_places_the_move() {
    let b = bench("program-mark");
    assert!(b.api.presets().unwrap().is_empty());
    assert_eq!((b.line(K5).as_str(), b.light(K5).0), ("", Lit::Off), "there is no mark to cancel");

    // Set camera 1, at five seconds, and press its number.
    tap(&b, K3, 125);
    let pan = b.drive(JOY_PAN, 0, |p| p.pan);
    b.press(CAM1);
    b.wait_said("Cam 1 marked at 01:00:05:00 as new Preset 1. Press 1 again where it must have arrived");
    let presets = b.api.presets().unwrap();
    assert_eq!(
        presets.iter().map(|p| (p.name.as_str(), p.camera.as_str(), p.pose.pan)).collect::<Vec<_>>(),
        [("Preset 1", "cam1", pan)]
    );
    b.wait("camera 1's key to flash", || (b.panel.lit(CAM1), b.panel.color(CAM1)) == (Lit::Blink, Some(Color::Amber)));
    assert_ne!(b.panel.lit(CAM2), Lit::Blink);
    assert_eq!((b.line(K5).as_str(), b.light(K5)), ("Cancel", (Lit::On, Color::Amber)));
    assert!(b.song().song.motion.is_empty(), "the first press places nothing");

    // Ten seconds on, the same number again: the move leaves at the first press and has arrived here.
    tap(&b, K3, 250);
    tap(&b, K3, 375);
    b.press(CAM1);
    b.wait_said("Cam 1 moves to Preset 1 from 01:00:05:00 to 01:00:15:00, 10.00 s");
    let song = b.song().song;
    assert_eq!(song.motion.len(), 1);
    let clip = &song.motion[0];
    assert_eq!((clip.camera.as_str(), clip.start_frame, clip.end_frame()), ("cam1", 125, 375));
    assert!(
        matches!(&clip.body, ClipBody::PresetMove { to, auto_duration: false, .. } if to.preset_id == presets[0].id),
        "a move to the preset, as long as it was marked: {:?}",
        clip.body
    );
    b.wait("camera 1's key to stop flashing", || b.panel.lit(CAM1) == Lit::On);
    assert_eq!(b.status().panel.unwrap().clips, std::slice::from_ref(&clip.id), "the move is the selection");

    // The same pose again is the preset it already has, and cancelling keeps that preset.
    tap(&b, K3, 500);
    b.press(CAM1);
    b.wait_said("Cam 1 is within range of Preset 1: using it. Marked at 01:00:20:00. Press 1 again where it must have arrived");
    assert_eq!(b.api.presets().unwrap().len(), 1);
    b.press_key("Cancel");
    b.wait_said("Cam 1 is no longer marked. Preset 1 is kept");
    assert_eq!(b.api.presets().unwrap().len(), 1);
    b.wait("the key to stop flashing", || b.panel.lit(CAM1) != Lit::Blink);

    // A new pose is a new preset, and cancelling that mark takes it away again.
    b.drive(JOY_TILT, 0, |p| p.tilt);
    b.press(CAM1);
    b.wait_said("Cam 1 marked at 01:00:20:00 as new Preset 2. Press 1 again where it must have arrived");
    assert_eq!(b.api.presets().unwrap().len(), 2);
    b.press_key("Cancel");
    b.wait_said("Cam 1 is no longer marked. Preset 2 is removed");
    assert_eq!(b.api.presets().unwrap().iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Preset 1"]);
    assert_eq!(b.song().song.motion.len(), 1);

    // Two cameras marked at once, each finished at its own time. Camera 2 has never been moved.
    b.press(CAM1);
    b.wait_said_start("Cam 1 marked at 01:00:20:00 as new Preset 2");
    b.press(CAM2);
    b.wait_said_start("Cam 2 marked at 01:00:20:00 as new Preset 1");
    b.wait("both keys to flash", || b.panel.lit(CAM1) == Lit::Blink && b.panel.lit(CAM2) == Lit::Blink);
    b.wait("the display to count the marks", || b.status().panel.is_some_and(|p| p.camera == "cam2"));
    tap(&b, K3, 625);
    b.press(CAM2);
    b.wait_said("Cam 2 moves to Preset 1 from 01:00:20:00 to 01:00:25:00, 5.00 s");
    assert_eq!((b.panel.lit(CAM1), b.panel.lit(CAM2)), (Lit::Blink, Lit::On));
    tap(&b, K3, 750);
    b.press(CAM1);
    b.wait_said("Cam 1 moves to Preset 2 from 01:00:20:00 to 01:00:30:00, 10.00 s");
    let song = b.song().song;
    let mut moves: Vec<(&str, i64, i64)> =
        song.motion.iter().map(|c| (c.camera.as_str(), c.start_frame, c.end_frame())).collect();
    moves.sort();
    assert_eq!(moves, [("cam1", 125, 375), ("cam1", 500, 750), ("cam2", 500, 625)]);

    // Undo takes the last move back.
    b.press_key("Undo");
    b.wait("the last move to be undone", || b.song().song.motion.len() == 2);

    // The second press at the mark's own time, or before it: the move goes there with its auto time.
    b.press(CAM1);
    // With that move undone the simulator shows the rig where the song leaves it: at Preset 1.
    b.wait_said_start("Cam 1 is within range of Preset 1: using it. Marked at 01:00:30:00");
    b.press(CAM1);
    b.wait_said("Cam 1: move to Preset 1 placed at 01:00:30:00. It takes its auto time");
    let song = b.song().song;
    let last = song.motion.iter().find(|c| c.camera == "cam1" && c.start_frame == 750).expect("the move at the mark");
    assert!(matches!(&last.body, ClipBody::PresetMove { auto_duration: true, .. }));
}

#[test]
fn a_move_that_cannot_be_placed_keeps_its_mark_and_marks_end_with_the_lease() {
    let b = bench("program-refused");
    tap(&b, K3, 125);
    b.drive(JOY_PAN, 0, |p| p.pan);
    b.press(CAM1);
    b.wait_said_start("Cam 1 marked at 01:00:05:00 as new Preset 1");
    tap(&b, K3, 250);
    b.press(CAM1);
    b.wait_said_start("Cam 1 moves to Preset 1 from 01:00:05:00 to 01:00:10:00");

    // A second move across the first is refused, and the key goes on flashing. Back at the start
    // the simulator shows the rig where the song has it there, which is a pose of its own.
    tap(&b, K1, 125);
    tap(&b, K1, 0);
    b.press(CAM1);
    b.wait_said_start("Cam 1 marked at 01:00:00:00 as new Preset 2");
    tap(&b, K3, 125);
    tap(&b, K3, 250);
    tap(&b, K3, 375);
    b.press(CAM1);
    b.wait("the refusal", || b.title(MESSAGE_TILE).starts_with("Refused"));
    assert_eq!(b.song().song.motion.len(), 1, "nothing was placed");
    assert_eq!(b.panel.lit(CAM1), Lit::Blink, "the mark stays for another place");

    // Another station takes the lease over: the mark cannot be finished from here any more.
    let other = b.server.client("desk");
    other.take_lease("Desk", true).expect("the desk takes the lease over");
    b.wait_reported("The panel no longer holds the edit lease: the marks are dropped. Their presets are kept");
    b.wait("the key to stop flashing", || b.panel.lit(CAM1) != Lit::Blink);
    assert_eq!(b.api.presets().unwrap().len(), 2);
}

#[test]
fn nothing_is_programmed_while_the_show_is_armed() {
    let b = Bench::empty("program-armed");
    b.setup_cameras(1);
    b.program();
    b.mode(TRANSPORT);
    let arm = b.wait_key("Arm");
    b.hold_until(arm, "the hold to arm", || b.status().transport.output != Output::Rehearse);
    b.wait("the show to be armed", || b.status().transport.output == Output::Armed);

    program_mode(&b);
    let (at, presets) = (frame(&b), b.api.presets().unwrap().len());
    assert!(KEYS.iter().all(|k| b.line(*k).is_empty() && b.panel.lit(*k) == Lit::Off), "every tape key is dark");
    b.press(K3);
    b.wait_said(ARMED);
    b.panel.hold(K3, Duration::from_millis(900));
    b.press(CAM1);
    b.wait_said(ARMED);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!((frame(&b), b.api.presets().unwrap().len()), (at, presets), "nothing moved and nothing was stored");
    assert_ne!(b.panel.lit(CAM1), Lit::Blink);
}
