//! UI-model test: transport button enablement matches what the server will accept,
//! and guidance never names a disabled action.

use ptz_core::transport::{
    actions_named_in_text, TransportAction, TransportPhase, TransportPolicy, TransportSnapshot,
};

fn assert_ui_matches_server(snap: &TransportSnapshot) {
    let g = TransportPolicy::guidance(snap);
    for action in TransportAction::all() {
        assert_eq!(
            TransportPolicy::can(snap, *action).is_ok(),
            g.buttons.enabled(*action),
            "UI/server mismatch for {action:?} in {:?}",
            snap.phase
        );
    }
}

fn assert_guidance_no_dead_ends(snap: &TransportSnapshot) {
    let g = TransportPolicy::guidance(snap);
    let mut texts = vec![g.next_hint.clone()];
    for opt in [
        &g.play_blocked_reason,
        &g.preview_blocked_reason,
        &g.prepare_blocked_reason,
        &g.arm_blocked_reason,
        &g.disarm_blocked_reason,
    ] {
        if let Some(t) = opt {
            texts.push(t.clone());
        }
    }
    for text in texts {
        for action in actions_named_in_text(&text) {
            assert!(
                g.buttons.enabled(action),
                "phase={:?}: guidance names {:?} but it is disabled. text={text:?}",
                snap.phase,
                action
            );
        }
    }
}

fn snap(phase: TransportPhase, dirty: bool) -> TransportSnapshot {
    TransportSnapshot {
        phase,
        mode: ptz_core::TransportMode::Idle,
        dirty,
        committed: !dirty,
        playhead_frames: 0,
        song_id: Some("s".into()),
        hardware_live: phase == TransportPhase::LivePlaying,
    }
}

#[test]
fn editing_play_disabled_preview_enabled() {
    let s = snap(TransportPhase::Editing, true);
    assert_ui_matches_server(&s);
    assert_guidance_no_dead_ends(&s);
    assert!(!TransportPolicy::guidance(&s).buttons.play);
    assert!(TransportPolicy::guidance(&s).buttons.preview);
    assert!(
        TransportPolicy::guidance(&s)
            .play_blocked_reason
            .as_deref()
            .unwrap()
            .contains("Save/Commit")
    );
}

#[test]
fn armed_play_and_disarm_enabled() {
    let s = snap(TransportPhase::Armed, false);
    assert_ui_matches_server(&s);
    assert_guidance_no_dead_ends(&s);
    let g = TransportPolicy::guidance(&s);
    assert!(g.buttons.play);
    assert!(g.buttons.disarm);
    assert!(!g.buttons.preview);
    assert!(g.preview_blocked_reason.as_deref().unwrap().contains("Disarm"));
}

#[test]
fn preview_paused_prepare_enabled() {
    let s = snap(TransportPhase::PreviewPaused, false);
    assert_ui_matches_server(&s);
    assert_guidance_no_dead_ends(&s);
    assert!(TransportPolicy::guidance(&s).buttons.prepare);
}

#[test]
fn live_paused_disarm_enabled() {
    let s = snap(TransportPhase::LivePaused, false);
    assert_ui_matches_server(&s);
    assert_guidance_no_dead_ends(&s);
    assert!(TransportPolicy::guidance(&s).buttons.disarm);
}

#[test]
fn every_phase_guidance_names_only_enabled_actions() {
    for phase in [
        TransportPhase::Editing,
        TransportPhase::Idle,
        TransportPhase::Prepared,
        TransportPhase::Armed,
        TransportPhase::LivePlaying,
        TransportPhase::LivePaused,
        TransportPhase::PreviewPlaying,
        TransportPhase::PreviewPaused,
        TransportPhase::Estopped,
    ] {
        let dirty = phase == TransportPhase::Editing;
        let s = snap(phase, dirty);
        assert_ui_matches_server(&s);
        assert_guidance_no_dead_ends(&s);
    }
}
