//! Show settings: the one place for what belongs to the whole show (cameras, rigs, lenses, the stage,
//! audio and timecode). It takes the place of the panels; the header, the transport and the camera
//! bar stay on screen.

use super::safety::{acts, assert_whole, rect_of, shot};
use super::*;
use egui_kittest::kittest::Queryable as _;

fn song() -> Song {
    Song::new("s", "Test Song", "01:00:00:00", 1500)
}

fn open_page(r: &Rig) -> Option<Page> {
    r.harness.state().session.settings
}

/// Every page, with something only that page shows.
const PAGES: [(Page, &str); 12] = [
    (Page::Cameras, "Add a camera"),
    (Page::General, "When a camera faults"),
    (Page::Stage, "Import a model"),
    (Page::Programming, "Use a preset within range"),
    (Page::Rig, "Arm moves at"),
    (Page::Lens, "Focal length"),
    (Page::Position, "Floor position"),
    (Page::Limits, "Top speed /s"),
    (Page::KeepOut, "Add a keep-out rule"),
    (Page::Audio, "Right"),
    (Page::Timecode, "List devices again"),
    (Page::Panel, "Use this panel"),
];

impl Rig {
    /// Where a page's name is in the list at the left of Show settings.
    fn page_in_list(&self, page: Page) -> egui::Rect {
        let list = rect_of(self, "THE SHOW").left();
        self.harness
            .query_all_by_label(page.title())
            .map(|n| n.rect())
            .find(|rect| (rect.left() - list).abs() < 4.0)
            .unwrap_or_else(|| panic!("{} is in the list of pages", page.title()))
    }

    /// Click a page's name in the list.
    fn choose_page(&mut self, page: Page) {
        let at = self.page_in_list(page);
        self.click(at.center());
    }
}

#[test]
fn show_settings_opens_from_the_camera_bar_and_the_inspector_and_its_button_or_esc_closes_it() {
    let mut r = rig(song());
    assert_eq!(open_page(&r), None);

    // The Inspector no longer holds the settings: with nothing selected it shows the song and the way there.
    r.harness.state_mut().show_tab(Tab::Inspector);
    r.harness.run_steps(3);
    for gone in ["Add a camera", "Frame rate", "Import a model", "Add a keep-out rule", "List devices again"] {
        assert!(r.harness.query_by_label(gone).is_none(), "{gone} is still in the Inspector");
    }
    r.press("Open Show settings");
    assert_eq!(open_page(&r), Some(Page::Cameras), "it opens on the camera list");
    r.harness.get_by_label("Add a camera");
    assert!(r.harness.query_by_label("Open Show settings").is_none(), "the panels are out of the way");
    for kept in ["E-STOP", "Cue", "New", "1 Camera 1"] {
        r.harness.get_by_label(kept);
    }

    // The button at the end of the camera bar closes it and, while it is closed, opens it.
    assert!(r.harness.query_by_label("Show settings").is_some_and(|n| n.rect().left() < 200.0), "only its heading has the words");
    r.press("Close settings");
    assert_eq!(open_page(&r), None);
    r.harness.get_by_label("Open Show settings");
    r.press("Show settings");
    assert_eq!(open_page(&r), Some(Page::Cameras));
    r.press("Close settings");
    assert_eq!(open_page(&r), None);

    // Esc closes it, but first closes a list that is open over it.
    r.settings(Page::Lens);
    let lens = r.harness.state().session.project.cameras[0].lens.name.clone();
    r.harness.get_by_value(&lens).click();
    r.harness.run_steps(3);
    r.harness.key_press(egui::Key::Escape);
    r.harness.run_steps(3);
    assert_eq!(open_page(&r), Some(Page::Lens), "the first Esc closed the list of lenses");
    r.harness.key_press(egui::Key::Escape);
    r.harness.run_steps(3);
    assert_eq!(open_page(&r), None);
    assert!(r.projects().is_empty(), "opening and closing changed nothing");
}

#[test]
fn every_page_is_reached_from_the_list_and_fits_the_smallest_window() {
    for window in [SMALLEST_WINDOW, DEFAULT_WINDOW] {
        let (project, presets) = three_cameras();
        let mut r = rig_sized(window, project, presets, song());
        r.press("Show settings");
        let inside = egui::Rect::from_min_size(Pos2::ZERO, window);
        // The whole list is on screen: no page is reached only by scrolling.
        let list = inside.with_max_y(inside.bottom() - 8.0);
        for (page, _) in PAGES {
            let at = r.page_in_list(page);
            assert!(list.contains_rect(at), "{window:?}: {page:?} is in the list at {at:?}");
        }
        for (page, shows) in PAGES {
            r.choose_page(page);
            assert_eq!(open_page(&r), Some(page));
            let at = rect_of(&r, shows);
            assert!(inside.contains_rect(at), "{window:?}: {shows:?} on {page:?} is at {at:?}");
            // Only this page is on show.
            for (other, theirs) in PAGES {
                assert!(other == page || r.harness.query_by_label(theirs).is_none(), "{theirs:?} is shown on {page:?}");
            }
            shot(&r, &format!("settings-{}-{:?}", window.x, page).to_lowercase());
        }
        // The pages of one camera say which: the camera bar chooses it without leaving the page.
        r.choose_page(Page::Rig);
        assert_eq!(r.harness.query_all_by_label("Stage left").count(), 2, "in the list and over the page");
        r.harness.get_by_label("These are this camera's. Choose another on the camera bar, or with its number key.");
        r.press("3 Balcony");
        assert_eq!(open_page(&r), Some(Page::Rig));
        assert_eq!(r.harness.query_all_by_label("Balcony").count(), 2);
        assert!(r.projects().is_empty(), "looking through the pages changed nothing");
    }
}

#[test]
fn e_stop_and_the_transport_work_while_show_settings_is_open() {
    for window in [SMALLEST_WINDOW, DEFAULT_WINDOW] {
        let mut r = rig_sized(window, Project::default(), presets(), song());
        for (page, _) in PAGES {
            r.settings(page);
            assert_whole(&mut r, window, "E-STOP", Action::Estop);
            let (stop, close) = (rect_of(&r, "E-STOP"), rect_of(&r, "Close settings"));
            assert!(!stop.intersects(close), "{window:?}: Close settings at {close:?} is across E-STOP at {stop:?}");
        }
        r.click(rect_of(&r, "Cue").center());
        assert_eq!(acts(&r), vec![Action::Cue]);
        r.harness.key_press(egui::Key::Space);
        r.harness.run_steps(3);
        assert_eq!(acts(&r), vec![Action::Play], "Space still runs the transport, which is still on screen");
    }
}

#[test]
fn the_keys_that_edit_the_song_do_nothing_while_show_settings_is_open() {
    use egui::{Key, Modifiers};
    let (project, all) = three_cameras();
    let empty = song();
    let (with_move, id) = ptz_core::edit::insert_preset_move(&empty, &project, &all[1], 100).unwrap();
    let mut r = rig_with(project, all, empty);
    {
        let s = &mut r.harness.state_mut().session;
        s.edit(|_, _| Ok(with_move.clone()));
        s.timeline.selection.insert(id.clone());
    }
    r.harness.run_steps(3);
    let before = r.song().clone();
    assert_eq!(before.motion.len(), 1);
    let playhead = r.harness.state().session.playhead();

    r.settings(Page::Cameras);
    let _ = r.cmds.try_iter().count();
    for (modifiers, key) in [
        (Modifiers::COMMAND, Key::Z),
        (Modifiers::COMMAND | Modifiers::SHIFT, Key::Z),
        (Modifiers::COMMAND, Key::D),
        (Modifiers::NONE, Key::Delete),
        (Modifiers::NONE, Key::Backspace),
        (Modifiers::NONE, Key::ArrowRight),
        (Modifiers::NONE, Key::End),
        (Modifiers::NONE, Key::Home),
    ] {
        r.harness.key_press_modifiers(modifiers, key);
        r.harness.run_steps(2);
        assert_eq!(*r.song(), before, "{key:?} changed the song from behind Show settings");
        assert_eq!(r.harness.state().session.playhead(), playhead, "{key:?} moved the playhead");
    }
    assert_eq!(r.cmds.try_iter().count(), 0, "nothing was sent");
    assert!(r.toasts().is_empty(), "{:?}", r.toasts());

    // The number keys still choose a camera: the camera bar is on screen.
    r.harness.key_press(Key::Num2);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.cam_id(), "cam2");
    assert_eq!(open_page(&r), Some(Page::Cameras));

    // Back at the timeline the same keys work again.
    r.press("Close settings");
    r.harness.key_press_modifiers(Modifiers::COMMAND, Key::Z);
    r.harness.run_steps(2);
    assert!(r.song().motion.is_empty(), "Cmd-Z undoes once the song is back on screen");
}

#[test]
fn without_the_edit_lease_show_settings_says_so_and_changes_nothing() {
    let mut r = rig(song());
    r.settings(Page::Cameras);
    assert!(r
        .harness
        .query_by_label("Take the edit lease to change settings. Until then the settings can only be read.")
        .is_none());

    r.harness.state_mut().session.status.you_hold_lease = false;
    r.harness.run_steps(3);
    r.harness.get_by_label("Take the edit lease to change settings. Until then the settings can only be read.");
    let _ = r.cmds.try_iter().count();
    r.press("Add a camera");
    r.choose_page(Page::KeepOut);
    r.press("Add a keep-out rule");
    assert!(r.projects().is_empty(), "nothing is changed without the lease");
    assert_eq!(r.harness.state().session.project.cameras.len(), 1);

    // Armed, with the lease: the same, with the reason for it.
    {
        let s = &mut r.harness.state_mut().session;
        s.status.you_hold_lease = true;
        s.status.transport.output = Output::Armed;
    }
    r.harness.run_steps(3);
    r.harness.get_by_label("Disarm to change settings. Until then the settings can only be read.");
    r.press("Add a keep-out rule");
    assert!(r.projects().is_empty());
}

#[test]
fn the_frame_rate_and_the_fault_rule_are_set_on_their_own_page() {
    let mut r = rig(song());
    r.settings(Page::General);
    let _ = r.cmds.try_iter().count();
    let rate = r.harness.state().session.project.rate;
    assert_eq!(rate, ptz_core::FrameRate::F25);

    r.harness.get_by_value(rate.label()).click();
    r.harness.run_steps(3);
    r.press("30");
    let sent = r.projects().pop().expect("the project was sent");
    assert_eq!(sent.rate, ptz_core::FrameRate::F30);
    // The server can refuse a change of rate, so it is shown once the server has taken it.
    assert_eq!(r.harness.state().session.project.rate, rate);

    let rule = r.harness.state().session.project.on_fault;
    let other = ptz_core::camera::OnFault::ALL.into_iter().find(|o| *o != rule).unwrap();
    r.harness.get_by_value(rule.label()).click();
    r.harness.run_steps(3);
    r.press(other.label());
    let sent = r.projects().pop().expect("the project was sent");
    assert_eq!(sent.on_fault, other);
    assert_eq!(sent.rate, rate);
    sent.validate().unwrap();
}

#[test]
fn the_control_panel_page_says_what_the_server_says_about_the_panel() {
    use crate::settings_ui::panel_state;
    use ptz_core::api::{PanelHost, PanelState};
    let mut st = rig(song()).harness.state().session.status.clone();
    assert_eq!(panel_state(&st, None), ("No panel is in use.".to_string(), true));
    assert_eq!(panel_state(&st, Some("10.0.0.5")).0, "Starting.");
    st.panel_host = Some(PanelHost { address: "10.0.0.5:9923".into(), ..Default::default() });
    let (text, good) = panel_state(&st, Some("10.0.0.5"));
    assert!(text.starts_with("Looking for the panel at 10.0.0.5:9923.") && !good, "{text}");
    st.panel = Some(PanelState { station: "Panel".into(), connected: true, model: "PTZ Pro".into(), ..Default::default() });
    let (text, good) = panel_state(&st, Some("10.0.0.5"));
    assert!(text.starts_with("Connected to PTZ Pro.") && good, "{text}");
    st.panel_host.as_mut().unwrap().problem = Some("panel.json: bad".into());
    let (text, good) = panel_state(&st, Some("10.0.0.5"));
    assert!(text.contains("panel.json: bad") && !good, "{text}");

    // On the page: the box cannot be ticked before there is an address, and a panel from --panel is not changed here.
    let mut r = rig(song());
    r.settings(Page::Panel);
    let _ = r.cmds.try_iter().count();
    r.harness.get_by_label("No panel is in use.");
    r.press("Use this panel");
    assert!(r.projects().is_empty(), "no panel is switched on without an address");
    r.harness.state_mut().session.project.panel.address = "10.0.0.5".into();
    r.harness.run_steps(3);
    r.press("Use this panel");
    let sent = r.projects().pop().expect("the project was sent");
    assert!(sent.panel.enabled && sent.panel.address == "10.0.0.5");
    sent.validate().unwrap();

    r.harness.state_mut().session.status.panel_host =
        Some(PanelHost { address: "10.0.0.9:9923".into(), from_command_line: true, problem: None });
    r.harness.run_steps(3);
    r.press("Use this panel");
    assert!(r.projects().is_empty(), "a panel given where the server was started is not changed here");
}

#[test]
fn the_programming_page_sets_how_near_a_pose_must_be_to_a_preset_to_use_it_again() {
    let mut r = rig(song());
    r.settings(Page::Programming);
    let _ = r.cmds.try_iter().count();
    for axis in ["Pan", "Tilt", "Zoom", "Focus", "Elevation"] {
        r.harness.get_by_label(axis);
    }

    r.press("Use a preset within range");
    let sent = r.projects().pop().expect("the project was sent");
    assert!(!sent.preset_match.enabled, "the box switches it off");
    assert_eq!(sent.preset_match.range, r.harness.state().session.project.preset_match.range, "the ranges are kept");
    sent.validate().unwrap();

    // Nothing is changed by a station that may not edit.
    let mut r = rig(song());
    r.harness.state_mut().session.status.you_hold_lease = false;
    r.settings(Page::Programming);
    let _ = r.cmds.try_iter().count();
    r.press("Use a preset within range");
    assert!(r.projects().is_empty(), "a station without the lease changes nothing");
}
