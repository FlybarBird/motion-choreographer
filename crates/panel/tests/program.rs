//! A whole show programmed from the panel alone, after the base setup.

mod common;

use common::*;
use ptz_core::song::{ClipBody, Ease};
use ptz_core::transport::{Output, Run};
use ptz_panel::codec::{Color, Lit};

#[test]
fn a_show_is_programmed_from_nothing_with_the_panel_alone() {
    let b = Bench::empty("program");
    b.setup_cameras(2);
    assert!(b.status().song.is_none() && b.api.presets().unwrap().is_empty(), "the show starts empty");

    // Without the lease the panel watches, and says so.
    assert_eq!(b.line(GO), "", "the primary key is dark while the panel only watches");
    b.take_lease();
    assert_eq!(b.light(K1), (Lit::On, Color::Blue));

    b.new_song();
    b.wait_said("Song 1 starts at 01:00:00:00 and is loaded");
    let song = b.song().song;
    assert_eq!((song.name.as_str(), song.start_tc.as_str()), ("Song 1", "01:00:00:00"));
    b.wait("the status display to name the song", || b.title(STATUS_TILE) == "Song 1");
    b.wait("the primary key to be named as the server's guidance names it", || b.line(GO) == "Rehearse");

    // Camera 2, by its select key. The app can follow: the server passes the selection on.
    b.press(CAM2);
    b.wait("camera 2 to be selected", || b.status().panel.is_some_and(|p| p.camera == "cam2"));
    assert_eq!(b.panel.lit(CAM2), Lit::On);
    assert_eq!(b.panel.lit(CAM1), Lit::Dim);

    // Three poses by hand, each stored as a preset.
    b.mode(CAMERA);
    assert_eq!((b.title(K2).as_str(), b.line(K2).as_str()), ("Cam 2", "Camera 2 Sim"));
    assert_eq!(b.light(K2), (Lit::On, Color::Blue));
    let pan = b.drive(JOY_PAN, 1, |p| p.pan);
    b.press_key("Store");
    b.wait_said("Stored Preset 1 for 2 Camera 2");
    let tilt = b.drive(JOY_TILT, 1, |p| p.tilt);
    b.press_key("Store");
    b.wait_said("Stored Preset 2 for 2 Camera 2");
    let zoom = b.drive(JOY_ZOOM, 1, |p| p.zoom);
    b.press_key("Store");
    b.wait_said("Stored Preset 3 for 2 Camera 2");
    let presets = b.api.presets().unwrap();
    assert_eq!(
        presets.iter().map(|p| (p.name.as_str(), p.camera.as_str())).collect::<Vec<_>>(),
        [("Preset 1", "cam2"), ("Preset 2", "cam2"), ("Preset 3", "cam2")]
    );
    assert!(pan != 0.0 && tilt != 0.0 && zoom != 0.0);
    assert_eq!((presets[0].pose.pan, presets[0].pose.tilt), (pan, 0.0), "preset 1 is the pose driven to");
    assert_eq!((presets[1].pose.pan, presets[1].pose.tilt), (pan, tilt));
    assert_eq!((presets[2].pose.tilt, presets[2].pose.zoom), (tilt, zoom));

    // Three preset moves at three times.
    b.mode(PRESETS);
    for (n, frame) in [(1, 50), (2, 300), (3, 600)] {
        b.playhead_to(frame);
        b.press_key(&format!("Preset {n}"));
        b.wait(&format!("the move to preset {n}"), || b.song().song.motion.len() == n);
    }
    let song = b.song().song;
    assert_eq!(
        song.motion.iter().map(|c| (c.camera.as_str(), c.start_frame)).collect::<Vec<_>>(),
        [("cam2", 50), ("cam2", 300), ("cam2", 600)]
    );
    assert!(
        song.motion.iter().all(|c| matches!(&c.body, ClipBody::PresetMove { auto_duration: true, .. })),
        "moves take their auto time"
    );
    let third = song.motion[2].clone();
    assert_eq!(
        b.status().panel.unwrap().clips,
        std::slice::from_ref(&third.id),
        "the move just placed is the selection the app is told of"
    );

    // Lengthen the third move: Shift makes a knob step one second.
    b.mode(TIMELINE);
    b.wait("the main display to describe the selected clip", || b.title(MESSAGE_TILE).starts_with("Clip 3/3"));
    assert_eq!(b.title(E2_TILE), "Length");
    b.shift(|| b.turn(E2, 2));
    b.wait("the move to be two seconds longer", || b.song().song.motion[2].duration_frames == third.duration_frames + 50);
    assert!(
        matches!(&b.song().song.motion[2].body, ClipBody::PresetMove { auto_duration: false, .. }),
        "a set length is a custom time"
    );

    // Its ease: the key shows the ease in force and steps to the next.
    assert_eq!((b.title(K3).as_str(), b.line(K3).as_str()), ("Ease", "Ease in-out"));
    b.press(K3);
    b.wait("the ease to change", || matches!(&b.song().song.motion[2].body, ClipBody::PresetMove { ease: Ease::Linear, .. }));
    b.wait("the key to show the new ease", || b.line(K3) == "Linear");

    // The clip before it, then a marker at its start.
    b.press_key("Prev");
    b.wait("the playhead to go to the second move", || b.status().frame == 300);
    b.shift(|| b.press(K5));
    b.wait_said("Marker 1 added at 01:00:12:00");
    assert_eq!(b.song().song.markers.iter().map(|m| (m.name.as_str(), m.frame)).collect::<Vec<_>>(), [("Marker 1", 300)]);

    // Undo takes the marker away; Redo brings it back.
    b.press_key("Undo");
    b.wait_said("Undone. Shift and Undo is Redo");
    assert!(b.song().song.markers.is_empty());
    b.shift(|| b.press(K7));
    b.wait_said("Redone");
    assert_eq!(b.song().song.markers.len(), 1);

    // Save, then commit.
    b.mode(SHOW);
    assert!(b.status().song.unwrap().dirty);
    b.wait("unsaved changes to be marked on the status display", || b.title(STATUS_TILE) == "Song 1*");
    assert_eq!(b.light(b.key("Save")), (Lit::On, Color::Amber));
    b.press_key("Save");
    b.wait_said("Saved Song 1");
    assert!(!b.status().song.unwrap().dirty);
    b.wait("Commit to ask for attention", || b.light(b.key("Commit")) == (Lit::On, Color::Amber));
    b.press_key("Commit");
    b.wait_said("Committed Song 1 as revision 1");
    let state = b.status().song.unwrap();
    assert_eq!((state.revision, state.committed_clean), (Some(1), true));

    // Rehearse, stop, cue: the primary key carries the server's name for each.
    b.press_key("Rehearse");
    b.wait("the show to play", || b.status().transport.run == Run::Playing);
    assert_eq!(b.status().transport.output, Output::Rehearse);
    b.wait("the status display to say Rehearse", || b.text(STATUS_TILE).line1.ends_with(" Rehearse"));
    b.wait("the playhead to run", || b.status().frame > 310);
    b.press_key("Stop");
    b.wait("the show to stop", || b.status().transport.run == Run::Stopped);
    b.wait("the status display to say Stopped", || b.text(STATUS_TILE).line1.ends_with(" Stopped"));
    b.mode(TRANSPORT);
    b.press_key("Cue");
    b.wait("the playhead to return to the start", || b.status().frame == 0);
    assert_eq!(b.text(STATUS_TILE).line1, "01:00:00:00 Stopped");
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}
