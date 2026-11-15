//! Which control does what. The controller knows only roles; this map turns a
//! role into a hardware component number, and it is data: the PTZ Pro default
//! is built in, `panel.json` changes any part of it.

use crate::topology::{Input, Topology};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The pages of the panel. The name is what the status display shows.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Mode {
    Show,
    Transport,
    Camera,
    Presets,
    Timeline,
    Record,
    Media,
}

impl Mode {
    pub const ALL: [Mode; 7] =
        [Mode::Show, Mode::Transport, Mode::Camera, Mode::Presets, Mode::Timeline, Mode::Record, Mode::Media];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Show => "Show",
            Mode::Transport => "Transport",
            Mode::Camera => "Camera",
            Mode::Presets => "Presets",
            Mode::Timeline => "Timeline",
            Mode::Record => "Record",
            Mode::Media => "Media",
        }
    }

    fn key(self) -> &'static str {
        match self {
            Mode::Show => "mode_show",
            Mode::Transport => "mode_transport",
            Mode::Camera => "mode_camera",
            Mode::Presets => "mode_presets",
            Mode::Timeline => "mode_timeline",
            Mode::Record => "mode_record",
            Mode::Media => "mode_media",
        }
    }
}

pub const SOFT_KEYS: u8 = 7;
pub const SOFT_ENCODERS: u8 = 4;
pub const CAMERA_KEYS: u8 = 10;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Role {
    /// E-Stop. The same in every mode; never moved by a page or Shift.
    Estop,
    /// The primary transport action, as the server's guidance names it.
    Go,
    Shift,
    /// Soft keys 1 to 7.
    Key(u8),
    Mode(Mode),
    /// Camera select 1 to 10.
    Cam(u8),
    PageUp,
    PageDown,
    /// The playhead knob.
    Playhead,
    /// Soft encoders 1 to 4.
    Enc(u8),
    /// The display that says what a soft encoder does.
    EncDisplay(u8),
    JoyPan,
    JoyTilt,
    JoyZoom,
    /// A second zoom control, for panels with a rocker. The PTZ Pro has none.
    ZoomRocker,
    Status,
    Message,
    /// A soft key's display, on panels where it is a component of its own.
    KeyDisplay(u8),
    GoDisplay,
    EstopDisplay,
}

/// What kind of control a role needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Needs {
    Button,
    Encoder,
    Axis,
    Display,
}

impl Role {
    pub fn all() -> Vec<Role> {
        let mut out = vec![Role::Estop, Role::Go, Role::Shift];
        out.extend((1..=SOFT_KEYS).map(Role::Key));
        out.extend(Mode::ALL.map(Role::Mode));
        out.extend((1..=CAMERA_KEYS).map(Role::Cam));
        out.extend([Role::PageUp, Role::PageDown, Role::Playhead]);
        out.extend((1..=SOFT_ENCODERS).map(Role::Enc));
        out.extend((1..=SOFT_ENCODERS).map(Role::EncDisplay));
        out.extend([Role::JoyPan, Role::JoyTilt, Role::JoyZoom, Role::ZoomRocker, Role::Status, Role::Message]);
        out.extend((1..=SOFT_KEYS).map(Role::KeyDisplay));
        out.extend([Role::GoDisplay, Role::EstopDisplay]);
        out
    }

    /// The name used in `panel.json` and on the command line.
    pub fn key(self) -> String {
        match self {
            Role::Estop => "estop".into(),
            Role::Go => "go".into(),
            Role::Shift => "shift".into(),
            Role::Key(n) => format!("key{n}"),
            Role::Mode(m) => m.key().into(),
            Role::Cam(n) => format!("cam{n}"),
            Role::PageUp => "page_up".into(),
            Role::PageDown => "page_down".into(),
            Role::Playhead => "playhead".into(),
            Role::Enc(n) => format!("enc{n}"),
            Role::EncDisplay(n) => format!("enc{n}_display"),
            Role::JoyPan => "joy_pan".into(),
            Role::JoyTilt => "joy_tilt".into(),
            Role::JoyZoom => "joy_zoom".into(),
            Role::ZoomRocker => "zoom_rocker".into(),
            Role::Status => "status".into(),
            Role::Message => "message".into(),
            Role::KeyDisplay(n) => format!("key{n}_display"),
            Role::GoDisplay => "go_display".into(),
            Role::EstopDisplay => "estop_display".into(),
        }
    }

    pub fn from_key(key: &str) -> Option<Role> {
        Role::all().into_iter().find(|r| r.key() == key)
    }

    /// What it does, for the table and for `learn`.
    pub fn describe(self) -> String {
        match self {
            Role::Estop => "E-Stop, in every mode".into(),
            Role::Go => "Primary action: Rehearse, Play Live, Stop and so on".into(),
            Role::Shift => "Shift".into(),
            Role::Key(n) => format!("Soft key {n}"),
            Role::Mode(m) => format!("Mode: {}", m.name()),
            Role::Cam(n) => format!("Select camera {n}"),
            Role::PageUp => "Previous page of a list".into(),
            Role::PageDown => "Next page of a list".into(),
            Role::Playhead => "Playhead knob".into(),
            Role::Enc(n) => format!("Soft encoder {n}"),
            Role::EncDisplay(n) => format!("Display of soft encoder {n}"),
            Role::JoyPan => "Joystick left and right: pan".into(),
            Role::JoyTilt => "Joystick up and down: tilt".into(),
            Role::JoyZoom => "Joystick twist: zoom".into(),
            Role::ZoomRocker => "Zoom rocker".into(),
            Role::Status => "Status display".into(),
            Role::Message => "Message display".into(),
            Role::KeyDisplay(n) => format!("Display of soft key {n}"),
            Role::GoDisplay => "Display of the primary action".into(),
            Role::EstopDisplay => "Display of E-Stop".into(),
        }
    }

    pub fn needs(self) -> Needs {
        match self {
            Role::Estop
            | Role::Go
            | Role::Shift
            | Role::Key(_)
            | Role::Mode(_)
            | Role::Cam(_)
            | Role::PageUp
            | Role::PageDown => Needs::Button,
            Role::Playhead | Role::Enc(_) => Needs::Encoder,
            Role::JoyPan | Role::JoyTilt | Role::JoyZoom | Role::ZoomRocker => Needs::Axis,
            Role::EncDisplay(_) | Role::Status | Role::Message | Role::KeyDisplay(_) | Role::GoDisplay | Role::EstopDisplay => {
                Needs::Display
            }
        }
    }

    /// The show cannot be run from the panel without it.
    pub fn required(self) -> bool {
        !matches!(self, Role::ZoomRocker | Role::Cam(_) | Role::KeyDisplay(_) | Role::GoDisplay | Role::EstopDisplay)
    }

    /// The role that holds this button's display when the button has none of its own.
    pub fn display_role(self) -> Option<Role> {
        match self {
            Role::Key(n) => Some(Role::KeyDisplay(n)),
            Role::Go => Some(Role::GoDisplay),
            Role::Estop => Some(Role::EstopDisplay),
            Role::Enc(n) => Some(Role::EncDisplay(n)),
            _ => None,
        }
    }

    /// Its text must be readable for the panel to be usable.
    fn needs_text(self) -> bool {
        matches!(self, Role::Key(_) | Role::Go | Role::Estop)
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct PanelMap {
    roles: BTreeMap<Role, u32>,
    /// Axes whose sign is turned round.
    invert: BTreeSet<Role>,
}

/// Something about the map that will not work on the panel it is used with.
#[derive(Clone, PartialEq, Debug)]
pub struct Problem {
    pub role: Role,
    pub text: String,
}

impl PanelMap {
    /// The SKAARHOJ PTZ Pro, by the component numbers of its published topology.
    pub fn ptz_pro() -> PanelMap {
        let mut roles = BTreeMap::new();
        // CAM 1 to CAM 7 are the soft keys, CAM 8 the primary action, CAM 9 E-Stop.
        for n in 1..=SOFT_KEYS {
            roles.insert(Role::Key(n), 5 + n as u32);
        }
        roles.insert(Role::Go, 13);
        roles.insert(Role::Estop, 56);
        // EXP, WB, Color, Image, U1, U2, U3 are the modes; U4 is Shift.
        for (i, mode) in Mode::ALL.into_iter().enumerate() {
            roles.insert(Role::Mode(mode), 27 + i as u32);
        }
        roles.insert(Role::Shift, 34);
        // P1 to P10 select cameras; UP and DWN page.
        for n in 1..=CAMERA_KEYS {
            roles.insert(Role::Cam(n), 38 + n as u32);
        }
        roles.insert(Role::PageUp, 49);
        roles.insert(Role::PageDown, 50);
        // Menu1 to Menu4 under the bottom row of the big display: Q3A, Q3B, Q4A, Q4B.
        for n in 1..=SOFT_ENCODERS {
            roles.insert(Role::Enc(n), 34 + n as u32);
            roles.insert(Role::EncDisplay(n), 22 + n as u32);
        }
        roles.insert(Role::Playhead, 5);
        roles.insert(Role::JoyPan, 1);
        roles.insert(Role::JoyTilt, 2);
        roles.insert(Role::JoyZoom, 3);
        // The top row of the big display as two wide tiles: Q1 and Q2.
        roles.insert(Role::Status, 15);
        roles.insert(Role::Message, 16);
        PanelMap { roles, invert: BTreeSet::new() }
    }

    pub fn hwc(&self, role: Role) -> Option<u32> {
        self.roles.get(&role).copied()
    }

    pub fn inverted(&self, role: Role) -> bool {
        self.invert.contains(&role)
    }

    pub fn bind(&mut self, role: Role, hwc: Option<u32>) {
        match hwc {
            Some(hwc) => self.roles.insert(role, hwc),
            None => self.roles.remove(&role),
        };
    }

    /// The control role that a component number plays, if any.
    pub fn role_of(&self, hwc: u32) -> Option<Role> {
        self.roles.iter().find(|(r, h)| **h == hwc && r.needs() != Needs::Display).map(|(r, _)| *r)
    }

    /// Where a role's text goes: its display role when bound, otherwise the control itself.
    pub fn text_hwc(&self, role: Role) -> Option<u32> {
        if role.needs() == Needs::Display {
            return self.hwc(role);
        }
        role.display_role().and_then(|d| self.hwc(d)).or_else(|| if role.needs_text() { self.hwc(role) } else { None })
    }

    /// Every component number the map writes to or listens on.
    pub fn used(&self) -> BTreeSet<u32> {
        self.roles.values().copied().collect()
    }

    /// Apply `panel.json` on top of this map: `{"roles": {"estop": 50, "page_up": null}, "invert": ["joy_tilt"]}`.
    pub fn apply_json(&mut self, json: &str) -> Result<(), String> {
        let value: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("panel.json is not valid JSON: {e}"))?;
        let Some(top) = value.as_object() else {
            return Err("panel.json must be an object with \"roles\" and, if needed, \"invert\"".into());
        };
        for key in top.keys() {
            if !["roles", "invert", "panel"].contains(&key.as_str()) {
                return Err(format!("panel.json: \"{key}\" is not known. Use \"roles\" and \"invert\""));
            }
        }
        if let Some(roles) = top.get("roles") {
            let Some(roles) = roles.as_object() else {
                return Err("panel.json: \"roles\" must be an object of role to component number".into());
            };
            for (key, hwc) in roles {
                let role = Role::from_key(key)
                    .ok_or_else(|| format!("panel.json: there is no role \"{key}\". Run ptz-panel map for the list"))?;
                match hwc {
                    serde_json::Value::Null => self.bind(role, None),
                    serde_json::Value::Number(n) => match n.as_u64().filter(|n| (1..=u32::MAX as u64).contains(n)) {
                        Some(n) => self.bind(role, Some(n as u32)),
                        None => return Err(format!("panel.json: \"{key}\" must be a component number from 1, or null")),
                    },
                    _ => return Err(format!("panel.json: \"{key}\" must be a component number, or null to leave it unbound")),
                }
            }
        }
        if let Some(invert) = top.get("invert") {
            let Some(list) = invert.as_array() else {
                return Err("panel.json: \"invert\" must be a list of axis roles".into());
            };
            self.invert.clear();
            for item in list {
                let role = item.as_str().and_then(Role::from_key).filter(|r| r.needs() == Needs::Axis);
                match role {
                    Some(role) => self.invert.insert(role),
                    None => {
                        return Err(format!(
                            "panel.json: {item} is not an axis role. Axes are joy_pan, joy_tilt, joy_zoom, zoom_rocker"
                        ))
                    }
                };
            }
        }
        Ok(())
    }

    /// The PTZ Pro default with the file's changes, or the default alone when there is no file.
    pub fn load(path: &Path) -> Result<PanelMap, String> {
        let mut map = PanelMap::ptz_pro();
        match std::fs::read_to_string(path) {
            Ok(text) => map.apply_json(&text).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{} could not be read: {e}", path.display())),
        }
        Ok(map)
    }

    /// The whole map as `panel.json`, every role listed.
    pub fn to_json(&self) -> String {
        let roles: serde_json::Map<String, serde_json::Value> =
            Role::all().into_iter().map(|r| (r.key(), self.hwc(r).map(serde_json::Value::from).unwrap_or_default())).collect();
        let invert: Vec<String> = self.invert.iter().map(|r| r.key()).collect();
        let mut text = serde_json::to_string_pretty(&serde_json::json!({ "roles": roles, "invert": invert })).unwrap_or_default();
        text.push('\n');
        text
    }

    /// What will not work. With a topology, each bound control is also checked
    /// to be on the panel and of the right kind.
    pub fn problems(&self, topology: Option<&Topology>) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut say = |role: Role, text: String| out.push(Problem { role, text });
        for role in Role::all() {
            let Some(hwc) = self.hwc(role) else {
                if role.required() {
                    say(role, format!("{} ({}) is not bound to any control", role.key(), role.describe()));
                }
                continue;
            };
            let Some(topology) = topology else { continue };
            let Some(c) = topology.get(hwc) else {
                say(role, format!("{} is bound to component {hwc}, which this panel does not have", role.key()));
                continue;
            };
            let fits = match role.needs() {
                Needs::Button => matches!(c.input, Input::Button | Input::Encoder { push: true }),
                Needs::Encoder => matches!(c.input, Input::Encoder { .. }),
                Needs::Axis => matches!(c.input, Input::Intensity | Input::Absolute),
                Needs::Display => c.display.is_some(),
            };
            if !fits {
                let wants = match role.needs() {
                    Needs::Button => "a button",
                    Needs::Encoder => "an encoder",
                    Needs::Axis => "a joystick axis or fader",
                    Needs::Display => "a display",
                };
                say(role, {
                    let is = c.input.word();
                    let article = if is.starts_with(['a', 'e', 'i', 'o', 'u']) { "an" } else { "a" };
                    format!("{} needs {wants}, but component {hwc} ({}) is {article} {is}", role.key(), c.label)
                });
            }
        }
        if let Some(topology) = topology {
            for role in Role::all().into_iter().filter(|r| r.needs_text() && self.hwc(*r).is_some()) {
                let shown = self.text_hwc(role).and_then(|h| topology.get(h)).is_some_and(|c| c.display.is_some());
                if !shown {
                    say(
                        role,
                        format!(
                            "{} has no display, so it cannot say what it does. Bind {}",
                            role.key(),
                            role.display_role().map(|d| d.key()).unwrap_or_default()
                        ),
                    );
                }
            }
        }
        let mut seen: BTreeMap<(bool, u32), Role> = BTreeMap::new();
        for (role, hwc) in &self.roles {
            let display = role.needs() == Needs::Display;
            if let Some(first) = seen.insert((display, *hwc), *role) {
                say(*role, format!("{} and {} are both bound to component {hwc}", first.key(), role.key()));
            }
        }
        out
    }

    /// The map as a table. Labels and kinds come from `topology`.
    pub fn table(&self, topology: Option<&Topology>) -> String {
        let mut rows = vec![["Role".to_string(), "Does".into(), "HWC".into(), "On the panel".into(), "Kind".into()]];
        for role in Role::all() {
            let hwc = self.hwc(role);
            if hwc.is_none() && !role.required() && !matches!(role, Role::Cam(_) | Role::ZoomRocker) {
                continue;
            }
            let c = hwc.and_then(|h| topology.and_then(|t| t.get(h)));
            let mut does = role.describe();
            if self.inverted(role) {
                does.push_str(" (inverted)");
            }
            rows.push([
                role.key(),
                does,
                hwc.map(|h| h.to_string()).unwrap_or_else(|| "-".into()),
                c.map(|c| c.label.clone()).unwrap_or_default(),
                c.map(|c| match c.display {
                    Some((w, h)) if c.input == Input::None => format!("display {w}x{h}"),
                    Some((w, h)) => format!("{}, display {w}x{h}", c.input.word()),
                    None => c.input.word().to_string(),
                })
                .unwrap_or_default(),
            ]);
        }
        let widths: Vec<usize> = (0..5).map(|i| rows.iter().map(|r| r[i].len()).max().unwrap_or(0)).collect();
        let mut out = String::new();
        for (n, row) in rows.iter().enumerate() {
            let line: Vec<String> = row.iter().enumerate().map(|(i, cell)| format!("{cell:<w$}", w = widths[i])).collect();
            out.push_str(line.join("  ").trim_end());
            out.push('\n');
            if n == 0 {
                out.push_str(&widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("  "));
                out.push('\n');
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::ptz_pro;

    #[test]
    fn the_ptz_pro_default_covers_every_role_and_fits_the_published_topology() {
        let map = PanelMap::ptz_pro();
        let topology = ptz_pro();
        assert_eq!(map.problems(Some(&topology)), Vec::new());
        for role in Role::all() {
            let optional = matches!(role, Role::ZoomRocker | Role::KeyDisplay(_) | Role::GoDisplay | Role::EstopDisplay);
            assert_eq!(map.hwc(role).is_some(), !optional, "{}", role.key());
        }
        // The fixed keys sit where the plan says: CAM 9, CAM 8, U4, and the modes on EXP to U3.
        let label = |role: Role| topology.get(map.hwc(role).unwrap()).unwrap().label.clone();
        assert_eq!(label(Role::Estop), "CAM 9");
        assert_eq!(label(Role::Go), "CAM 8");
        assert_eq!(label(Role::Shift), "U4");
        assert_eq!(label(Role::Key(1)), "CAM 1");
        assert_eq!(label(Role::Key(7)), "CAM 7");
        assert_eq!(label(Role::Mode(Mode::Show)), "EXP");
        assert_eq!(label(Role::Mode(Mode::Media)), "U3");
        assert_eq!(label(Role::Cam(1)), "P1");
        assert_eq!(label(Role::Cam(10)), "P10");
        assert_eq!(label(Role::PageUp), "UP");
        assert_eq!(label(Role::PageDown), "DWN");
        assert_eq!(label(Role::Playhead), "Focus");
        assert_eq!(label(Role::Enc(1)), "Menu1");
        assert_eq!(label(Role::EncDisplay(1)), "Q3A");
        assert_eq!(label(Role::EncDisplay(4)), "Q4B");
        assert_eq!(label(Role::JoyZoom), "Rotate");
        assert_eq!(label(Role::Status), "Q1");
        assert_eq!(label(Role::Message), "Q2");
        // Soft keys write their text to themselves on this panel.
        assert_eq!(map.text_hwc(Role::Key(3)), Some(8));
        assert_eq!(map.text_hwc(Role::Enc(2)), Some(24));
        assert_eq!(map.text_hwc(Role::Shift), None);
        assert_eq!(map.role_of(56), Some(Role::Estop));
        assert_eq!(map.role_of(23), None, "a display is not a control");
    }

    #[test]
    fn every_role_has_a_name_that_reads_back() {
        let all = Role::all();
        let names: BTreeSet<String> = all.iter().map(|r| r.key()).collect();
        assert_eq!(names.len(), all.len(), "role names are unique");
        for role in all {
            assert_eq!(Role::from_key(&role.key()), Some(role));
        }
        assert_eq!(Role::from_key("key8"), None);
    }

    #[test]
    fn panel_json_remaps_unbinds_and_inverts() {
        let mut map = PanelMap::ptz_pro();
        map.apply_json(r#"{"roles": {"estop": 50, "page_down": null, "zoom_rocker": 60}, "invert": ["joy_tilt"]}"#).unwrap();
        assert_eq!(map.hwc(Role::Estop), Some(50));
        assert_eq!(map.hwc(Role::PageDown), None);
        assert_eq!(map.hwc(Role::ZoomRocker), Some(60));
        assert!(map.inverted(Role::JoyTilt) && !map.inverted(Role::JoyPan));
        assert_eq!(map.hwc(Role::Key(1)), Some(6), "what the file does not name keeps its default");
        assert_eq!(map.role_of(50), Some(Role::Estop));

        let problems = map.problems(Some(&ptz_pro()));
        let texts: Vec<&str> = problems.iter().map(|p| p.text.as_str()).collect();
        assert!(texts.contains(&"page_down (Next page of a list) is not bound to any control"), "{texts:?}");
        assert!(texts.contains(&"zoom_rocker is bound to component 60, which this panel does not have"), "{texts:?}");
        assert!(texts.iter().any(|t| t.contains("estop has no display")), "{texts:?}");
        assert_eq!(problems.len(), 3, "{texts:?}");

        // Written out and read back, it is the same map.
        let mut again = PanelMap::ptz_pro();
        again.apply_json(&map.to_json()).unwrap();
        assert_eq!(again, map);
    }

    #[test]
    fn a_control_of_the_wrong_kind_or_used_twice_is_reported_by_name() {
        let mut map = PanelMap::ptz_pro();
        map.bind(Role::Key(1), Some(1));
        map.bind(Role::JoyPan, Some(6));
        map.bind(Role::Enc(1), Some(27));
        map.bind(Role::Status, Some(27));
        map.bind(Role::PageUp, Some(50));
        let texts: Vec<String> = map.problems(Some(&ptz_pro())).into_iter().map(|p| p.text).collect();
        for want in [
            "key1 needs a button, but component 1 (LR) is a joystick axis",
            "joy_pan needs a joystick axis or fader, but component 6 (CAM 1) is a button",
            "enc1 needs an encoder, but component 27 (EXP) is a button",
            "status needs a display, but component 27 (EXP) is a button",
            "page_up and page_down are both bound to component 50",
        ] {
            assert!(texts.iter().any(|t| t == want), "missing {want:?} in {texts:?}");
        }
    }

    #[test]
    fn a_bad_panel_json_says_what_is_wrong() {
        let bad = |json: &str| PanelMap::ptz_pro().apply_json(json).unwrap_err();
        assert!(bad("{").contains("not valid JSON"));
        assert!(bad(r#"{"roles": {"keyz": 4}}"#).contains("there is no role \"keyz\""));
        assert!(bad(r#"{"roles": {"key1": "six"}}"#).contains("must be a component number"));
        assert!(bad(r#"{"roles": {"key1": 0}}"#).contains("from 1"));
        assert!(bad(r#"{"invert": ["key1"]}"#).contains("is not an axis role"));
        assert!(bad(r#"{"rolls": {}}"#).contains("\"rolls\" is not known"));
    }

    #[test]
    fn the_table_names_every_bound_role_with_the_panels_own_label() {
        let table = PanelMap::ptz_pro().table(Some(&ptz_pro()));
        assert!(
            table
                .lines()
                .any(|l| l.starts_with("estop") && l.contains("56") && l.contains("CAM 9") && l.contains("display 64x32")),
            "{table}"
        );
        assert!(
            table.lines().any(|l| l.starts_with("joy_zoom") && l.contains("Rotate") && l.contains("joystick axis")),
            "{table}"
        );
        assert!(table.lines().any(|l| l.starts_with("status") && l.contains("Q1") && l.contains("display 128x32")), "{table}");
        assert!(table.lines().any(|l| l.starts_with("zoom_rocker") && l.ends_with('-')), "{table}");
    }
}
