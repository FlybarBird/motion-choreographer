//! E-STOP and the hand controls: what must work in every state of the window.
//! The pointer is moved and pressed as an operator's is, so a click lands on
//! whatever is really under it.

use super::*;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

fn song() -> Song {
    Song::new("s", "Test Song", "01:00:00:00", 1500)
}

fn acts(r: &Rig) -> Vec<Action> {
    r.cmds.try_iter().filter_map(|c| if let Cmd::Act(a, _) = c { Some(a) } else { None }).collect()
}

fn rect_of(r: &Rig, label: &str) -> egui::Rect {
    r.harness.get_by_label(label).rect()
}

/// Everything on screen that carries words, with the rectangle it was laid out in.
fn worded(r: &Rig) -> Vec<(String, egui::Rect)> {
    let mut found = Vec::new();
    for node in r.harness.query_all(egui_kittest::kittest::by().predicate(|_| true)) {
        let ak = node.accesskit_node();
        let words = ak.label().or_else(|| ak.value()).unwrap_or_default();
        if !words.is_empty() {
            found.push((words, node.rect()));
        }
    }
    found
}

/// What else is laid out on the place the button with this label is drawn in.
fn laid_over(r: &Rig, label: &str) -> Vec<String> {
    let button = rect_of(r, label);
    worded(r)
        .into_iter()
        .filter(|(words, _)| words != label)
        .filter(|(_, rect)| {
            let both = rect.intersect(button);
            both.width() > 0.5 && both.height() > 0.5
        })
        .map(|(words, rect)| format!("{words:?} at {rect:?}"))
        .collect()
}

/// Click all over the button and return the places where the click did not send `action`.
fn dead_points(r: &mut Rig, label: &str, action: Action) -> Vec<Pos2> {
    let button = rect_of(r, label);
    let _ = r.cmds.try_iter().count();
    let mut dead = Vec::new();
    for y in [button.top() + 2.0, button.center().y, button.bottom() - 2.0] {
        let mut x = button.left() + 2.0;
        while x < button.right() - 1.0 {
            r.click(pos2(x, y));
            if acts(r) != vec![action] {
                dead.push(pos2(x, y));
            }
            x += 7.0;
        }
    }
    dead
}

/// The button is inside the window, alone on its place, and answers wherever it is clicked.
fn assert_whole(r: &mut Rig, window: egui::Vec2, label: &str, action: Action) {
    let button = rect_of(r, label);
    assert!(egui::Rect::from_min_size(Pos2::ZERO, window).contains_rect(button), "{label} at {button:?} is inside {window:?}");
    let over = laid_over(r, label);
    assert!(over.is_empty(), "{window:?}: {label} is at {button:?}, and laid out over it: {over:#?}");
    let dead = dead_points(r, label, action);
    assert!(dead.is_empty(), "{window:?}: {label} is at {button:?}; clicks at {dead:?} did nothing");
}

/// More than the bar is likely to be asked to show at once: locked to a file with a long name, a problem
/// with the timecode input, Record on, and a fault. The texts are written for this test to be long; they
/// are not quoted from the server.
const LONG_FAULT: &str = "12 Balcony stage left: AutoPod+ at 192.168.10.44:5000 stopped answering: no reply to the last 12 \
     position requests (connection reset by peer, os error 54). The rig is held where it stopped; the other cameras carry on";
const LONG_PROBLEM: &str = "No timecode has arrived on input 3 of Focusrite Scarlett 18i20 USB for 2 seconds";
const LONG_CLIP: &str = "Opening number full band rehearsal take 14 stereo mix.wav";

fn crowded(window: egui::Vec2) -> Rig {
    let project = Project::default();
    let (song, clip) =
        ptz_core::edit::add_media_clip(&song(), &project, "bed", LONG_CLIP, ptz_core::song::Track::Audio, 1000, 0).unwrap();
    let mut r = rig_sized(window, project, presets(), song);
    let s = &mut r.harness.state_mut().session;
    s.status.telemetry.fault = Some(LONG_FAULT.into());
    s.status.transport.time = ptz_core::transport::TimeMode::LockedFile;
    s.status.transport.record = true;
    s.status.lock.source = Some(format!("file:{clip}"));
    s.status.lock.chase = ptz_core::chase::ChaseState::Locked;
    s.status.lock.problem = Some(LONG_PROBLEM.into());
    r.harness.run_steps(3);
    r
}

#[test]
fn e_stop_takes_a_click_anywhere_on_it_at_every_window_size() {
    for window in [SMALLEST_WINDOW, vec2(1280.0, 800.0), DEFAULT_WINDOW, vec2(2560.0, 1400.0)] {
        let mut r = rig_sized(window, Project::default(), presets(), song());
        assert_whole(&mut r, window, "E-STOP", Action::Estop);
    }
}

#[test]
fn e_stop_keeps_its_place_beside_the_longest_fault_and_lock_texts() {
    for window in [SMALLEST_WINDOW, vec2(1280.0, 800.0), DEFAULT_WINDOW] {
        let mut r = crowded(window);
        assert_whole(&mut r, window, "E-STOP", Action::Estop);

        // What the operator needs to read is still in the bar, whole and inside the window.
        let bar =
            egui::Rect::from_min_max(pos2(0.0, rect_of(&r, "E-STOP").top() - 10.0), pos2(window.x, rect_of(&r, "Cameras").top()));
        let shown = worded(&r);
        for needed in [LONG_FAULT, LONG_PROBLEM, "REC", &format!("LOCKED TO {LONG_CLIP}")] {
            let at = shown
                .iter()
                .filter(|(words, _)| words == needed)
                .map(|(_, rect)| *rect)
                .min_by(|a, b| a.top().total_cmp(&b.top()));
            let at = at.unwrap_or_else(|| panic!("{window:?}: the bar does not show {needed:?}"));
            assert!(bar.contains_rect(at), "{window:?}: {needed:?} is at {at:?}, the bar at {bar:?}");
        }
    }
}

#[test]
fn reset_e_stop_keeps_its_place_while_the_latch_and_a_fault_are_on_show() {
    for window in [SMALLEST_WINDOW, DEFAULT_WINDOW] {
        let mut r = crowded(window);
        {
            let s = &mut r.harness.state_mut().session;
            s.status.transport.estop = true;
            s.status.guidance = ptz_core::transport::guidance(&s.status.transport, &ptz_core::transport::Context::ready());
        }
        r.harness.run_steps(3);
        assert!(r.harness.query_by_label("E-STOP").is_none());
        r.harness.get_by_label("E-STOP LATCHED");
        assert_whole(&mut r, window, "Reset E-Stop", Action::ResetEstop);
    }
}

/// Where pictures of the bars go when `PTZ_APP_SHOTS` names a folder: for a person to look at.
fn shot(r: &Rig, name: &str) {
    if let Some(dir) = std::env::var_os("PTZ_APP_SHOTS") {
        let window = r.harness.ctx.content_rect();
        let dir = std::path::PathBuf::from(dir);
        r.picture().save(window, &dir.join(format!("{name}.png")));
        r.picture().save(window.with_max_y(190.0), &dir.join(format!("{name}-bars.png")));
    }
}

/// E-STOP as it is painted: the whole of its place is its own red, with nothing drawn across it.
fn assert_painted_whole(r: &Rig, state: &str) {
    let button = rect_of(r, "E-STOP");
    assert_eq!(button.size(), vec2(108.0, 34.0), "{state}: E-STOP is its full size");
    let picture = r.picture();
    let red = picture.share_of(button, crate::theme::RED);
    assert!(red > 0.8, "{state}: {:.0}% of E-STOP at {button:?} is its red", red * 100.0);
    for edge in [button.left_center() + vec2(4.0, 0.0), button.right_center() - vec2(4.0, 0.0)] {
        assert_eq!(picture.at(edge), crate::theme::RED, "{state}: E-STOP is red at {edge:?}");
    }
}

#[test]
fn e_stop_is_painted_whole_with_and_without_a_fault() {
    for (window, size) in [(SMALLEST_WINDOW, "1100x700"), (DEFAULT_WINDOW, "1500x920")] {
        let plain = rig_sized(window, Project::default(), presets(), song());
        assert_painted_whole(&plain, &format!("{size} plain"));
        shot(&plain, &format!("{size}-plain"));
        let crowded = crowded(window);
        assert_painted_whole(&crowded, &format!("{size} with a fault"));
        shot(&crowded, &format!("{size}-fault"));
    }
}
