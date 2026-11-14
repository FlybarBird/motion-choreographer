//! Authoritative transport state machine shared by server and UI.
//!
//! Design goals:
//! - Live path is deliberate: Commit → Prepare → Arm → Play
//! - Preview is one-click and never drives hardware
//! - Stop pauses; Play resumes; Cue re-cues to start
//! - E-Stop is always available
//! - UI button enablement is derived from the same rules the server enforces

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransportError {
    #[error("{0}")]
    Rejected(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportPhase {
    /// Song has unsaved / uncommitted edits.
    Editing,
    /// Committed revision selected; idle (not prepared).
    Idle,
    /// Live path: hardware prepared, not armed.
    Prepared,
    /// Live path: armed, waiting for Play.
    Armed,
    /// Live playback driving hardware.
    LivePlaying,
    /// Live playback paused.
    LivePaused,
    /// Preview playback (no hardware output).
    PreviewPlaying,
    /// Preview paused.
    PreviewPaused,
    /// Emergency stop latched.
    Estopped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportMode {
    /// No output / editing.
    Idle,
    /// Audible timeline preview; simulated monitor only.
    Preview,
    /// Live show path that may drive FR7/AutoPod.
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportAction {
    SaveCommit,
    Prepare,
    Arm,
    Play,
    Preview,
    Stop,
    Cue,
    Estop,
    ClearEstop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportSnapshot {
    pub phase: TransportPhase,
    pub mode: TransportMode,
    pub dirty: bool,
    pub committed: bool,
    pub playhead_frames: i64,
    pub song_id: Option<String>,
    pub hardware_live: bool,
}

impl TransportSnapshot {
    pub fn editing_default() -> Self {
        Self {
            phase: TransportPhase::Editing,
            mode: TransportMode::Idle,
            dirty: true,
            committed: false,
            playhead_frames: 0,
            song_id: None,
            hardware_live: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportButtons {
    pub save_commit: bool,
    pub prepare: bool,
    pub arm: bool,
    pub play: bool,
    pub preview: bool,
    pub stop: bool,
    pub cue: bool,
    pub estop: bool,
    pub clear_estop: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportGuidance {
    pub buttons: TransportButtons,
    /// Human-readable next step shown beside disabled primary actions.
    pub next_hint: String,
    pub play_blocked_reason: Option<String>,
    pub preview_blocked_reason: Option<String>,
    pub prepare_blocked_reason: Option<String>,
    pub arm_blocked_reason: Option<String>,
}

/// Pure policy: both server rejection and UI enablement call this.
pub struct TransportPolicy;

impl TransportPolicy {
    pub fn guidance(snap: &TransportSnapshot) -> TransportGuidance {
        let mut buttons = TransportButtons {
            save_commit: snap.dirty || !snap.committed,
            prepare: false,
            arm: false,
            play: false,
            preview: false,
            stop: false,
            cue: true,
            estop: snap.phase != TransportPhase::Estopped,
            clear_estop: snap.phase == TransportPhase::Estopped,
        };

        let mut play_blocked = None;
        let mut preview_blocked = None;
        let mut prepare_blocked = None;
        let mut arm_blocked = None;

        match snap.phase {
            TransportPhase::Estopped => {
                play_blocked = Some("E-Stop latched — Clear E-Stop first".into());
                preview_blocked = Some("E-Stop latched — Clear E-Stop first".into());
                prepare_blocked = Some("E-Stop latched — Clear E-Stop first".into());
                arm_blocked = Some("E-Stop latched — Clear E-Stop first".into());
            }
            TransportPhase::Editing => {
                buttons.preview = snap.song_id.is_some();
                buttons.prepare = false;
                buttons.arm = false;
                buttons.play = false;
                play_blocked = Some("Unsaved changes — Save/Commit first".into());
                prepare_blocked = Some("Unsaved changes — Save/Commit first".into());
                arm_blocked = Some("Not prepared — Prepare first".into());
                if snap.song_id.is_none() {
                    preview_blocked = Some("No song selected".into());
                    buttons.preview = false;
                }
            }
            TransportPhase::Idle => {
                buttons.preview = snap.song_id.is_some();
                buttons.prepare = snap.committed && snap.song_id.is_some() && !snap.dirty;
                buttons.play = false;
                buttons.arm = false;
                if !snap.committed || snap.dirty {
                    play_blocked = Some("Unsaved changes — Save/Commit first".into());
                    prepare_blocked = Some("Unsaved changes — Save/Commit first".into());
                } else {
                    play_blocked = Some("Not prepared — Prepare first".into());
                    arm_blocked = Some("Not prepared — Prepare first".into());
                }
                if snap.song_id.is_none() {
                    preview_blocked = Some("No song selected".into());
                    buttons.preview = false;
                    prepare_blocked = Some("No song selected".into());
                    buttons.prepare = false;
                }
            }
            TransportPhase::Prepared => {
                buttons.preview = true;
                buttons.prepare = false;
                buttons.arm = true;
                buttons.play = false;
                play_blocked = Some("Not armed — Arm first".into());
            }
            TransportPhase::Armed => {
                buttons.preview = false; // deliberate: leave live path for preview
                buttons.arm = false;
                buttons.play = true;
                buttons.prepare = false;
                preview_blocked = Some("Armed for live — Disarm/Stop path or E-Stop to preview".into());
            }
            TransportPhase::LivePlaying => {
                buttons.stop = true;
                buttons.play = false;
                buttons.preview = false;
                play_blocked = Some("Already playing".into());
            }
            TransportPhase::LivePaused => {
                buttons.play = true; // resume live
                buttons.stop = false;
                buttons.preview = false;
                buttons.cue = true;
            }
            TransportPhase::PreviewPlaying => {
                buttons.stop = true;
                buttons.play = false;
                buttons.preview = false;
                play_blocked = Some("In preview — Stop first for live path".into());
            }
            TransportPhase::PreviewPaused => {
                buttons.preview = true; // resume preview
                buttons.play = false;
                buttons.cue = true;
                if snap.committed && !snap.dirty {
                    play_blocked = Some("Not prepared — Prepare first".into());
                } else {
                    play_blocked = Some("Unsaved changes — Save/Commit first".into());
                }
            }
        }

        let next_hint = if snap.phase == TransportPhase::Estopped {
            "E-Stop latched. Clear E-Stop when safe.".to_string()
        } else if buttons.play {
            "Ready: Play (live)".to_string()
        } else if buttons.arm {
            "Next: Arm for live hardware".to_string()
        } else if buttons.prepare {
            "Next: Prepare for live".to_string()
        } else if buttons.save_commit && (snap.dirty || !snap.committed) {
            "Next: Save/Commit before live Play".to_string()
        } else if buttons.preview {
            "Preview available (no hardware). Live path: Prepare → Arm → Play.".to_string()
        } else if buttons.stop {
            "Playing — Stop pauses; Cue re-cues to start.".to_string()
        } else {
            "Select a song to begin.".to_string()
        };

        TransportGuidance {
            buttons,
            next_hint,
            play_blocked_reason: play_blocked,
            preview_blocked_reason: preview_blocked,
            prepare_blocked_reason: prepare_blocked,
            arm_blocked_reason: arm_blocked,
        }
    }

    pub fn can(snap: &TransportSnapshot, action: TransportAction) -> Result<(), TransportError> {
        let g = Self::guidance(snap);
        let ok = match action {
            TransportAction::SaveCommit => g.buttons.save_commit,
            TransportAction::Prepare => g.buttons.prepare,
            TransportAction::Arm => g.buttons.arm,
            TransportAction::Play => g.buttons.play,
            TransportAction::Preview => g.buttons.preview,
            TransportAction::Stop => g.buttons.stop,
            TransportAction::Cue => g.buttons.cue,
            TransportAction::Estop => g.buttons.estop,
            TransportAction::ClearEstop => g.buttons.clear_estop,
        };
        if ok {
            Ok(())
        } else {
            let reason = match action {
                TransportAction::Play => g.play_blocked_reason,
                TransportAction::Preview => g.preview_blocked_reason,
                TransportAction::Prepare => g.prepare_blocked_reason,
                TransportAction::Arm => g.arm_blocked_reason,
                _ => None,
            }
            .unwrap_or_else(|| format!("{action:?} not available in {:?}", snap.phase));
            Err(TransportError::Rejected(reason))
        }
    }

    /// Apply a validated action. Caller must have called `can` (or this rechecks).
    pub fn apply(
        snap: &TransportSnapshot,
        action: TransportAction,
    ) -> Result<TransportSnapshot, TransportError> {
        Self::can(snap, action)?;
        let mut next = snap.clone();
        match action {
            TransportAction::SaveCommit => {
                next.dirty = false;
                next.committed = true;
                if next.phase == TransportPhase::Editing {
                    next.phase = TransportPhase::Idle;
                }
                next.mode = TransportMode::Idle;
                next.hardware_live = false;
            }
            TransportAction::Prepare => {
                next.phase = TransportPhase::Prepared;
                next.mode = TransportMode::Live;
                next.hardware_live = false;
            }
            TransportAction::Arm => {
                next.phase = TransportPhase::Armed;
                next.mode = TransportMode::Live;
                next.hardware_live = false;
            }
            TransportAction::Play => {
                next.phase = TransportPhase::LivePlaying;
                next.mode = TransportMode::Live;
                next.hardware_live = true;
            }
            TransportAction::Preview => {
                next.phase = TransportPhase::PreviewPlaying;
                next.mode = TransportMode::Preview;
                next.hardware_live = false;
            }
            TransportAction::Stop => match next.phase {
                TransportPhase::LivePlaying => {
                    next.phase = TransportPhase::LivePaused;
                    next.hardware_live = false;
                }
                TransportPhase::PreviewPlaying => {
                    next.phase = TransportPhase::PreviewPaused;
                    next.hardware_live = false;
                }
                _ => {}
            },
            TransportAction::Cue => {
                next.playhead_frames = 0;
                match next.phase {
                    TransportPhase::LivePlaying | TransportPhase::LivePaused => {
                        next.phase = TransportPhase::LivePaused;
                        next.hardware_live = false;
                    }
                    TransportPhase::PreviewPlaying | TransportPhase::PreviewPaused => {
                        next.phase = TransportPhase::PreviewPaused;
                        next.hardware_live = false;
                    }
                    TransportPhase::Armed | TransportPhase::Prepared => {
                        // stay armed/prepared, re-cue
                    }
                    TransportPhase::Estopped => {}
                    _ => {
                        next.phase = if next.committed && !next.dirty {
                            TransportPhase::Idle
                        } else {
                            TransportPhase::Editing
                        };
                        next.mode = TransportMode::Idle;
                        next.hardware_live = false;
                    }
                }
            }
            TransportAction::Estop => {
                next.phase = TransportPhase::Estopped;
                next.mode = TransportMode::Idle;
                next.hardware_live = false;
            }
            TransportAction::ClearEstop => {
                next.phase = if next.committed && !next.dirty {
                    TransportPhase::Idle
                } else {
                    TransportPhase::Editing
                };
                next.mode = TransportMode::Idle;
                next.hardware_live = false;
            }
        }
        Ok(next)
    }

    /// Mark the song dirty (operator edited). Drops live arm/prepare.
    pub fn mark_dirty(snap: &TransportSnapshot) -> TransportSnapshot {
        let mut next = snap.clone();
        next.dirty = true;
        next.committed = false;
        if !matches!(
            next.phase,
            TransportPhase::PreviewPlaying
                | TransportPhase::PreviewPaused
                | TransportPhase::Estopped
        ) {
            next.phase = TransportPhase::Editing;
            next.mode = TransportMode::Idle;
            next.hardware_live = false;
        } else if matches!(
            next.phase,
            TransportPhase::PreviewPlaying | TransportPhase::PreviewPaused
        ) {
            // keep preview; but committed flag cleared for live path
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn committed_idle() -> TransportSnapshot {
        TransportSnapshot {
            phase: TransportPhase::Idle,
            mode: TransportMode::Idle,
            dirty: false,
            committed: true,
            playhead_frames: 0,
            song_id: Some("song1".into()),
            hardware_live: false,
        }
    }

    fn editing() -> TransportSnapshot {
        TransportSnapshot {
            phase: TransportPhase::Editing,
            mode: TransportMode::Idle,
            dirty: true,
            committed: false,
            playhead_frames: 0,
            song_id: Some("song1".into()),
            hardware_live: false,
        }
    }

    #[test]
    fn play_rejected_while_editing() {
        let snap = editing();
        let g = TransportPolicy::guidance(&snap);
        assert!(!g.buttons.play);
        assert!(g.play_blocked_reason.as_deref().unwrap().contains("Save/Commit"));
        assert!(TransportPolicy::can(&snap, TransportAction::Play).is_err());
        // Preview still available
        assert!(g.buttons.preview);
        assert!(TransportPolicy::can(&snap, TransportAction::Preview).is_ok());
    }

    #[test]
    fn live_path_prepare_arm_play() {
        let mut snap = committed_idle();
        snap = TransportPolicy::apply(&snap, TransportAction::Prepare).unwrap();
        assert_eq!(snap.phase, TransportPhase::Prepared);
        assert!(!TransportPolicy::guidance(&snap).buttons.play);
        snap = TransportPolicy::apply(&snap, TransportAction::Arm).unwrap();
        assert_eq!(snap.phase, TransportPhase::Armed);
        assert!(TransportPolicy::guidance(&snap).buttons.play);
        snap = TransportPolicy::apply(&snap, TransportAction::Play).unwrap();
        assert!(snap.hardware_live);
        assert_eq!(snap.phase, TransportPhase::LivePlaying);
    }

    #[test]
    fn preview_never_sets_hardware_live() {
        let snap = editing();
        let next = TransportPolicy::apply(&snap, TransportAction::Preview).unwrap();
        assert!(!next.hardware_live);
        assert_eq!(next.mode, TransportMode::Preview);
        assert_eq!(next.phase, TransportPhase::PreviewPlaying);
    }

    #[test]
    fn stop_pauses_play_resumes() {
        let mut snap = committed_idle();
        snap = TransportPolicy::apply(&snap, TransportAction::Prepare).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Arm).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Play).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Stop).unwrap();
        assert_eq!(snap.phase, TransportPhase::LivePaused);
        assert!(!snap.hardware_live);
        snap = TransportPolicy::apply(&snap, TransportAction::Play).unwrap();
        assert_eq!(snap.phase, TransportPhase::LivePlaying);
    }

    #[test]
    fn cue_resets_playhead() {
        let mut snap = committed_idle();
        snap.playhead_frames = 1234;
        snap = TransportPolicy::apply(&snap, TransportAction::Cue).unwrap();
        assert_eq!(snap.playhead_frames, 0);
    }

    #[test]
    fn estop_always_then_clear() {
        let mut snap = committed_idle();
        snap = TransportPolicy::apply(&snap, TransportAction::Prepare).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Arm).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Play).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Estop).unwrap();
        assert_eq!(snap.phase, TransportPhase::Estopped);
        assert!(!snap.hardware_live);
        assert!(TransportPolicy::can(&snap, TransportAction::Play).is_err());
        snap = TransportPolicy::apply(&snap, TransportAction::ClearEstop).unwrap();
        assert_eq!(snap.phase, TransportPhase::Idle);
    }

    #[test]
    fn every_transition_matrix_smoke() {
        // Exhaustive can()/apply() for a representative set of phases.
        let phases = [
            TransportPhase::Editing,
            TransportPhase::Idle,
            TransportPhase::Prepared,
            TransportPhase::Armed,
            TransportPhase::LivePlaying,
            TransportPhase::LivePaused,
            TransportPhase::PreviewPlaying,
            TransportPhase::PreviewPaused,
            TransportPhase::Estopped,
        ];
        let actions = [
            TransportAction::SaveCommit,
            TransportAction::Prepare,
            TransportAction::Arm,
            TransportAction::Play,
            TransportAction::Preview,
            TransportAction::Stop,
            TransportAction::Cue,
            TransportAction::Estop,
            TransportAction::ClearEstop,
        ];
        for phase in phases {
            let snap = TransportSnapshot {
                phase,
                mode: TransportMode::Idle,
                dirty: phase == TransportPhase::Editing,
                committed: phase != TransportPhase::Editing,
                playhead_frames: 10,
                song_id: Some("s".into()),
                hardware_live: phase == TransportPhase::LivePlaying,
            };
            let g = TransportPolicy::guidance(&snap);
            for action in actions {
                let allowed = match action {
                    TransportAction::SaveCommit => g.buttons.save_commit,
                    TransportAction::Prepare => g.buttons.prepare,
                    TransportAction::Arm => g.buttons.arm,
                    TransportAction::Play => g.buttons.play,
                    TransportAction::Preview => g.buttons.preview,
                    TransportAction::Stop => g.buttons.stop,
                    TransportAction::Cue => g.buttons.cue,
                    TransportAction::Estop => g.buttons.estop,
                    TransportAction::ClearEstop => g.buttons.clear_estop,
                };
                let result = TransportPolicy::apply(&snap, action);
                assert_eq!(
                    result.is_ok(),
                    allowed,
                    "phase={phase:?} action={action:?} guidance={allowed}"
                );
            }
        }
    }

    #[test]
    fn ui_buttons_match_server_acceptance() {
        // The UI-model contract: for every phase, button.enabled iff can(action).ok()
        let snaps = [
            editing(),
            committed_idle(),
            TransportPolicy::apply(&committed_idle(), TransportAction::Prepare).unwrap(),
            {
                let s = TransportPolicy::apply(&committed_idle(), TransportAction::Prepare).unwrap();
                TransportPolicy::apply(&s, TransportAction::Arm).unwrap()
            },
        ];
        for snap in snaps {
            let g = TransportPolicy::guidance(&snap);
            assert_eq!(
                g.buttons.play,
                TransportPolicy::can(&snap, TransportAction::Play).is_ok()
            );
            assert_eq!(
                g.buttons.preview,
                TransportPolicy::can(&snap, TransportAction::Preview).is_ok()
            );
            assert_eq!(
                g.buttons.prepare,
                TransportPolicy::can(&snap, TransportAction::Prepare).is_ok()
            );
            assert_eq!(
                g.buttons.arm,
                TransportPolicy::can(&snap, TransportAction::Arm).is_ok()
            );
        }
    }

    #[test]
    fn mark_dirty_drops_arm() {
        let mut snap = committed_idle();
        snap = TransportPolicy::apply(&snap, TransportAction::Prepare).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Arm).unwrap();
        snap = TransportPolicy::mark_dirty(&snap);
        assert_eq!(snap.phase, TransportPhase::Editing);
        assert!(!snap.hardware_live);
        assert!(TransportPolicy::can(&snap, TransportAction::Play).is_err());
    }
}
