//! Place cameras while watching their simulated view, without jogging the rig or changing a shot.

use super::*;
use egui_kittest::kittest::{NodeT as _, Queryable as _};

fn song() -> Song {
    Song::new("s", "Placement", "01:00:00:00", 1500)
}

fn open(window: egui::Vec2) -> Rig {
    let (mut project, presets) = three_cameras();
    project.cameras[1].mount.z = 4.0;
    let mut r = rig_sized(window, project, presets, song());
    r.harness.state_mut().show_tab(Tab::Stage);
    r.harness.run_steps(3);
    r.press("Camera placement");
    assert!(r.harness.state().session.stage_view.placement);
    r.press("2 Centre");
    let _ = r.cmds.try_iter().count();
    r
}

fn preview(r: &Rig) -> egui::Rect {
    r.harness.state().session.stage_view.placement_preview.expect("the placement preview was drawn").rect
}

fn pixels(r: &Rig) -> Vec<u8> {
    r.harness.state().session.world.frame("placement").expect("the camera view was rendered").rgba.clone()
}

/// Placement may update a project, but must never send a jog, transport or song edit command.
fn only_projects(r: &Rig) -> Vec<Project> {
    r.cmds
        .try_iter()
        .map(|command| match command {
            Cmd::PutProject(project) => *project,
            other => panic!("placement sent {other:?}"),
        })
        .collect()
}

fn type_number(r: &mut Rig, label: &str, value: &str) {
    let role = egui::accesskit::Role::SpinButton;
    r.harness.get_by_role_and_label(role, label).focus();
    r.harness.run_steps(2);
    r.harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    r.harness.run_steps(1);
    r.harness.get_by_role_and_label(role, label).type_text(value);
    r.harness.run_steps(2);
    r.harness.key_press(egui::Key::Enter);
    r.harness.run_steps(3);
}

#[test]
fn placement_numbers_use_the_show_units_and_refresh_the_camera_view() {
    for units in [ptz_core::Units::Metric, ptz_core::Units::Imperial] {
        let mut r = open(DEFAULT_WINDOW);
        r.harness.state_mut().session.project.units = units;
        r.harness.run_steps(3);
        let image = pixels(&r);
        let poses = r.harness.state().session.poses();
        type_number(&mut r, "Placement X", "10");
        type_number(&mut r, "Mount height", "15");
        type_number(&mut r, "Lens offset", "20");
        let m = r.mount("cam2");
        assert!((m.x - 10.0 * units.m_per_long()).abs() < 1e-9, "{units:?}: {m:?}");
        assert!((m.z - 15.0 * units.m_per_long()).abs() < 1e-9, "{units:?}: {m:?}");
        assert!((m.base_height_mm - 20.0 * units.mm_per_short()).abs() < 1e-9, "{units:?}: {m:?}");
        assert_eq!(r.harness.state().session.poses(), poses);
        let sent = only_projects(&r);
        assert_eq!(sent.last().unwrap().cameras[1].mount, m);
        assert_ne!(pixels(&r), image, "position updates the simulated view");
    }
}

#[test]
fn placement_previews_the_selected_camera_and_inverts_only_its_mount() {
    let mut r = open(DEFAULT_WINDOW);
    let before = r.harness.state().session.project.clone();
    let poses = r.harness.state().session.poses();
    let image = pixels(&r);
    let stage = r.harness.state().session.world.frame("stage").unwrap().rgba.clone();

    r.press("Inverted mount");
    let after = r.harness.state().session.project.clone();
    assert!(after.cameras[1].mount.inverted);
    assert_eq!(after.cameras[0], before.cameras[0]);
    assert_eq!(after.cameras[2], before.cameras[2]);
    assert_eq!(after.cameras[1].rig, before.cameras[1].rig, "mounting must not invert hardware calibration a second time");
    assert_eq!(r.harness.state().session.poses(), poses, "mounting changes neither a shot nor the rig pose");
    assert_eq!(only_projects(&r), vec![after.clone()]);
    assert_ne!(pixels(&r), image, "the preview follows the changed lens height");
    assert_ne!(r.harness.state().session.world.frame("stage").unwrap().rgba, stage, "the hanging marker redraws");
    let lens = after.cameras[1].mount.lens_position(poses[1].elevation);
    assert!(lens[2] < after.cameras[1].mount.z, "the lens hangs below its attachment");

    r.press("Undo move");
    assert_eq!(r.harness.state().session.project, before);
    assert_eq!(only_projects(&r), vec![before]);

    r.press("3 Balcony");
    assert_eq!(r.harness.state().session.cam_id(), "cam3");
    assert_ne!(pixels(&r), image, "choosing another camera changes the preview");
    assert!(only_projects(&r).is_empty(), "selection does not edit the project");
}

#[test]
fn dragging_the_camera_preview_changes_mount_angles_once_and_undo_restores_them() {
    let mut r = open(DEFAULT_WINDOW);
    let before = r.harness.state().session.project.clone();
    let poses = r.harness.state().session.poses();
    let image = pixels(&r);
    let from = preview(&r).center();
    r.drag(from, from + vec2(45.0, 24.0));

    let after = r.harness.state().session.project.clone();
    let (a, b) = (before.cameras[1].mount, after.cameras[1].mount);
    assert!((b.heading_deg - a.heading_deg).abs() > 1.0, "horizontal drag changes the mounting heading: {b:?}");
    assert!((b.tilt_deg - a.tilt_deg).abs() > 1.0, "vertical drag changes the mounting tilt: {b:?}");
    assert_eq!((a.x, a.y, a.z, a.base_height_mm), (b.x, b.y, b.z, b.base_height_mm));
    assert_eq!(r.harness.state().session.poses(), poses);
    assert_eq!(r.song(), &song());
    assert_eq!(only_projects(&r), vec![after], "one completed drag is one project update");
    assert_ne!(pixels(&r), image, "the camera view follows the new angles");
    assert_eq!(r.harness.state().session.stage_view.history.len(), 1, "one drag is one undo step");

    r.press("Undo move");
    assert_eq!(r.harness.state().session.project, before);
    assert_eq!(only_projects(&r), vec![before]);
}

#[test]
fn escape_cancels_a_preview_drag_without_sending_a_project() {
    let mut r = open(DEFAULT_WINDOW);
    let before = r.harness.state().session.project.clone();
    let from = preview(&r).center();
    r.harness.hover_at(from);
    r.harness.run_steps(2);
    r.harness.drag_at(from);
    r.harness.run_steps(2);
    let to = from + vec2(35.0, 20.0);
    r.harness.hover_at(to);
    r.harness.run_steps(3);
    assert_ne!(r.harness.state().session.project, before, "the drag is previewed before release");
    assert!(only_projects(&r).is_empty(), "the preview is local until release");

    r.harness.key_press(egui::Key::Escape);
    r.harness.run_steps(2);
    r.harness.drop_at(to);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.project, before);
    assert!(only_projects(&r).is_empty());
    assert!(r.harness.state().session.stage_view.history.is_empty());
}

#[test]
fn placement_is_read_only_without_the_lease_or_while_armed() {
    for armed in [false, true] {
        let mut r = open(DEFAULT_WINDOW);
        let before = r.harness.state().session.project.clone();
        let s = &mut r.harness.state_mut().session;
        if armed {
            s.status.transport.output = Output::Armed;
        } else {
            s.status.you_hold_lease = false;
        }
        r.harness.run_steps(3);
        assert!(r.harness.get_by_label("Inverted mount").accesskit_node().is_disabled());
        r.press("Inverted mount");
        let from = preview(&r).center();
        r.drag(from, from + vec2(35.0, 20.0));
        assert_eq!(r.harness.state().session.project, before);
        assert!(only_projects(&r).is_empty());
        assert!(!pixels(&r).is_empty(), "a read-only station can still inspect the preview");
    }
}

#[test]
fn losing_the_edit_lease_during_a_preview_drag_discards_the_change() {
    let mut r = open(DEFAULT_WINDOW);
    let before = r.harness.state().session.project.clone();
    let from = preview(&r).center();
    r.harness.hover_at(from);
    r.harness.run_steps(2);
    r.harness.drag_at(from);
    r.harness.run_steps(2);
    let to = from + vec2(35.0, 20.0);
    r.harness.hover_at(to);
    r.harness.run_steps(3);
    assert_ne!(r.harness.state().session.project, before);
    r.harness.state_mut().session.status.you_hold_lease = false;
    r.harness.drop_at(to);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.project, before);
    assert!(only_projects(&r).is_empty());
}

#[test]
fn a_server_project_update_during_placement_is_not_overwritten_by_the_cancelled_drag() {
    let mut r = open(DEFAULT_WINDOW);
    let mut authoritative = r.harness.state().session.project.clone();
    authoritative.cameras[1].mount.x = -7.0;
    authoritative.cameras[1].mount.heading_deg = 32.0;
    let from = preview(&r).center();
    r.harness.hover_at(from);
    r.harness.run_steps(2);
    r.harness.drag_at(from);
    r.harness.run_steps(2);
    let to = from + vec2(35.0, 20.0);
    r.harness.hover_at(to);
    r.harness.run_steps(3);

    r.events.send(Event::Project(Box::new(authoritative.clone()))).unwrap();
    r.harness.state_mut().session.status.you_hold_lease = false;
    r.harness.run_steps(3);
    r.harness.drop_at(to);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.project, authoritative, "a cancelled preview must not roll back a server update");
    assert!(only_projects(&r).is_empty());
}

#[test]
fn opening_settings_cancels_an_unfinished_preview_drag() {
    let mut r = open(DEFAULT_WINDOW);
    let before = r.harness.state().session.project.clone();
    let from = preview(&r).center();
    r.harness.hover_at(from);
    r.harness.run_steps(2);
    r.harness.drag_at(from);
    r.harness.run_steps(2);
    let to = from + vec2(35.0, 20.0);
    r.harness.hover_at(to);
    r.harness.run_steps(3);
    assert_ne!(r.harness.state().session.project, before);

    r.harness.state_mut().session.settings = Some(Page::Position);
    r.harness.run_steps(3);
    r.harness.drop_at(to);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.project, before);
    assert!(only_projects(&r).is_empty());
    r.harness.state_mut().session.settings = None;
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.project, before, "returning must not resume the abandoned drag");
}

#[test]
fn placement_keeps_the_stage_preview_and_controls_inside_the_smallest_window() {
    for window in [SMALLEST_WINDOW, DEFAULT_WINDOW] {
        let mut r = open(window);
        let bounds = egui::Rect::from_min_size(Pos2::ZERO, window);
        let camera_view = preview(&r);
        let stage = r.harness.state().session.stage_view.probe.unwrap().rect;
        super::safety::shot(&r, &format!("placement-{}x{}", window.x, window.y));
        assert!(bounds.contains_rect(camera_view) && bounds.contains_rect(stage));
        assert!(!camera_view.intersects(stage), "the preview does not cover the stage tools");
        assert!(camera_view.width() >= 160.0 && camera_view.height() >= 90.0, "a useful camera preview: {camera_view:?}");
        assert!(stage.width() >= 200.0 && stage.height() >= 120.0, "a useful stage view: {stage:?}");
        r.press("Inverted mount");
        assert!(r.mount("cam2").inverted, "the mount control is reachable at {window:?}");
        super::safety::shot(&r, &format!("placement-{}x{}", window.x, window.y));
        r.press("Done placing");
        assert!(!r.harness.state().session.stage_view.placement, "Done placing restores the normal workspace");
        assert!(
            r.harness.state().session.stage_view.probe.unwrap().rect.width() < stage.width(),
            "the saved dock layout returns"
        );
    }
}
