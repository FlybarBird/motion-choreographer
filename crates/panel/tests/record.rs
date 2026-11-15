//! A pass recorded by hand from the panel, and its refit.

mod common;

use common::*;
use ptz_core::pose::Axis;
use ptz_core::song::Song;
use ptz_core::transport::Run;
use ptz_panel::codec::{Color, Lit};
use std::time::Duration;

fn keys_of(song: &Song, clip: &str) -> usize {
    let clip = song.motion_clip(clip).expect("the recorded clip");
    Axis::ALL.iter().map(|a| clip.keys(*a).len()).sum()
}

#[test]
fn a_pass_is_recorded_by_hand_and_refitted() {
    let b = Bench::empty("record");
    b.take_lease();
    b.new_song();

    // Choose the axes: Pan and Tilt are on to begin with. Zoom on, Tilt off.
    b.mode(RECORD);
    assert_eq!((b.line(K1).as_str(), b.light(K1)), ("Pan", (Lit::On, Color::Red)));
    assert_eq!((b.line(K2).as_str(), b.light(K2)), ("Tilt", (Lit::On, Color::Red)));
    assert_eq!((b.line(K3).as_str(), b.light(K3)), ("Zoom", (Lit::Dim, Color::White)));
    b.press(K3);
    b.wait("Zoom to be record-enabled", || b.light(K3) == (Lit::On, Color::Red));
    b.press(K2);
    b.wait("Tilt to be taken out", || b.light(K2) == (Lit::Dim, Color::White));

    // Record on.
    assert_eq!((b.title(K6).as_str(), b.line(K6).as_str()), ("Record", "Off"));
    b.press(K6);
    b.wait("Record to be on", || b.status().transport.record);
    b.wait_said("Record is on for 1 Camera 1: Pan, Zoom. Play, then drive by hand");
    let s = b.status();
    assert_eq!((s.record.camera.as_str(), s.record.axes.clone()), ("cam1", vec![Axis::Pan, Axis::Zoom]));
    assert_eq!((b.line(K6).as_str(), b.light(K6)), ("On", (Lit::On, Color::Red)));

    // Play with the primary key, named as the server names it for the lease holder, and drive.
    b.wait("the primary key to offer a rehearsal", || b.line(GO) == "Rehearse");
    b.press(GO);
    b.wait("the show to play", || b.status().transport.run == Run::Playing);
    b.wait("the status display to say Record", || b.text(STATUS_TILE).line1.ends_with(" Record"));
    b.panel.deflect(JOY_PAN, 400);
    b.panel.deflect(JOY_TILT, 400);
    std::thread::sleep(Duration::from_millis(600));
    b.panel.deflect(JOY_ZOOM, 300);
    std::thread::sleep(Duration::from_millis(600));
    b.panel.deflect(JOY_PAN, -250);
    std::thread::sleep(Duration::from_millis(500));
    for axis in [JOY_PAN, JOY_TILT, JOY_ZOOM] {
        b.panel.deflect(axis, 0);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert!(b.status().record.frames > 30, "frames are being captured: {}", b.status().record.frames);
    b.press_key("Stop");
    b.wait("the show to stop", || b.status().transport.run == Run::Stopped);

    // The pass lands as a keys clip, is selected, and is one Undo away.
    b.wait("the pass to land on the timeline", || b.song().song.motion.len() == 1);
    b.wait_said("The pass is on the timeline as a keys clip. Undo removes it");
    let recorded = b.song().song;
    let clip = recorded.motion[0].clone();
    assert!(clip.is_keys() && clip.recorded_pass.is_some(), "{clip:?}");
    assert_eq!(
        (clip.camera.as_str(), clip.axes.clone()),
        ("cam1", vec![Axis::Pan, Axis::Zoom]),
        "only the enabled axes were written"
    );
    assert!(clip.keys(Axis::Pan).len() >= 2 && clip.keys(Axis::Zoom).len() >= 2);
    b.wait("the pass to be the panel's selection", || {
        b.status().panel.is_some_and(|p| p.clips == std::slice::from_ref(&clip.id))
    });
    let original = keys_of(&recorded, &clip.id);

    // Fewer keys, then More keys: Shift and the first two keys.
    b.panel.down(SHIFT);
    b.wait("the refit keys to show", || b.line(K1) == "Fewer keys" && b.line(K2) == "More keys");
    b.press(K1);
    let said = b.wait_said_start("Fewer keys: ");
    let fewer = keys_of(&b.song().song, &clip.id);
    assert_eq!(said, format!("Fewer keys: {original} keys before, {fewer} now"));
    assert!(fewer <= original, "{fewer} keys after Fewer keys, {original} before");
    b.press(K2);
    let said = b.wait_said_start("More keys: ");
    let more = keys_of(&b.song().song, &clip.id);
    assert_eq!(said, format!("More keys: {fewer} keys before, {more} now"));
    assert!(more > fewer, "{more} keys after More keys, {fewer} before");
    b.panel.up(SHIFT);

    // Undo steps back through the refits and then takes the pass away.
    b.wait_key("Undo");
    b.press_key("Undo");
    b.wait("the last refit to be undone", || keys_of(&b.song().song, &clip.id) == fewer);
    b.press_key("Undo");
    b.wait("the first refit to be undone", || b.song().song == recorded);
    b.press_key("Undo");
    b.wait("the pass to be taken off", || b.song().song.motion.is_empty());

    // Stopping turned Record off. The key turns it on and off again.
    assert!(!b.status().transport.record);
    b.wait("the Record key to say Off", || b.line(K6) == "Off");
    b.press(K6);
    b.wait("Record to be on", || b.status().transport.record);
    b.wait("the Record key to say On", || b.line(K6) == "On");
    b.press(K6);
    b.wait("Record to be off", || !b.status().transport.record);
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}
