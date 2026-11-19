//! The station's preset drop time, through the actual preset toolbar and drag gesture.

use super::*;
use crate::timeline::PresetDropDuration;
use egui_kittest::kittest::Queryable as _;

fn type_duration(r: &mut Rig, seconds: &str) {
    let role = egui::accesskit::Role::SpinButton;
    r.harness.get_by_role_and_label(role, "Drop time").focus();
    r.harness.run_steps(2);
    r.harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    r.harness.run_steps(1);
    r.harness.get_by_role_and_label(role, "Drop time").type_text(seconds);
    r.harness.run_steps(2);
    r.harness.key_press(egui::Key::Enter);
    r.harness.run_steps(3);
}

#[test]
fn preset_duration_edit_sets_new_drop_time_and_auto_restores_automatic_moves() {
    let mut r = rig(Song::new("s", "Preset time", "01:00:00:00", 1500));
    assert_eq!(r.harness.state().session.timeline.preset_duration, PresetDropDuration::default());
    type_duration(&mut r, "5");
    assert!(!r.harness.state().session.timeline.preset_duration.automatic, "typing the suggested value also selects fixed time");
    type_duration(&mut r, "2.5");
    assert_eq!(r.harness.state().session.timeline.preset_duration, PresetDropDuration { automatic: false, seconds: 2.5 });
    assert!(r.puts().is_empty(), "changing this station's preference does not edit the song");
    let from = r.preset_point("lectern");
    let to = r.lane_point(Lane::Motion(0), 200.0);
    r.drag(from, to);
    let fixed = r.song().motion[0].clone();
    assert_eq!(fixed.duration_frames, 63, "2.5 seconds rounds to 63 frames at 25 fps");
    assert!(matches!(fixed.body, ClipBody::PresetMove { auto_duration: false, .. }));
    let beside_duration =
        r.harness.get_by_role_and_label(egui::accesskit::Role::SpinButton, "Drop time").rect().right_center() + vec2(20.0, 0.0);
    let auto = r
        .harness
        .query_all_by_label("Auto")
        .min_by(|a, b| a.rect().center().distance_sq(beside_duration).total_cmp(&b.rect().center().distance_sq(beside_duration)))
        .expect("the preset timing Auto option")
        .rect()
        .center();
    r.click(auto);
    assert!(r.harness.state().session.timeline.preset_duration.automatic);
    assert_eq!(r.song().motion[0], fixed, "Auto only affects future drops");
    let from = r.preset_point("wide");
    let to = r.lane_point(Lane::Motion(0), 700.0);
    r.drag(from, to);
    let automatic = r.song().motion.iter().find(|clip| clip.id != fixed.id).unwrap();
    assert!(matches!(automatic.body, ClipBody::PresetMove { auto_duration: true, .. }));
    assert_eq!(r.song().motion.iter().find(|clip| clip.id == fixed.id), Some(&fixed));
}

#[test]
fn preset_duration_survives_settings_round_trip_and_opening_another_song() {
    let preference = PresetDropDuration { automatic: false, seconds: 7.25 };
    let settings = crate::app::Settings { preset_duration: preference, ..Default::default() };
    let saved = serde_json::to_vec(&settings).unwrap();
    let restored: crate::app::Settings = serde_json::from_slice(&saved).unwrap();
    assert_eq!(restored.preset_duration, preference);
    let legacy: crate::app::Settings = serde_json::from_str(r#"{"station":"Booth"}"#).unwrap();
    assert_eq!(legacy.preset_duration, PresetDropDuration::default());

    let mut r = rig(Song::new("s", "First", "01:00:00:00", 1500));
    r.harness.state_mut().session.timeline.preset_duration = restored.preset_duration;
    r.events
        .send(Event::Working(
            Box::new(ptz_core::api::WorkingSong {
                song: Song::new("next", "Next", "02:00:00:00", 1500),
                edit_counter: 0,
                dirty: false,
                revision: None,
                issues: Vec::new(),
            }),
            crate::link::Why::Open,
        ))
        .unwrap();
    r.harness.run_steps(4);
    assert_eq!(r.song().id, "next");
    assert_eq!(r.harness.state().session.timeline.preset_duration, preference);
}

#[test]
fn pasting_the_suggested_preset_duration_selects_fixed_time_without_a_timeline_paste() {
    let mut r = rig(Song::new("s", "Preset time", "01:00:00:00", 1500));
    r.harness.get_by_role_and_label(egui::accesskit::Role::SpinButton, "Drop time").focus();
    r.harness.run_steps(2);
    r.harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    r.harness.run_steps(1);
    r.harness.event(egui::Event::Paste("5".into()));
    r.harness.run_steps(2);
    r.harness.key_press(egui::Key::Enter);
    r.harness.run_steps(3);
    assert_eq!(r.harness.state().session.timeline.preset_duration, PresetDropDuration { automatic: false, seconds: 5.0 });
    assert!(r.puts().is_empty());
    assert!(r.song().motion.is_empty());
}

#[test]
fn preset_duration_controls_fit_and_work_at_supported_window_sizes() {
    for window in [SMALLEST_WINDOW, DEFAULT_WINDOW] {
        let mut r = rig_sized(window, Project::default(), presets(), Song::new("s", "Preset time", "01:00:00:00", 1500));
        type_duration(&mut r, "8.5");
        let control = r.harness.get_by_role_and_label(egui::accesskit::Role::SpinButton, "Drop time").rect();
        assert!(egui::Rect::from_min_size(Pos2::ZERO, window).contains_rect(control));
        let card = r.harness.state().session.probe_presets.iter().find(|(id, _)| id == "wide").unwrap().1;
        assert!(control.left() >= card.left() && control.right() <= card.right(), "the value fits inside the Presets tab");
        let auto = r
            .harness
            .query_all_by_label("Auto")
            .min_by(|a, b| {
                a.rect()
                    .center()
                    .distance_sq(control.right_center())
                    .total_cmp(&b.rect().center().distance_sq(control.right_center()))
            })
            .unwrap()
            .rect();
        assert!(auto.left() >= card.left() && auto.right() <= card.right(), "Auto remains inside the Presets tab");
        assert!(control.bottom() < r.preset_point("wide").y, "the preset cards remain below the controls");
        assert_eq!(r.harness.state().session.timeline.preset_duration, PresetDropDuration { automatic: false, seconds: 8.5 });
        super::safety::shot(&r, &format!("preset-duration-{}x{}", window.x, window.y));
    }
}
