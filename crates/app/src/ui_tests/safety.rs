//! E-STOP and the hand controls: what must work in every state of the window.
//! The pointer is moved and pressed as an operator's is, so a click lands on
//! whatever is really under it.

use super::*;
use crate::app::Prompt;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

fn song() -> Song {
    Song::new("s", "Test Song", "01:00:00:00", 1500)
}

pub(super) fn acts(r: &Rig) -> Vec<Action> {
    r.cmds.try_iter().filter_map(|c| if let Cmd::Act(a, _) = c { Some(a) } else { None }).collect()
}

pub(super) fn rect_of(r: &Rig, label: &str) -> egui::Rect {
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
pub(super) fn assert_whole(r: &mut Rig, window: egui::Vec2, label: &str, action: Action) {
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
pub(super) fn shot(r: &Rig, name: &str) {
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
        Prompt::StopNotSent => shell.session.stop_not_sent = Some(("Connection refused".into(), true)),
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
        Prompt::StopNotSent => crate::app::STOP_NOT_SENT,
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
        (Modifiers::CTRL, Key::S),
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

/// The transport buttons are off while the server cannot be reached; so are the keys that stand for them.
#[test]
fn transport_keys_do_nothing_while_the_server_is_unreachable() {
    use egui::Key;
    let mut r = busy(DEFAULT_WINDOW);
    r.harness.state_mut().session.lost = Some("Lost the connection to the server. Reconnecting".into());
    r.harness.run_steps(2);
    for label in ["Rehearse", "Cue"] {
        assert!(r.harness.get_by_label(label).accesskit_node().is_disabled(), "the {label} button is off");
    }
    for key in [Key::Space, Key::Home, Key::End, Key::ArrowLeft, Key::ArrowRight] {
        r.harness.key_press(key);
        r.harness.run_steps(2);
        assert_eq!(acts(&r), vec![], "{key:?} asks nothing of a server that cannot be reached");
    }
    let l = r.layout();
    r.click(pos2(l.x_of(480.0), l.ruler.center().y));
    assert_eq!(acts(&r), vec![], "a click on the ruler asks nothing of it either");

    // The same keys, once the server is back.
    r.harness.state_mut().session.lost = None;
    r.harness.run_steps(2);
    r.harness.key_press(Key::Space);
    r.harness.run_steps(2);
    assert_eq!(acts(&r).len(), 1, "Space runs the transport again");
}

/// E-Stop is the one thing sent to a server that cannot be reached. If it does not arrive the operator is
/// told so, over everything else, until the stop is seen to be latched.
#[test]
fn e_stop_is_sent_to_a_server_out_of_reach_and_the_operator_is_told_when_it_does_not_arrive() {
    use crate::app::{STOP_GAVE_UP, STOP_NOT_SENT, STOP_NOT_SENT_DO, STOP_STILL_TRYING};
    let mut r = rig_sized(SMALLEST_WINDOW, Project::default(), presets(), song());
    r.harness.state_mut().session.lost = Some("Lost the connection to the server. Reconnecting".into());
    // A dialog happens to be open as well.
    open(&mut r, Prompt::SyncPresets);
    let at = rect_of(&r, "E-STOP").center();
    r.click(at);
    assert_eq!(acts(&r), vec![Action::Estop], "the press is sent all the same");
    assert!(r.harness.query_by_label(STOP_NOT_SENT).is_none(), "nothing is said before the server has had its chance");

    r.events.send(Event::StopNotSent { why: "Connection refused".into(), trying: true }).unwrap();
    r.harness.run_steps(3);
    for words in [STOP_NOT_SENT, STOP_NOT_SENT_DO, "What went wrong: Connection refused", STOP_STILL_TRYING] {
        assert!(r.harness.query_by_label(words).is_some(), "the operator is told: {words}");
    }
    shot(&r, "1100x700-stop-not-sent");
    r.events.send(Event::StopNotSent { why: "Connection refused".into(), trying: false }).unwrap();
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label(STOP_GAVE_UP).is_some());
    assert!(r.harness.query_by_label(STOP_STILL_TRYING).is_none());

    // E-STOP is above this too, and pressing it sends the stop again.
    assert_whole(&mut r, SMALLEST_WINDOW, "E-STOP", Action::Estop);
    assert!(r.harness.query_by_label(STOP_NOT_SENT).is_some(), "it stays until it is closed or the stop is latched");

    // The stop got through after all: the server reports the latch.
    let mut status = r.harness.state().session.status.clone();
    status.transport.estop = true;
    r.events.send(Event::Status(Box::new(status))).unwrap();
    r.harness.run_steps(3);
    assert!(r.harness.query_by_label(STOP_NOT_SENT).is_none(), "the notice goes when the stop is latched");
    assert!(r.harness.query_by_label("E-STOP LATCHED").is_some());
}

fn cam_jogs(r: &Rig) -> Vec<(String, Jog)> {
    r.cmds.try_iter().filter_map(|c| if let Cmd::Jog(camera, jog) = c { Some((camera, jog)) } else { None }).collect()
}

/// Run a few frames, far enough apart for a held control to repeat its velocity.
fn hold_on(r: &mut Rig) {
    for _ in 0..3 {
        std::thread::sleep(crate::rig_ui::SEND_EVERY + std::time::Duration::from_millis(5));
        r.harness.run_steps(1);
    }
}

/// Three cameras, the Rig controller in front, and where to press its pad to pan right.
fn at_the_controller() -> (Rig, Pos2) {
    let (project, presets) = three_cameras();
    let mut r = rig_with(project, presets, song());
    r.harness.state_mut().show_tab(Tab::Rig);
    r.harness.run_steps(3);
    let pad = r.harness.state().session.rig.probe.expect("the controller was drawn").pad;
    (r, pos2(pad.right() - 8.0, pad.center().y))
}

fn press_and_hold(r: &mut Rig, at: Pos2) {
    r.harness.hover_at(at);
    r.harness.run_steps(2);
    r.harness.drag_at(at);
    r.harness.run_steps(3);
}

/// The camera that was moving was sent a stop, last of all, and nothing else was set moving.
fn assert_stopped(sent: &[(String, Jog)], camera: &str, when: &str) {
    let last = sent.iter().rev().find(|(c, _)| c == camera).map(|(_, jog)| *jog);
    assert_eq!(last, Some(Jog::default()), "{when}: the last thing {camera} was sent is a stop: {sent:?}");
    let others: Vec<_> = sent.iter().filter(|(c, jog)| c != camera && !jog.is_zero()).collect();
    assert!(others.is_empty(), "{when}: no other camera was set moving: {others:?}");
}

#[test]
fn a_held_pad_stops_the_camera_it_was_moving_when_another_camera_is_taken_in_hand() {
    let (mut r, right) = at_the_controller();
    press_and_hold(&mut r, right);
    let moving = cam_jogs(&r);
    assert!(moving.iter().all(|(camera, jog)| camera == "cam1" && jog.pan > 0.0) && !moving.is_empty(), "{moving:?}");

    // With the pad still held, 2 takes the second camera in hand.
    r.harness.key_press(egui::Key::Num2);
    r.harness.run_steps(2);
    assert_eq!(r.harness.state().session.cam_id(), "cam2");
    hold_on(&mut r);
    assert_stopped(&cam_jogs(&r), "cam1", "after the camera changed");

    // The second camera moves once the pad has been let go and pressed again.
    r.harness.drop_at(right);
    r.harness.run_steps(3);
    assert!(cam_jogs(&r).iter().all(|(_, jog)| jog.is_zero()));
    press_and_hold(&mut r, right);
    let moving = cam_jogs(&r);
    assert!(moving.iter().all(|(camera, jog)| camera == "cam2" && jog.pan > 0.0) && !moving.is_empty(), "{moving:?}");
    r.harness.drop_at(right);
    r.harness.run_steps(3);
    assert_stopped(&cam_jogs(&r), "cam2", "after letting go");
}

#[test]
fn a_held_jog_key_stops_when_the_controller_leaves_the_screen() {
    let (mut r, right) = at_the_controller();
    r.click(right);
    let _ = cam_jogs(&r);
    r.harness.key_down(egui::Key::ArrowLeft);
    r.harness.run_steps(3);
    assert!(cam_jogs(&r).iter().any(|(camera, jog)| camera == "cam1" && jog.pan < 0.0), "the arrow drives the rig");

    // The Inspector shares the controller's group: bringing it forward covers the controller.
    r.harness.state_mut().show_tab(Tab::Inspector);
    r.harness.run_steps(2);
    hold_on(&mut r);
    assert_stopped(&cam_jogs(&r), "cam1", "with the controller covered");

    // Coming back with the key still down does not set it off again.
    r.harness.state_mut().show_tab(Tab::Rig);
    r.harness.run_steps(2);
    hold_on(&mut r);
    assert!(cam_jogs(&r).iter().all(|(_, jog)| jog.is_zero()));
}

#[test]
fn a_held_pad_stops_when_the_hand_is_taken_off_it_by_anything_else() {
    type Change = fn(&mut Rig, bool);
    let cases: [(&str, Change); 5] = [
        ("a dialog opens", |r, on| r.harness.state_mut().session.confirm_takeover = on),
        ("the window loses focus", |r, on| r.harness.input_mut().focused = !on),
        ("the lease is lost", |r, on| r.harness.state_mut().session.status.you_hold_lease = !on),
        ("the server is lost", |r, on| r.harness.state_mut().session.lost = on.then(|| "Lost the server".to_string())),
        ("E-Stop is latched", |r, on| r.harness.state_mut().session.status.transport.estop = on),
    ];
    for (when, change) in cases {
        let (mut r, right) = at_the_controller();
        press_and_hold(&mut r, right);
        assert!(cam_jogs(&r).iter().any(|(_, jog)| jog.pan > 0.0), "{when}: the pad drives the rig first");

        change(&mut r, true);
        r.harness.run_steps(2);
        hold_on(&mut r);
        assert_stopped(&cam_jogs(&r), "cam1", when);

        // It passes with the pad still held: the rig does not start again by itself.
        change(&mut r, false);
        r.harness.run_steps(3);
        hold_on(&mut r);
        let after = cam_jogs(&r);
        assert!(after.iter().all(|(_, jog)| jog.is_zero()), "{when}, then not: the rig set off again: {after:?}");

        // Let go and press again, and it moves.
        r.harness.drop_at(right);
        r.harness.run_steps(3);
        press_and_hold(&mut r, right);
        assert!(cam_jogs(&r).iter().any(|(_, jog)| jog.pan > 0.0), "{when}: a new press moves the rig");
    }
}

/// Undo belongs to the text box while one is being typed in: it does not reach past it into the song.
#[test]
fn undo_and_redo_leave_the_song_alone_while_a_text_field_has_the_keyboard() {
    use egui::{Key, Modifiers};
    let mut r = busy(DEFAULT_WINDOW);
    assert_eq!(r.song().motion.len(), 1);
    // The song's name, in the header.
    let name = r
        .harness
        .query_all(egui_kittest::kittest::by().role(egui::accesskit::Role::TextInput))
        .filter(|n| n.accesskit_node().value().as_deref() == Some("Test Song"))
        .map(|n| n.rect())
        .min_by(|a, b| a.top().total_cmp(&b.top()))
        .expect("the name box is in the header");
    r.click(name.center());
    assert!(r.harness.ctx.text_edit_focused(), "the name box has the keyboard");
    r.harness.event(egui::Event::Text("x".into()));
    r.harness.run_steps(2);
    let _ = r.cmds.try_iter().count();

    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    r.harness.run_steps(2);
    assert_eq!(r.song().motion.len(), 1, "Cmd-Z in the name box did not undo the move on the timeline");
    assert!(r.toasts().is_empty(), "{:?}", r.toasts());

    // Out of the box, the same keys undo and redo the song.
    r.harness.key_press(Key::Escape);
    r.harness.run_steps(2);
    let l = r.layout();
    r.click(pos2(l.x_of(900.0), l.lane_rect(Lane::Audio).unwrap().center().y));
    assert!(!r.harness.ctx.text_edit_focused());
    let before = r.song().clone();
    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    r.harness.run_steps(2);
    assert_ne!(*r.song(), before, "Cmd-Z undoes when no text field has the keyboard");
    r.harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
    r.harness.run_steps(2);
    assert_eq!(*r.song(), before, "and Cmd-Shift-Z puts it back");
}

/// Space is the transport's key and nothing else's, and Arm, Disarm, E-Stop and Reset E-Stop have no key
/// at all (programming manual, section 14). So the buttons of the transport bar cannot be reached with
/// Tab, and Space does not press a button that Tab has left the keyboard on.
#[test]
fn space_runs_the_transport_once_and_presses_no_button() {
    // After a click on a button.
    let mut r = rig(song());
    let cue = rect_of(&r, "Cue").center();
    r.click(cue);
    assert_eq!(acts(&r), vec![Action::Cue]);
    r.harness.key_press(egui::Key::Space);
    r.harness.run_steps(3);
    assert_eq!(acts(&r), vec![Action::Play], "after a click on Cue");

    // Tab goes round the window and never lands on the transport bar.
    let mut r = rig(song());
    let mut landed_on_new = false;
    for _ in 0..150 {
        r.harness.key_press(egui::Key::Tab);
        r.harness.run_steps(2);
        for label in [crate::app::ARM_LABEL, "Rehearse", "Stop", "Cue", "Record"] {
            assert!(!r.harness.get_by_label(label).is_focused(), "Tab put the keyboard on {label}");
        }
        landed_on_new |= r.harness.get_by_label("New").is_focused();
    }
    assert!(landed_on_new, "Tab does reach the ordinary buttons");
    {
        let s = &mut r.harness.state_mut().session;
        s.status.transport.estop = true;
        s.status.transport.output = Output::Armed;
    }
    for _ in 0..150 {
        r.harness.key_press(egui::Key::Tab);
        r.harness.run_steps(2);
        for label in ["Reset E-Stop", "Disarm"] {
            assert!(!r.harness.get_by_label(label).is_focused(), "Tab put the keyboard on {label}");
        }
    }
    {
        let s = &mut r.harness.state_mut().session;
        s.status.transport.estop = false;
        s.status.transport.output = Output::Rehearse;
    }
    r.harness.run_steps(2);
    assert_eq!(acts(&r), vec![]);
    for key in [egui::Key::Space, egui::Key::Enter] {
        r.harness.key_press(key);
        r.harness.run_steps(3);
    }
    assert_eq!(acts(&r), vec![Action::Play], "Space ran the transport and Enter did nothing to it");

    // With the keyboard on New, Space still means the transport: no dialog opens behind the operator's back.
    while !r.harness.get_by_label("New").is_focused() {
        r.harness.key_press(egui::Key::Tab);
        r.harness.run_steps(2);
    }
    r.harness.key_press(egui::Key::Space);
    r.harness.run_steps(3);
    assert_eq!(acts(&r), vec![Action::Play]);
    assert!(!crate::app::modal_open(&r.harness.ctx), "Space did not press New");

    // E-STOP is the one button of the bar that Tab reaches. Space there stops, and does not play.
    let mut tabs = 0;
    while !r.harness.get_by_label("E-STOP").is_focused() {
        assert!(tabs < 150, "Tab never reached E-STOP");
        r.harness.key_press(egui::Key::Tab);
        r.harness.run_steps(2);
        tabs += 1;
    }
    r.harness.key_press(egui::Key::Space);
    r.harness.run_steps(3);
    assert_eq!(acts(&r), vec![Action::Estop], "with the keyboard on E-STOP");
}
