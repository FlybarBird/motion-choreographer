//! Camera focus modes and the geometry of FR7 Auto Follow.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FocusMode {
    #[default]
    ShowDefault,
    On,
    Off,
}

impl FocusMode {
    pub const ALL: [Self; 3] = [Self::ShowDefault, Self::On, Self::Off];

    pub fn label(self) -> &'static str {
        match self {
            Self::ShowDefault => "Use Show Default",
            Self::On => "On",
            Self::Off => "Off",
        }
    }

    pub fn enabled(self, show_default: bool) -> bool {
        match self {
            Self::ShowDefault => show_default,
            Self::On => true,
            Self::Off => false,
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::ShowDefault
    }
}

/// Coordinates in ten-thousandths of the preset frame, as used by the FR7.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct DetectionArea {
    pub left: u16,
    pub top: u16,
    pub width: u16,
    pub height: u16,
}

impl Default for DetectionArea {
    fn default() -> Self {
        Self { left: 0, top: 0, width: 10_000, height: 10_000 }
    }
}

impl DetectionArea {
    pub fn valid(self) -> bool {
        self.width >= 1000
            && self.height >= 2000
            && u32::from(self.left) + u32::from(self.width) <= 10_000
            && u32::from(self.top) + u32::from(self.height) <= 10_000
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LeadRoom {
    #[default]
    Off,
    Low,
    Medium,
    High,
}

impl LeadRoom {
    pub const ALL: [Self; 4] = [Self::Off, Self::Low, Self::Medium, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct FollowComposition {
    pub vertical: i8,
    pub horizontal: i8,
    /// Subject size in parts per thousand, in Sony's composition units.
    pub size: u16,
    pub lead_room: LeadRoom,
}

impl Default for FollowComposition {
    fn default() -> Self {
        Self { vertical: 0, horizontal: 0, size: 1200, lead_room: LeadRoom::Off }
    }
}

impl FollowComposition {
    pub fn valid(self) -> bool {
        (-50..=70).contains(&self.vertical) && (-50..=50).contains(&self.horizontal) && (250..=3500).contains(&self.size)
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard;
    use crate::compile;
    use crate::edit;
    use crate::pose::{Pose, Preset, Project};
    use crate::song::{ClipBody, PresetRef, Song};
    use crate::{CameraConfig, FIRST_CAMERA};

    #[test]
    fn autofocus_resolves_timed_preset_and_show_settings_per_camera() {
        let mut project = Project { default_autofocus: true, ..Project::default() };
        project.cameras.push(CameraConfig::new("cam2", "Second"));
        let mut preset = Preset::new("wide", "Wide", Pose::home());
        preset.autofocus = FocusMode::Off;
        let song = Song::new("s", "Focus", "01:00:00:00", 400);
        let (song, movement) = edit::insert_preset_move(&song, &project, &preset, 15).unwrap();
        let (song, _) =
            edit::insert_camera_control(&song, &project, FIRST_CAMERA, 50, ClipBody::Autofocus { enabled: true }).unwrap();
        let at = |song: &Song, camera, frame| song.autofocus_at(camera, frame, project.default_autofocus, false);
        assert!(at(&song, FIRST_CAMERA, 14));
        assert!(!at(&song, FIRST_CAMERA, 30), "the preset stays in effect through the held shot");
        assert!(at(&song, FIRST_CAMERA, 50));
        assert!(at(&song, FIRST_CAMERA, 99));
        assert!(!at(&song, FIRST_CAMERA, 100));
        assert!(at(&song, "cam2", 100));

        let song = edit::set_focus_override(&song, &project, &movement, Some(FocusMode::ShowDefault)).unwrap();
        let mut preset = preset.clone();
        preset.autofocus = FocusMode::On;
        let linked = edit::adopt_presets(&song, &project, &[preset]).unwrap().unwrap();
        assert!(at(&linked, FIRST_CAMERA, 100));
        assert!(!linked.autofocus_at(FIRST_CAMERA, 100, false, false), "the explicit override survives a library edit");
        let legacy: Project = serde_json::from_str(r#"{}"#).unwrap();
        assert!(!legacy.default_autofocus);
    }

    #[test]
    fn auto_follow_keeps_its_search_preset_in_copied_clips_and_flags_conflicting_focus() {
        let project = Project::default();
        let mut mark = Preset::new("mark", "Mark", Pose { pan: 8.0, ..Pose::home() });
        mark.autofocus = FocusMode::On;
        let song = Song::new("s", "Follow", "01:00:00:00", 500);
        let (song, id) = edit::insert_camera_control(
            &song,
            &project,
            FIRST_CAMERA,
            40,
            ClipBody::AutoFollow { preset: None, detection: Default::default() },
        )
        .unwrap();
        assert_eq!(compile::compile(&song, &project).blocking_count(&song), 1);
        let song = edit::set_to(&song, &project, &id, &mark).unwrap();
        assert_eq!(compile::compile(&song, &project).blocking_count(&song), 0);
        assert!(song.autofocus_at(FIRST_CAMERA, 40, false, true));
        assert!(!song.autofocus_at(FIRST_CAMERA, 165, false, true));

        let copy = clipboard::capture(&song, project.rate, std::slice::from_ref(&id)).unwrap();
        let json = copy.to_json().unwrap();
        let decoded = clipboard::Clipboard::from_json(&json).unwrap();
        assert_eq!(copy, decoded);
        assert!(json.contains("auto_follow"));
        assert!(json.contains(&PresetRef::of(&mark).preset_id));

        let (song, _) =
            edit::insert_camera_control(&song, &project, FIRST_CAMERA, 50, ClipBody::Autofocus { enabled: false }).unwrap();
        let issues = compile::compile(&song, &project).issues;
        assert!(issues.iter().any(|issue| issue.message().contains("Autofocus must stay in one mode")));
        assert!(edit::insert_preset_move(&song, &project, &mark, 50).is_err());
    }
}
