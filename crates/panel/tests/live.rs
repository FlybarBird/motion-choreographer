//! Arm, Play Live, Stop and Disarm from the panel: on the simulator, and on the
//! FR7 and AutoPod+ protocol stand-ins. Arm and Play Live move a rig, so each
//! takes a hold and says what it will do first; a tap does nothing but say so.

mod common;

use common::*;
use ptz_core::transport::{Output, Run};
use ptz_panel::codec::{Color, Lit};
use std::time::Duration;

const HOLD: Duration = Duration::from_millis(950);

/// From the panel: a committed song with one long pan for camera 1, starting at frame 50.
fn program_a_long_move(b: &Bench, rig: Option<&Rig>) {
    b.take_lease();
    b.new_song();
    b.mode(CAMERA);
    match rig {
        // The server drives only the simulator by hand. A real camera is put in place with its own
        // controller, and the panel shows the server's reason when its joystick is tried.
        Some(rig) => {
            let (_, tilt, zoom, focus) = rig.fr7.pose();
            rig.fr7.place(40.0, tilt, zoom, focus);
            b.wait("the server to read where the rig stands", || (b.status().first().rig_pose.pan - 40.0).abs() < 0.5);
            b.panel.deflect(JOY_PAN, 500);
            b.wait_refused("The rig controller drives the simulator only. Use the camera's own controller for hardware");
            b.panel.deflect(JOY_PAN, 0);
            assert!(!rig.is_moving() && (rig.pose().pan - 40.0).abs() < 0.01, "the joystick did not move the rig");
        }
        None => {
            b.turn(E3, 15);
            b.wait("top speed to read 100%", || b.line(E3_TILE) == "100%");
            b.panel.deflect(JOY_PAN, 500);
            b.wait("the camera to pan well away from home", || b.status().first().pose.pan.abs() > 25.0);
            b.panel.deflect(JOY_PAN, 0);
            let mut last = f64::NAN;
            b.wait("the camera to stand still", || {
                let now = b.status().first().pose.pan;
                let still = now == last;
                last = now;
                std::thread::sleep(Duration::from_millis(120));
                still
            });
        }
    }
    b.press_key("Store");
    b.wait_said_start("Stored Preset 1");
    b.mode(PRESETS);
    b.playhead_to(50);
    b.press(K2);
    b.wait("the move to be placed", || b.song().song.motion.len() == 1);
    b.mode(SHOW);
    b.press_key("Save");
    b.wait_said_start("Saved ");
    b.press_key("Commit");
    b.wait_said_start("Committed ");
}

fn transport_log(b: &Bench) -> Vec<(String, String)> {
    let word = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
    b.log().iter().filter(|r| r.kind == "transport").map(|r| (word(&r.data["action"]), word(&r.data["by"]))).collect()
}

fn arm_by_hold(b: &Bench, rig: Option<&Rig>) {
    b.mode(TRANSPORT);
    let arm = b.wait_key("Arm");
    assert_eq!(b.title(arm), "Hold", "the Arm key says it takes a hold");
    assert_eq!(b.light(arm), (Lit::Dim, Color::Green));

    // A tap arms nothing and says what a hold would do, in the app's words.
    b.press(arm);
    b.wait_said("Hold Arm for 1 second. Arm (moves rig to playhead)");
    // Most of a hold arms nothing either.
    b.hold(arm, Duration::from_millis(700));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(b.status().transport.output, Output::Rehearse, "a tap and a short hold do not arm");
    assert!(!b.status().hardware_live);
    if let Some(rig) = rig {
        assert!(!rig.is_moving(), "the rig has not been moved");
    }

    let began = std::time::Instant::now();
    b.panel.down(arm);
    b.wait_said("Arm (moves rig to playhead). Keep holding");
    b.wait("the hold to arm", || b.status().transport.output != Output::Rehearse);
    let took = began.elapsed();
    b.panel.up(arm);
    assert!(took >= HOLD, "armed after only {took:?} of holding");
    b.wait("the rig to be at the playhead", || b.status().transport.output == Output::Armed);
    b.wait("the status display to say Armed", || b.text(STATUS_TILE).line1.ends_with(" Armed"));
}

fn play_live_by_hold(b: &Bench) {
    b.wait("the primary key to offer Play Live", || b.line(GO) == "Play Live");
    assert_eq!(b.title(GO), "Hold", "the Play Live key says it takes a hold");
    assert_eq!(b.light(GO), (Lit::On, Color::Green));
    b.mode(TRANSPORT);
    assert_eq!((b.title(K1).as_str(), b.line(K1).as_str()), ("Hold", "Play Live"));

    // A tap of either Play key plays nothing.
    b.press(GO);
    b.wait_said_start("Hold Play Live for 1 second. ");
    b.press(K1);
    b.wait_said("Hold Play Live for 1 second. Play Live moves the rig");
    b.hold(GO, Duration::from_millis(700));
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(b.status().transport.run, Run::Stopped, "a tap and a short hold do not play live");

    let took = b.hold_until(GO, "the hold to play live", || b.status().transport.run == Run::Playing);
    assert!(took >= HOLD, "playing live after only {took:?} of holding");
    assert_eq!(b.status().transport.output, Output::Armed);
    b.wait("the status display to say LIVE", || b.text(STATUS_TILE).line1.ends_with(" LIVE"));
}

fn live(b: &Bench, rig: Option<&Rig>) {
    program_a_long_move(b, rig);
    let preset = b.api.presets().expect("presets")[0].pose;
    let (start, length) = {
        let song = b.song().song;
        (song.motion[0].start_frame, song.motion[0].duration_frames)
    };
    assert!(length > 25, "the move is long enough to catch in the middle: {length} frames");

    arm_by_hold(b, rig);
    assert!(b.status().hardware_live, "the output is live");
    if let Some(rig) = rig {
        // The rig stood at the preset; the playhead is at the start of the move, where the timeline says home.
        assert!(rig.pose().pan.abs() < 0.5, "Arm moved the rig to the playhead: {:?}", rig.pose());
        assert_eq!(b.panel.color(CAM1), Some(Color::Green), "the camera key shows a live rig");
    }

    play_live_by_hold(b);
    b.wait("the move to be under way", || b.status().frame > start + length / 3);
    if let Some(rig) = rig {
        assert!(rig.is_moving(), "the rig follows the timeline");
    }

    // Hand control is refused while the rig is live, in the server's words.
    b.mode(CAMERA);
    b.panel.deflect(JOY_TILT, 300);
    b.wait_refused("Disarm to drive the rig by hand");
    b.panel.deflect(JOY_TILT, 0);

    // The primary key is Stop now; it stops the show and stays armed.
    b.press_key("Stop");
    b.wait("the show to stop", || b.status().transport.run == Run::Stopped);
    assert_eq!(b.status().transport.output, Output::Armed);
    if let Some(rig) = rig {
        b.wait("the rig to stand still", || !rig.is_moving());
        assert!(rig.pose().pan.abs() > 1.0, "the rig moved towards the preset at {}: {:?}", preset.pan, rig.pose());
    }

    b.mode(TRANSPORT);
    b.press_key("Disarm");
    b.wait("the show to be disarmed", || b.status().transport.output == Output::Rehearse);
    assert!(!b.status().hardware_live);
    b.wait("the status display to say Stopped", || b.text(STATUS_TILE).line1.ends_with(" Stopped"));

    let acts: Vec<(String, String)> =
        transport_log(b).into_iter().filter(|(a, _)| ["arm", "play", "stop", "disarm"].contains(&a.as_str())).collect();
    let panel = |a: &str| (a.to_string(), "Panel".to_string());
    assert_eq!(acts, [panel("arm"), panel("play"), panel("stop"), panel("disarm")], "each done once, and logged as the panel's");

    // E-Stop while live: the show stops, the output is let go, and the rig stands still.
    b.press_key("Cue");
    b.wait("the playhead to return to the start", || b.status().frame == 0);
    arm_by_hold(b, rig);
    play_live_by_hold(b);
    b.wait("the move to be under way again", || b.status().frame > start + length / 3);
    if let Some(rig) = rig {
        assert!(rig.is_moving());
    }
    b.press(ESTOP);
    b.wait("E-Stop to latch", || b.status().transport.estop);
    let s = b.status();
    assert_eq!((s.transport.run, s.transport.output, s.hardware_live), (Run::Stopped, Output::Rehearse, false));
    b.wait("the status display to say E-STOP", || b.text(STATUS_TILE).line1.ends_with(" E-STOP"));
    if let Some(rig) = rig {
        b.wait("the rig to have halted", || !b.status().telemetry.halting && !rig.is_moving());
        let held = rig.pose();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(rig.pose(), held, "the rig stays where E-Stop left it");
        assert!((held.pan - preset.pan).abs() > 1.0, "stopped short of the end of the move: {held:?}");
    }
    assert!(b.panel.unknown().is_empty(), "the panel understood every line: {:?}", b.panel.unknown());
}

#[test]
fn arm_and_play_live_take_a_hold_on_the_simulator() {
    let b = Bench::empty("live-sim");
    live(&b, None);
}

#[test]
fn arm_and_play_live_take_a_hold_on_the_fr7_and_autopod_stand_ins() {
    let b = Bench::empty("live-rig");
    let rig = Rig::connect(&b);
    live(&b, Some(&rig));
}
