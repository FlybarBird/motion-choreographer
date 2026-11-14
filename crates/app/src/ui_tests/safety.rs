//! E-STOP and the hand controls: what must work in every state of the window.
//! The pointer is moved and pressed as an operator's is, so a click lands on
//! whatever is really under it.

use super::*;
use crate::app::Prompt;
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

/// Put a dialog on the screen. Every dialog there is has an arm here: a new one does not compile without.
fn open(r: &mut Rig, prompt: Prompt) {
    use crate::app::Dialog;
    let ctx = r.harness.ctx.clone();
    let shell = r.harness.state_mut();
    match prompt {
        Prompt::NewSong => shell.dialog = Dialog::New { name: "Encore".into(), start_tc: "02:00:00:00".into() },
        Prompt::LoadSong => shell.dialog = Dialog::Load,
        Prompt::SaveAs => shell.dialog = Dialog::SaveAs { name: "Test Song copy".into() },
        Prompt::Revert => shell.dialog = Dialog::Revert,
        Prompt::DeleteSong => shell.dialog = Dialog::DeleteSong { id: "old".into(), name: "Old song".into() },
        Prompt::SyncPresets => shell.session.confirm_sync = true,
        Prompt::TakeOver => shell.session.confirm_takeover = true,
        Prompt::RenameMarker => {
            ctx.data_mut(|d| d.insert_temp(crate::app::marker_rename_id(), ("m1".to_string(), "Chorus".to_string())));
        }
        Prompt::StampedDrop => {
            shell.session.stamped_drop = Some(crate::session::StampedDrop {
                media: stamped_media("late", ptz_core::api::MediaKind::Audio, "05:00:00:00"),
                at_frame: 100,
            })
        }
    }
    r.harness.run_steps(3);
    assert!(crate::app::modal_open(&r.harness.ctx), "{prompt:?} is open");
}

/// Something of each dialog that is on the screen only while it is open.
fn words_of(prompt: Prompt) -> &'static str {
    match prompt {
        Prompt::NewSong => "New song",
        Prompt::LoadSong => "Load a song",
        Prompt::SaveAs => "Makes a copy with its own revisions and opens it.",
        Prompt::Revert => "Revert to the last Save?",
        Prompt::DeleteSong => "Delete Old song?",
        Prompt::SyncPresets => "Read presets from the camera?",
        Prompt::TakeOver => "Take the edit lease from Dana?",
        Prompt::RenameMarker => "Rename marker",
        Prompt::StampedDrop => "late.wav starts at 05:00:00:00",
    }
}

/// A window with another station on the lease, so that there is somebody to take it over from.
fn watched(window: egui::Vec2) -> Rig {
    let mut r = rig_sized(window, Project::default(), presets(), song());
    let status = &mut r.harness.state_mut().session.status;
    status.lease = Some(ptz_core::api::LeaseStatus { holder: "Dana".into(), client_id: "dana".into(), expires_in_ms: 5000 });
    status.you_hold_lease = false;
    r.harness.run_steps(2);
    r
}

#[test]
fn e_stop_takes_a_click_under_every_dialog() {
    for window in [SMALLEST_WINDOW, DEFAULT_WINDOW] {
        for prompt in Prompt::ALL {
            let mut r = watched(window);
            open(&mut r, prompt);
            assert!(r.harness.query_by_label(words_of(prompt)).is_some(), "{prompt:?} is on the screen");
            assert_painted_whole(&r, &format!("{window:?} under {prompt:?}"));
            shot(&r, &format!("{}x{}-under-{prompt:?}", window.x, window.y));
            assert_whole(&mut r, window, "E-STOP", Action::Estop);
            assert!(r.harness.query_by_label(words_of(prompt)).is_some(), "{prompt:?} stays open through an E-Stop");
        }
    }
}

/// Dialogs are put up in one way, which is what the walk through [`Prompt::ALL`] relies on.
#[test]
fn every_dialog_is_one_of_the_prompts() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut made = vec![];
    for entry in std::fs::read_dir(&src).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "rs") {
            let text = std::fs::read_to_string(&path).unwrap();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            made.extend(text.lines().filter(|l| l.contains("Modal::new(")).map(|l| format!("{name}: {}", l.trim())));
        }
    }
    assert_eq!(made.len(), 1, "only Prompt::show makes a modal: {made:#?}");
    assert!(made[0].starts_with("app.rs:"), "{made:#?}");
}

/// E-STOP is drawn over anything that opens across it, so the one menu beside it opens to its side.
#[test]
fn the_view_menu_opens_clear_of_e_stop() {
    for window in [SMALLEST_WINDOW, DEFAULT_WINDOW] {
        let mut r = rig_sized(window, Project::default(), presets(), song());
        let view = rect_of(&r, "View").center();
        r.click(view);
        let stop = rect_of(&r, "E-STOP");
        for item in ["Reset layout", "Fit timeline"] {
            let at = rect_of(&r, item);
            assert!(!at.intersects(stop.expand(2.0)), "{window:?}: {item} at {at:?} is clear of E-STOP at {stop:?}");
        }
        shot(&r, &format!("{}x{}-view-menu", window.x, window.y));
        let item = rect_of(&r, "Fit timeline").center();
        r.click(item);
        assert!(r.harness.query_by_label("Fit timeline").is_none(), "{window:?}: the menu closes when an item is chosen");
        assert_whole(&mut r, window, "E-STOP", Action::Estop);
    }
}

/// Three cameras and a song with a selected move on it, made by an edit that can be undone.
fn busy(window: egui::Vec2) -> Rig {
    let (project, presets) = three_cameras();
    let mut r = rig_sized(window, project, presets.clone(), song());
    {
        let s = &mut r.harness.state_mut().session;
        let mut id = String::new();
        assert!(s.edit(|song, project| {
            let (song, clip) = ptz_core::edit::insert_preset_move(song, project, &presets[1], 200)?;
            id = clip;
            Ok(song)
        }));
        s.timeline.selection.insert(id);
    }
    r.harness.run_steps(3);
    let _ = r.cmds.try_iter().count();
    r
}

/// What the keyboard can change: the song, the camera in hand, the selection, and what is sent to the server.
fn show_state(r: &Rig) -> (Song, String, Vec<String>, Vec<String>) {
    let s = &r.harness.state().session;
    (
        r.song().clone(),
        s.cam_id().to_string(),
        s.timeline.selection.iter().cloned().collect(),
        r.cmds.try_iter().map(|c| format!("{c:?}")).collect(),
    )
}

/// Every key the window gives a meaning to, pressed one after another.
fn press_every_shortcut(r: &mut Rig) {
    use egui::{Key, Modifiers};
    let plain = [
        Key::Space,
        Key::Home,
        Key::End,
        Key::ArrowLeft,
        Key::ArrowRight,
        Key::Num2,
        Key::M,
        Key::K,
        Key::Backslash,
        Key::Delete,
        Key::Backspace,
    ];
    for key in plain {
        r.harness.key_press(key);
        r.harness.run_steps(2);
    }
    for (modifiers, key) in [
        (Modifiers::COMMAND, Key::D),
        (Modifiers::COMMAND, Key::S),
        (Modifiers::COMMAND, Key::Z),
        (Modifiers::COMMAND | Modifiers::SHIFT, Key::Z),
    ] {
        r.harness.key_press_modifiers(modifiers, key);
        r.harness.run_steps(2);
    }
}

#[test]
fn no_key_reaches_the_show_while_a_dialog_is_open() {
    // With nothing open the same keys do run the transport, change camera and edit the song.
    let mut r = busy(DEFAULT_WINDOW);
    let before = show_state(&r);
    press_every_shortcut(&mut r);
    let after = show_state(&r);
    assert_ne!(before.0, after.0, "the keys edit the song");
    assert_ne!(before.1, after.1, "the keys change camera");
    assert!(after.3.iter().any(|c| c.starts_with("Act(")), "the keys run the transport: {:?}", after.3);

    for prompt in Prompt::ALL {
        let mut r = busy(DEFAULT_WINDOW);
        open(&mut r, prompt);
        let before = show_state(&r);
        press_every_shortcut(&mut r);
        assert!(crate::app::modal_open(&r.harness.ctx), "{prompt:?} is still open");
        assert_eq!(before, show_state(&r), "under {prompt:?}");
    }
}
