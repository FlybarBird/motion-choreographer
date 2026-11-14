//! Authoritative transport state machine shared by server and UI.
//!
//! Design goals:
//! - Live path is deliberate: Commit → Prepare → Arm → Play
//! - Preview is one-click and never drives hardware
//! - Stop pauses; Play resumes; Cue re-cues to start
//! - Disarm exits prepared/armed/live_paused back to idle (hardware still)
//! - E-Stop is always available
//! - Guidance never names an action that is disabled in the current phase

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransportAction {
    SaveCommit,
    Prepare,
    Arm,
    Disarm,
    Play,
    Preview,
    Stop,
    Cue,
    Estop,
    ClearEstop,
}

impl TransportAction {
    /// Operator-facing label used in hints / blocked reasons.
    pub fn label(self) -> &'static str {
        match self {
            Self::SaveCommit => "Save/Commit",
            Self::Prepare => "Prepare",
            Self::Arm => "Arm",
            Self::Disarm => "Disarm",
            Self::Play => "Play",
            Self::Preview => "Preview",
            Self::Stop => "Stop",
            Self::Cue => "Cue",
            Self::Estop => "E-Stop",
            Self::ClearEstop => "Clear E-Stop",
        }
    }

    pub fn all() -> &'static [TransportAction] {
        &[
            Self::SaveCommit,
            Self::Prepare,
            Self::Arm,
            Self::Disarm,
            Self::Play,
            Self::Preview,
            Self::Stop,
            Self::Cue,
            Self::Estop,
            Self::ClearEstop,
        ]
    }
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

    fn idle_or_editing_phase(&self) -> TransportPhase {
        if self.committed && !self.dirty {
            TransportPhase::Idle
        } else {
            TransportPhase::Editing
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportButtons {
    pub save_commit: bool,
    pub prepare: bool,
    pub arm: bool,
    pub disarm: bool,
    pub play: bool,
    pub preview: bool,
    pub stop: bool,
    pub cue: bool,
    pub estop: bool,
    pub clear_estop: bool,
}

impl TransportButtons {
    pub fn enabled(&self, action: TransportAction) -> bool {
        match action {
            TransportAction::SaveCommit => self.save_commit,
            TransportAction::Prepare => self.prepare,
            TransportAction::Arm => self.arm,
            TransportAction::Disarm => self.disarm,
            TransportAction::Play => self.play,
            TransportAction::Preview => self.preview,
            TransportAction::Stop => self.stop,
            TransportAction::Cue => self.cue,
            TransportAction::Estop => self.estop,
            TransportAction::ClearEstop => self.clear_estop,
        }
    }
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
    pub disarm_blocked_reason: Option<String>,
}

/// Pure policy: both server rejection and UI enablement call this.
pub struct TransportPolicy;

impl TransportPolicy {
    pub fn guidance(snap: &TransportSnapshot) -> TransportGuidance {
        let mut buttons = TransportButtons {
            save_commit: snap.dirty || !snap.committed,
            prepare: false,
            arm: false,
            disarm: false,
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
        let mut arm_blocked: Option<String> = None;
        let mut disarm_blocked = None;

        let can_prepare_live = snap.committed && !snap.dirty && snap.song_id.is_some();

        match snap.phase {
            TransportPhase::Estopped => {
                buttons.cue = false;
                buttons.save_commit = false;
                // Avoid the bare label "E-Stop" here (disabled); only name Clear E-Stop.
                let latched = "Latched — Clear E-Stop first".to_string();
                play_blocked = Some(latched.clone());
                preview_blocked = Some(latched.clone());
                prepare_blocked = Some(latched.clone());
                arm_blocked = Some(latched.clone());
                disarm_blocked = Some(latched);
            }
            TransportPhase::Editing => {
                buttons.preview = snap.song_id.is_some();
                play_blocked = Some("Unsaved changes — Save/Commit first".into());
                prepare_blocked = Some("Unsaved changes — Save/Commit first".into());
                arm_blocked = Some("Unsaved changes — Save/Commit first".into());
                disarm_blocked = Some("Not on live path".into());
                if snap.song_id.is_none() {
                    preview_blocked = Some("No song selected".into());
                    buttons.preview = false;
                }
            }
            TransportPhase::Idle => {
                buttons.preview = snap.song_id.is_some();
                buttons.prepare = can_prepare_live;
                if !can_prepare_live {
                    if snap.song_id.is_none() {
                        prepare_blocked = Some("No song selected".into());
                        play_blocked = Some("No song selected".into());
                        arm_blocked = Some("No song selected".into());
                        preview_blocked = Some("No song selected".into());
                        buttons.preview = false;
                    } else {
                        prepare_blocked = Some("Unsaved changes — Save/Commit first".into());
                        play_blocked = Some("Unsaved changes — Save/Commit first".into());
                        arm_blocked = Some("Unsaved changes — Save/Commit first".into());
                    }
                } else {
                    play_blocked = Some("Not prepared — Prepare first".into());
                    arm_blocked = Some("Not prepared — Prepare first".into());
                }
                disarm_blocked = Some("Not on live path".into());
            }
            TransportPhase::Prepared => {
                buttons.preview = true;
                buttons.arm = true;
                buttons.disarm = true;
                play_blocked = Some("Not armed — Arm first".into());
                prepare_blocked = Some("Already prepared — Arm or Disarm".into());
                arm_blocked = None;
            }
            TransportPhase::Armed => {
                buttons.play = true;
                buttons.disarm = true;
                buttons.arm = false;
                buttons.prepare = false;
                // Only name enabled actions (Disarm / Play) — never Preview here.
                preview_blocked = Some("Armed for live — Disarm first".into());
                prepare_blocked = Some("Already armed — Play or Disarm".into());
                arm_blocked = Some("Already armed — Play or Disarm".into());
            }
            TransportPhase::LivePlaying => {
                buttons.stop = true;
                buttons.cue = true;
                play_blocked = Some("Already playing — Stop pauses".into());
                preview_blocked = Some("Live playing — Stop first".into());
                prepare_blocked = Some("Live playing — Stop first".into());
                arm_blocked = Some("Live playing — Stop first".into());
                disarm_blocked = Some("Live playing — Stop first".into());
            }
            TransportPhase::LivePaused => {
                buttons.play = true; // resume live
                buttons.disarm = true; // end live → idle, hardware still
                buttons.cue = true;
                preview_blocked = Some("Live paused — Disarm first".into());
                prepare_blocked = Some("Live paused — Disarm first".into());
                arm_blocked = Some("Live paused — Play to resume, or Disarm to exit".into());
            }
            TransportPhase::PreviewPlaying => {
                buttons.stop = true;
                buttons.cue = true; // cue → idle (exit preview)
                // Only name Stop / Cue / Save/Commit — not Prepare (disabled until Stop).
                play_blocked = Some("In preview — Stop first".into());
                prepare_blocked = if can_prepare_live {
                    Some("In preview — Stop first".into())
                } else if buttons.save_commit {
                    Some("Unsaved changes — Save/Commit first".into())
                } else {
                    Some("In preview — Stop first".into())
                };
                arm_blocked = Some("In preview — Stop first".into());
                disarm_blocked = Some("Not on live path — Stop or Cue".into());
            }
            TransportPhase::PreviewPaused => {
                buttons.preview = true; // resume preview
                buttons.cue = true; // cue → idle
                buttons.prepare = can_prepare_live; // enter live path without E-Stop
                if can_prepare_live {
                    play_blocked = Some("Not prepared — Prepare first".into());
                    arm_blocked = Some("Not prepared — Prepare first".into());
                } else if buttons.save_commit {
                    play_blocked = Some("Unsaved changes — Save/Commit first".into());
                    prepare_blocked = Some("Unsaved changes — Save/Commit first".into());
                    arm_blocked = Some("Unsaved changes — Save/Commit first".into());
                } else {
                    play_blocked = Some("Not prepared — Cue to Idle, then Prepare".into());
                    // Cue is enabled — ok to name.
                    prepare_blocked = Some("Cannot Prepare — Cue to Idle first".into());
                    arm_blocked = Some("Cannot Arm — Cue to Idle first".into());
                }
                disarm_blocked = Some("Not on live path — Cue to Idle or Prepare".into());
                // If Prepare is enabled, disarm reason must not be the only escape named wrongly.
                if buttons.prepare {
                    disarm_blocked = Some("Not on live path — Prepare or Cue".into());
                } else if buttons.cue {
                    disarm_blocked = Some("Not on live path — Cue to Idle".into());
                }
            }
        }

        let next_hint = build_next_hint(snap, &buttons);

        TransportGuidance {
            buttons,
            next_hint,
            play_blocked_reason: play_blocked,
            preview_blocked_reason: preview_blocked,
            prepare_blocked_reason: prepare_blocked,
            arm_blocked_reason: arm_blocked,
            disarm_blocked_reason: disarm_blocked,
        }
    }

    pub fn can(snap: &TransportSnapshot, action: TransportAction) -> Result<(), TransportError> {
        let g = Self::guidance(snap);
        if g.buttons.enabled(action) {
            Ok(())
        } else {
            let reason = match action {
                TransportAction::Play => g.play_blocked_reason,
                TransportAction::Preview => g.preview_blocked_reason,
                TransportAction::Prepare => g.prepare_blocked_reason,
                TransportAction::Arm => g.arm_blocked_reason,
                TransportAction::Disarm => g.disarm_blocked_reason,
                _ => None,
            }
            .unwrap_or_else(|| format!("{} not available in {:?}", action.label(), snap.phase));
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
                // Stay in preview_paused if that is where we committed from.
                if !matches!(
                    next.phase,
                    TransportPhase::PreviewPlaying | TransportPhase::PreviewPaused
                ) {
                    next.mode = TransportMode::Idle;
                }
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
            TransportAction::Disarm => {
                // Exit live path; hardware stays still.
                next.phase = next.idle_or_editing_phase();
                next.mode = TransportMode::Idle;
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
                    // Cue exits preview back to idle/editing so the operator is not stuck.
                    TransportPhase::PreviewPlaying | TransportPhase::PreviewPaused => {
                        next.phase = next.idle_or_editing_phase();
                        next.mode = TransportMode::Idle;
                        next.hardware_live = false;
                    }
                    TransportPhase::Armed | TransportPhase::Prepared => {
                        // stay armed/prepared, re-cue
                    }
                    TransportPhase::Estopped => {}
                    _ => {
                        next.phase = next.idle_or_editing_phase();
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
                next.phase = next.idle_or_editing_phase();
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
        }
        next
    }
}

fn build_next_hint(snap: &TransportSnapshot, buttons: &TransportButtons) -> String {
    if buttons.clear_estop {
        return "Latched. Clear E-Stop when safe.".into();
    }
    if buttons.play && matches!(snap.phase, TransportPhase::Armed) {
        return "Ready: Play (live). Disarm cancels the live path.".into();
    }
    if buttons.play && matches!(snap.phase, TransportPhase::LivePaused) {
        return "Live paused — Play resumes, or Disarm to exit live path.".into();
    }
    if buttons.arm {
        return "Next: Arm for live hardware (or Disarm to cancel).".into();
    }
    if buttons.prepare {
        return "Next: Prepare for live (or Preview / Cue).".into();
    }
    if buttons.save_commit && (snap.dirty || !snap.committed) {
        // Only name Save/Commit — Prepare/Play are not enabled yet.
        return "Next: Save/Commit before the live path.".into();
    }
    if buttons.preview && buttons.prepare {
        return "Preview available. Or Prepare → Arm → Play for live.".into();
    }
    if buttons.preview && buttons.save_commit {
        return "Preview available (no hardware). Save/Commit before the live path.".into();
    }
    if buttons.preview {
        return "Preview available (no hardware).".into();
    }
    if buttons.stop {
        return "Playing — Stop pauses; Cue re-cues.".into();
    }
    if buttons.disarm {
        return "Next: Disarm to leave the live path.".into();
    }
    if snap.song_id.is_none() {
        return "Select a song to begin.".into();
    }
    "Ready.".into()
}

/// Labels that may appear in hints / blocked reasons; used by the no-dead-end test.
pub fn guidance_action_labels() -> &'static [(&'static str, TransportAction)] {
    // Longer phrases first so "Clear E-Stop" wins over "E-Stop".
    &[
        ("Clear E-Stop", TransportAction::ClearEstop),
        ("Save/Commit", TransportAction::SaveCommit),
        ("Prepare", TransportAction::Prepare),
        ("Disarm", TransportAction::Disarm),
        ("Preview", TransportAction::Preview),
        ("Arm", TransportAction::Arm),
        ("Play", TransportAction::Play),
        ("Stop", TransportAction::Stop),
        ("Cue", TransportAction::Cue),
        ("E-Stop", TransportAction::Estop),
    ]
}

fn label_at_word_boundary(text: &str, label: &str) -> bool {
    let bytes = text.as_bytes();
    let mut start = 0;
    while let Some(rel) = text[start..].find(label) {
        let abs = start + rel;
        let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
        let end = abs + label.len();
        let after_ok = end >= bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Extract transport actions named in a guidance string (word-boundary aware).
/// Longer labels are matched first and masked so `"Stop"` does not fire inside `"E-Stop"`.
pub fn actions_named_in_text(text: &str) -> Vec<TransportAction> {
    let mut remaining = text.to_string();
    let mut found = Vec::new();
    for (label, action) in guidance_action_labels() {
        if label_at_word_boundary(&remaining, label) && !found.contains(action) {
            found.push(*action);
            remaining = remaining.replace(label, &" ".repeat(label.len()));
        }
    }
    found
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

    fn snap_for_phase(phase: TransportPhase) -> TransportSnapshot {
        let dirty = phase == TransportPhase::Editing;
        TransportSnapshot {
            phase,
            mode: match phase {
                TransportPhase::PreviewPlaying | TransportPhase::PreviewPaused => {
                    TransportMode::Preview
                }
                TransportPhase::Prepared
                | TransportPhase::Armed
                | TransportPhase::LivePlaying
                | TransportPhase::LivePaused => TransportMode::Live,
                _ => TransportMode::Idle,
            },
            dirty,
            committed: !dirty,
            playhead_frames: 10,
            song_id: Some("s".into()),
            hardware_live: phase == TransportPhase::LivePlaying,
        }
    }

    #[test]
    fn play_rejected_while_editing() {
        let snap = editing();
        let g = TransportPolicy::guidance(&snap);
        assert!(!g.buttons.play);
        assert!(g
            .play_blocked_reason
            .as_deref()
            .unwrap()
            .contains("Save/Commit"));
        assert!(TransportPolicy::can(&snap, TransportAction::Play).is_err());
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
    fn preview_paused_can_prepare_for_live() {
        let mut snap = committed_idle();
        snap = TransportPolicy::apply(&snap, TransportAction::Preview).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Stop).unwrap();
        assert_eq!(snap.phase, TransportPhase::PreviewPaused);
        let g = TransportPolicy::guidance(&snap);
        assert!(g.buttons.prepare, "Prepare must be enabled from PreviewPaused");
        assert!(g.buttons.preview);
        assert!(g.buttons.cue);
        snap = TransportPolicy::apply(&snap, TransportAction::Prepare).unwrap();
        assert_eq!(snap.phase, TransportPhase::Prepared);
        assert!(!snap.hardware_live);
    }

    #[test]
    fn preview_paused_cue_returns_to_idle() {
        let mut snap = committed_idle();
        snap = TransportPolicy::apply(&snap, TransportAction::Preview).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Stop).unwrap();
        snap.playhead_frames = 99;
        snap = TransportPolicy::apply(&snap, TransportAction::Cue).unwrap();
        assert_eq!(snap.playhead_frames, 0);
        assert_eq!(snap.phase, TransportPhase::Idle);
        assert_eq!(snap.mode, TransportMode::Idle);
    }

    #[test]
    fn disarm_from_armed_to_idle() {
        let mut snap = committed_idle();
        snap = TransportPolicy::apply(&snap, TransportAction::Prepare).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Arm).unwrap();
        let g = TransportPolicy::guidance(&snap);
        assert!(g.buttons.disarm);
        assert!(
            g.preview_blocked_reason
                .as_deref()
                .unwrap()
                .contains("Disarm")
        );
        snap = TransportPolicy::apply(&snap, TransportAction::Disarm).unwrap();
        assert_eq!(snap.phase, TransportPhase::Idle);
        assert!(!snap.hardware_live);
        assert!(TransportPolicy::guidance(&snap).buttons.preview);
    }

    #[test]
    fn disarm_from_live_paused_to_idle() {
        let mut snap = committed_idle();
        snap = TransportPolicy::apply(&snap, TransportAction::Prepare).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Arm).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Play).unwrap();
        snap = TransportPolicy::apply(&snap, TransportAction::Stop).unwrap();
        assert_eq!(snap.phase, TransportPhase::LivePaused);
        assert!(TransportPolicy::guidance(&snap).buttons.disarm);
        snap = TransportPolicy::apply(&snap, TransportAction::Disarm).unwrap();
        assert_eq!(snap.phase, TransportPhase::Idle);
        assert!(!snap.hardware_live);
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
        for phase in phases {
            let snap = snap_for_phase(phase);
            let g = TransportPolicy::guidance(&snap);
            for action in TransportAction::all() {
                let allowed = g.buttons.enabled(*action);
                let result = TransportPolicy::apply(&snap, *action);
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
        let snaps = [
            editing(),
            committed_idle(),
            TransportPolicy::apply(&committed_idle(), TransportAction::Prepare).unwrap(),
            {
                let s =
                    TransportPolicy::apply(&committed_idle(), TransportAction::Prepare).unwrap();
                TransportPolicy::apply(&s, TransportAction::Arm).unwrap()
            },
            {
                let mut s =
                    TransportPolicy::apply(&committed_idle(), TransportAction::Preview).unwrap();
                s = TransportPolicy::apply(&s, TransportAction::Stop).unwrap();
                s
            },
            {
                let mut s =
                    TransportPolicy::apply(&committed_idle(), TransportAction::Prepare).unwrap();
                s = TransportPolicy::apply(&s, TransportAction::Arm).unwrap();
                s = TransportPolicy::apply(&s, TransportAction::Play).unwrap();
                TransportPolicy::apply(&s, TransportAction::Stop).unwrap()
            },
        ];
        for snap in snaps {
            let g = TransportPolicy::guidance(&snap);
            for action in TransportAction::all() {
                assert_eq!(
                    g.buttons.enabled(*action),
                    TransportPolicy::can(&snap, *action).is_ok(),
                    "UI/server mismatch for {action:?} in {:?}",
                    snap.phase
                );
            }
        }
    }

    #[test]
    fn guidance_never_names_disabled_actions() {
        // For every phase: any action named in next_hint or *_blocked_reason
        // must itself be enabled in that phase (no dead-end guidance).
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
        for phase in phases {
            let snap = snap_for_phase(phase);
            let g = TransportPolicy::guidance(&snap);
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
                        "phase={phase:?}: guidance names {:?} but it is disabled. text={text:?} hint buttons prepare={} disarm={} play={} preview={} stop={} save={}",
                        action,
                        g.buttons.prepare,
                        g.buttons.disarm,
                        g.buttons.play,
                        g.buttons.preview,
                        g.buttons.stop,
                        g.buttons.save_commit,
                    );
                }
            }
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
