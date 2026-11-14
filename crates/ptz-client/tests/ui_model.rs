//! UI-model test: transport button enablement matches what the server will accept.

use ptz_core::transport::{TransportAction, TransportPolicy, TransportSnapshot};

fn assert_ui_matches_server(snap: &TransportSnapshot) {
    let g = TransportPolicy::guidance(snap);
    let actions = [
        (TransportAction::Play, g.buttons.play),
        (TransportAction::Preview, g.buttons.preview),
        (TransportAction::Prepare, g.buttons.prepare),
        (TransportAction::Arm, g.buttons.arm),
        (TransportAction::Stop, g.buttons.stop),
        (TransportAction::SaveCommit, g.buttons.save_commit),
        (TransportAction::Estop, g.buttons.estop),
        (TransportAction::ClearEstop, g.buttons.clear_estop),
        (TransportAction::Cue, g.buttons.cue),
    ];
    for (action, enabled) in actions {
        assert_eq!(
            TransportPolicy::can(snap, action).is_ok(),
            enabled,
            "UI/server mismatch for {action:?} in {:?}",
            snap.phase
        );
    }
}

#[test]
fn editing_play_disabled_preview_enabled() {
    let snap = TransportSnapshot {
        phase: ptz_core::TransportPhase::Editing,
        mode: ptz_core::TransportMode::Idle,
        dirty: true,
        committed: false,
        playhead_frames: 0,
        song_id: Some("s".into()),
        hardware_live: false,
    };
    assert_ui_matches_server(&snap);
    assert!(!TransportPolicy::guidance(&snap).buttons.play);
    assert!(TransportPolicy::guidance(&snap).buttons.preview);
    assert!(
        TransportPolicy::guidance(&snap)
            .play_blocked_reason
            .as_deref()
            .unwrap()
            .contains("Save/Commit")
    );
}

#[test]
fn armed_play_enabled_matches_server() {
    let snap = TransportSnapshot {
        phase: ptz_core::TransportPhase::Armed,
        mode: ptz_core::TransportMode::Live,
        dirty: false,
        committed: true,
        playhead_frames: 0,
        song_id: Some("s".into()),
        hardware_live: false,
    };
    assert_ui_matches_server(&snap);
    assert!(TransportPolicy::guidance(&snap).buttons.play);
}
