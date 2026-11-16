//! Axes, poses and project settings. Values are real units; hardware counts
//! appear only inside adapters.

use crate::camera::{first_camera, CameraConfig, OnFault, Stage, MAX_CAMERAS};
use crate::time::FrameRate;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Pan,
    Tilt,
    Zoom,
    Focus,
    Elevation,
}

impl Axis {
    pub const ALL: [Axis; 5] = [Axis::Pan, Axis::Tilt, Axis::Zoom, Axis::Focus, Axis::Elevation];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Axis::Pan => "Pan",
            Axis::Tilt => "Tilt",
            Axis::Zoom => "Zoom",
            Axis::Focus => "Focus",
            Axis::Elevation => "Elevation",
        }
    }

    pub fn unit(self) -> &'static str {
        match self {
            Axis::Pan | Axis::Tilt => "°",
            Axis::Zoom | Axis::Focus => "",
            Axis::Elevation => "mm",
        }
    }

    pub fn format(self, v: f64) -> String {
        match self {
            Axis::Pan | Axis::Tilt => format!("{v:.1}°"),
            Axis::Zoom | Axis::Focus => format!("{:.0}%", v * 100.0),
            Axis::Elevation => format!("{v:.0} mm"),
        }
    }

    /// `unit`, in the units the show is read in: elevation is the only axis that is a length.
    pub fn unit_in(self, units: Units) -> &'static str {
        match self {
            Axis::Elevation => units.short(),
            _ => self.unit(),
        }
    }

    /// `format`, in the units the show is read in.
    pub fn format_in(self, v: f64, units: Units) -> String {
        match self {
            Axis::Elevation => units.height(v),
            _ => self.format(v),
        }
    }
}

/// The units lengths are shown and typed in. The show itself is always kept in millimetres
/// and metres, so changing this changes no pose, limit or position.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    #[default]
    Metric,
    Imperial,
}

impl Units {
    pub const ALL: [Units; 2] = [Units::Metric, Units::Imperial];

    pub fn label(self) -> &'static str {
        match self {
            Units::Metric => "Metric: millimetres and metres",
            Units::Imperial => "Imperial: inches and feet",
        }
    }

    /// The unit of heights: the column, the lens above the floor.
    pub fn short(self) -> &'static str {
        match self {
            Units::Metric => "mm",
            Units::Imperial => "in",
        }
    }

    /// The unit of distances on the stage.
    pub fn long(self) -> &'static str {
        match self {
            Units::Metric => "m",
            Units::Imperial => "ft",
        }
    }

    /// Millimetres in one of the short unit.
    pub fn mm_per_short(self) -> f64 {
        match self {
            Units::Metric => 1.0,
            Units::Imperial => 25.4,
        }
    }

    /// Metres in one of the long unit.
    pub fn m_per_long(self) -> f64 {
        match self {
            Units::Metric => 1.0,
            Units::Imperial => 0.3048,
        }
    }

    /// A height kept in millimetres, as text: "560 mm" or "22.0 in".
    pub fn height(self, mm: f64) -> String {
        match self {
            Units::Metric => format!("{mm:.0} mm"),
            Units::Imperial => format!("{:.1} in", mm / 25.4),
        }
    }

    /// A distance kept in metres, as text with that many decimals: "3.25 m" or "10.66 ft".
    pub fn distance(self, m: f64, decimals: usize) -> String {
        format!("{:.decimals$} {}", m / self.m_per_long(), self.long())
    }
}

/// Pan and tilt in degrees (pan positive is counter-clockwise seen from above,
/// tilt positive is up), zoom 0 wide to 1 tele, focus 0 far to 1 near,
/// elevation in millimetres above the lowest calibrated height.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Default)]
pub struct Pose {
    pub pan: f64,
    pub tilt: f64,
    pub zoom: f64,
    pub focus: f64,
    pub elevation: f64,
}

impl Pose {
    pub fn get(&self, axis: Axis) -> f64 {
        match axis {
            Axis::Pan => self.pan,
            Axis::Tilt => self.tilt,
            Axis::Zoom => self.zoom,
            Axis::Focus => self.focus,
            Axis::Elevation => self.elevation,
        }
    }

    pub fn set(&mut self, axis: Axis, v: f64) {
        match axis {
            Axis::Pan => self.pan = v,
            Axis::Tilt => self.tilt = v,
            Axis::Zoom => self.zoom = v,
            Axis::Focus => self.focus = v,
            Axis::Elevation => self.elevation = v,
        }
    }

    pub fn from_fn(mut f: impl FnMut(Axis) -> f64) -> Pose {
        let mut p = Pose::default();
        for a in Axis::ALL {
            p.set(a, f(a));
        }
        p
    }

    pub fn home() -> Pose {
        Pose { pan: 0.0, tilt: 0.0, zoom: 0.0, focus: 0.5, elevation: 0.0 }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct AxisLimits {
    pub min: f64,
    pub max: f64,
    /// Units per second.
    pub max_speed: f64,
    /// Units per second squared.
    pub max_accel: f64,
    pub settle_s: f64,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct AxisTable {
    pub pan: AxisLimits,
    pub tilt: AxisLimits,
    pub zoom: AxisLimits,
    pub focus: AxisLimits,
    pub elevation: AxisLimits,
}

impl AxisTable {
    pub fn get(&self, axis: Axis) -> &AxisLimits {
        match axis {
            Axis::Pan => &self.pan,
            Axis::Tilt => &self.tilt,
            Axis::Zoom => &self.zoom,
            Axis::Focus => &self.focus,
            Axis::Elevation => &self.elevation,
        }
    }

    pub fn get_mut(&mut self, axis: Axis) -> &mut AxisLimits {
        match axis {
            Axis::Pan => &mut self.pan,
            Axis::Tilt => &mut self.tilt,
            Axis::Zoom => &mut self.zoom,
            Axis::Focus => &mut self.focus,
            Axis::Elevation => &mut self.elevation,
        }
    }
}

impl AxisTable {
    /// The first axis of `pose` outside its range, as operator text.
    pub fn out_of_range(&self, pose: &Pose) -> Option<String> {
        Axis::ALL.into_iter().find_map(|a| {
            let (lim, v) = (self.get(a), pose.get(a));
            (v < lim.min - 1e-6 || v > lim.max + 1e-6)
                .then(|| format!("{} is at {}, outside {} to {}", a.label(), a.format(v), a.format(lim.min), a.format(lim.max)))
        })
    }
}

impl Default for AxisTable {
    /// FR7 pan/tilt range and top speed from Sony's command list (upright
    /// mounting). Elevation defaults to a single AutoPod+ column.
    fn default() -> Self {
        AxisTable {
            pan: AxisLimits { min: -170.0, max: 170.0, max_speed: 60.0, max_accel: 240.0, settle_s: 0.2 },
            tilt: AxisLimits { min: -30.0, max: 195.0, max_speed: 60.0, max_accel: 240.0, settle_s: 0.2 },
            zoom: AxisLimits { min: 0.0, max: 1.0, max_speed: 0.25, max_accel: 1.5, settle_s: 0.1 },
            focus: AxisLimits { min: 0.0, max: 1.0, max_speed: 0.5, max_accel: 3.0, settle_s: 0.1 },
            elevation: AxisLimits { min: 0.0, max: 670.0, max_speed: 43.0, max_accel: 120.0, settle_s: 0.3 },
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChannelRoute {
    Program,
    Ltc,
    #[default]
    Silent,
}

impl ChannelRoute {
    pub const ALL: [ChannelRoute; 3] = [ChannelRoute::Program, ChannelRoute::Ltc, ChannelRoute::Silent];

    pub fn label(self) -> &'static str {
        match self {
            ChannelRoute::Program => "Program",
            ChannelRoute::Ltc => "LTC generator",
            ChannelRoute::Silent => "Silent",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Routing {
    pub left: ChannelRoute,
    pub right: ChannelRoute,
}

impl Default for Routing {
    fn default() -> Self {
        Routing { left: ChannelRoute::Program, right: ChannelRoute::Silent }
    }
}

impl Routing {
    /// Program on both outputs, where it was on one and the other was silent. An output that
    /// carries generated timecode keeps it, and two silent outputs stay silent.
    pub fn mirrored(self) -> Routing {
        use ChannelRoute::{Program, Silent};
        match (self.left, self.right) {
            (Program, Silent) | (Silent, Program) => Routing { left: Program, right: Program },
            _ => self,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Cmp {
    Below,
    Above,
}

impl Cmp {
    pub fn holds(self, value: f64, bound: f64) -> bool {
        match self {
            Cmp::Below => value < bound,
            Cmp::Above => value > bound,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Cmp::Below => "below",
            Cmp::Above => "above",
        }
    }
}

/// "When elevation is below 200 mm, tilt must be above -10 degrees."
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct KeepOutRule {
    pub name: String,
    pub when_axis: Axis,
    pub when: Cmp,
    pub when_value: f64,
    pub then_axis: Axis,
    pub must_be: Cmp,
    pub must_value: f64,
}

impl KeepOutRule {
    /// "When elevation is below 200 mm, tilt must be above -10 degrees": the usual first rule.
    pub fn example() -> KeepOutRule {
        KeepOutRule {
            name: "Rule".into(),
            when_axis: Axis::Elevation,
            when: Cmp::Below,
            when_value: 200.0,
            then_axis: Axis::Tilt,
            must_be: Cmp::Above,
            must_value: -10.0,
        }
    }

    pub fn violated(&self, pose: &Pose) -> bool {
        self.when.holds(pose.get(self.when_axis), self.when_value)
            && !self.must_be.holds(pose.get(self.then_axis), self.must_value)
    }

    pub fn describe(&self) -> String {
        self.describe_in(Units::Metric)
    }

    /// `describe`, in the units the show is read in.
    pub fn describe_in(&self, units: Units) -> String {
        format!(
            "when {} is {} {}, {} must be {} {}",
            self.when_axis.label().to_lowercase(),
            self.when.word(),
            self.when_axis.format_in(self.when_value, units),
            self.then_axis.label().to_lowercase(),
            self.must_be.word(),
            self.then_axis.format_in(self.must_value, units),
        )
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum RigKind {
    #[default]
    Simulator,
    Hardware,
}

/// One measured point of the AutoPod+ calibration map.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct CalPoint {
    pub raw: f64,
    pub mm: f64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct RigConfig {
    pub kind: RigKind,
    /// `host` or `host:port` of the FR7. Empty means no camera head.
    pub fr7_host: String,
    /// Port of the FR7's web interface, which serves preset names and thumbnails.
    pub fr7_http_port: u16,
    pub fr7_user: String,
    pub fr7_password: String,
    /// Position counts per degree of pan and tilt.
    pub fr7_counts_per_degree: f64,
    /// Counts fall, not rise, as the head pans left.
    pub fr7_invert_pan: bool,
    /// Counts fall, not rise, as the head tilts up.
    pub fr7_invert_tilt: bool,
    /// Run a plain preset move as one absolute command and let the head ramp itself,
    /// instead of steering it along the curve.
    pub fr7_absolute_moves: bool,
    /// Degrees per second of each VISCA speed step, slowest first, as measured on the
    /// head by the bring-up tool. Empty uses an even spread between Sony's published ends.
    pub fr7_speed_steps: Vec<f64>,
    /// Age of a position reading when it arrives, in milliseconds. Zero uses the
    /// round trip the adapter measures as it runs.
    pub query_delay_ms: f64,
    /// `host` or `host:port` of the AutoPod+ leg box. Empty means no pedestal.
    pub autopod_host: String,
    pub autopod_calibration: Vec<CalPoint>,
    /// Allowed position error per axis before a tracking fault, in axis units.
    pub tolerance: Pose,
    /// How long the error may exceed tolerance, in milliseconds.
    pub tracking_window_ms: u64,
    /// Speed scale used while positioning to the playhead on Arm.
    pub positioning_speed: f64,
}

impl Default for RigConfig {
    fn default() -> Self {
        RigConfig {
            kind: RigKind::Simulator,
            fr7_host: String::new(),
            fr7_http_port: 80,
            fr7_user: String::new(),
            fr7_password: String::new(),
            fr7_counts_per_degree: 236.0,
            fr7_invert_pan: false,
            fr7_invert_tilt: false,
            fr7_absolute_moves: false,
            fr7_speed_steps: Vec::new(),
            query_delay_ms: 0.0,
            autopod_host: String::new(),
            autopod_calibration: Vec::new(),
            tolerance: Pose { pan: 2.0, tilt: 2.0, zoom: 0.05, focus: 0.08, elevation: 15.0 },
            tracking_window_ms: 500,
            positioning_speed: 0.5,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct InputConfig {
    /// Audio input device name for LTC. Empty selects the default input.
    pub ltc_device: String,
    pub ltc_channel: u32,
    /// MIDI input port name for MTC. Empty selects the first port.
    pub mtc_port: String,
    pub artnet_bind: String,
}

impl Default for InputConfig {
    fn default() -> Self {
        InputConfig { ltc_device: String::new(), ltc_channel: 0, mtc_port: String::new(), artnet_bind: "0.0.0.0:6454".into() }
    }
}

/// A hardware control panel the server runs as one more station of the show.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct PanelConfig {
    /// The server connects to the panel and runs the show from it as well.
    pub enabled: bool,
    /// The panel's address in Raw Panel mode: `192.168.0.60`, with `:port` when it is not the usual one.
    pub address: String,
}

impl PanelConfig {
    /// The address the server should connect to, when it should connect to one.
    pub fn wanted(&self) -> Option<&str> {
        let address = self.address.trim();
        (self.enabled && !address.is_empty()).then_some(address)
    }

    /// Why these settings cannot be used, as operator text.
    pub fn validate(&self) -> Result<(), String> {
        let address = self.address.trim();
        if self.enabled && address.is_empty() {
            return Err("Give the control panel's address before switching it on".into());
        }
        if address.len() > 255 || address.chars().any(|c| c.is_whitespace() || c.is_control() || c == '/') {
            return Err(format!("{address:?} is not a panel address. Use one like 192.168.0.60, or 192.168.0.60:9923"));
        }
        Ok(())
    }
}

/// When a pose captured while programming counts as a preset the camera already has.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct PresetMatch {
    /// A captured pose within range of a preset uses that preset instead of making another.
    pub enabled: bool,
    /// How far each axis may be from the preset's, in axis units, and still count as it.
    pub range: Pose,
}

impl Default for PresetMatch {
    fn default() -> Self {
        PresetMatch { enabled: true, range: Pose { pan: 1.0, tilt: 1.0, zoom: 0.02, focus: 0.05, elevation: 10.0 } }
    }
}

impl PresetMatch {
    /// How far `pose` is from `other`, as a share of the range on the axis that is furthest out.
    /// `None` when any axis is out of range.
    fn distance(&self, pose: &Pose, other: &Pose) -> Option<f64> {
        let mut worst: f64 = 0.0;
        for axis in Axis::ALL {
            let (off, range) = ((pose.get(axis) - other.get(axis)).abs(), self.range.get(axis));
            if off > range + 1e-9 {
                return None;
            }
            if range > 0.0 {
                worst = worst.max(off / range);
            }
        }
        Some(worst)
    }

    /// The preset of `camera` that `pose` counts as: the nearest one within range on every axis.
    pub fn nearest<'a>(&self, presets: &'a [Preset], camera: &str, pose: &Pose) -> Option<&'a Preset> {
        if !self.enabled {
            return None;
        }
        presets
            .iter()
            .filter(|p| p.camera == camera)
            .filter_map(|p| self.distance(pose, &p.pose).map(|d| (d, p)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, p)| p)
    }

    pub fn validate(&self) -> Result<(), String> {
        for axis in Axis::ALL {
            let range = self.range.get(axis);
            if !range.is_finite() || range < 0.0 {
                return Err(format!("The preset range for {} must be zero or more", axis.label().to_lowercase()));
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(from = "ProjectFile")]
pub struct Project {
    pub rate: FrameRate,
    pub routing: Routing,
    /// The audio output the server plays through, by name. Empty is the one it started with:
    /// the system's default, or the one named when it was started.
    pub audio_device: String,
    /// The AutoPod+ height map every camera reads its height through, unless the camera's
    /// rig has one of its own.
    pub autopod_calibration: Vec<CalPoint>,
    pub freewheel_frames: u32,
    /// LTC generator level, 0 to 1.
    pub ltc_level: f32,
    pub inputs: InputConfig,
    pub on_fault: OnFault,
    /// The units lengths are shown and typed in at every station.
    pub units: Units,
    /// The control panel the server runs, if it runs one.
    pub panel: PanelConfig,
    /// When a pose captured while programming uses a preset the camera already has.
    pub preset_match: PresetMatch,
    pub stage: Stage,
    /// The songs of the show in running order, by id. Stepping to the next or the one before,
    /// and following timecode from song to song, go by it.
    pub set_list: Vec<String>,
    /// Locked to LTC, MTC or Art-Net with Play pressed, load whichever song the code is in:
    /// one of the set list, or of every song while the set list is empty.
    pub follow_code: bool,
    /// One to twelve, in the order they are numbered for the operator. `validate` refuses
    /// anything else, and the server only ever holds a project that passed it.
    pub cameras: Vec<CameraConfig>,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            rate: FrameRate::F25,
            routing: Routing::default(),
            audio_device: String::new(),
            autopod_calibration: Vec::new(),
            freewheel_frames: 8,
            ltc_level: 0.5,
            inputs: InputConfig::default(),
            on_fault: OnFault::default(),
            units: Units::default(),
            panel: PanelConfig::default(),
            preset_match: PresetMatch::default(),
            stage: Stage::default(),
            set_list: Vec::new(),
            follow_code: false,
            cameras: vec![CameraConfig::default()],
        }
    }
}

/// `project.json` as any version wrote it. Before a project could hold several
/// cameras its one rig, limits and keep-out rules sat at the top level.
#[derive(Deserialize, Default)]
#[serde(default)]
struct ProjectFile {
    rate: Option<FrameRate>,
    routing: Option<Routing>,
    audio_device: String,
    autopod_calibration: Vec<CalPoint>,
    freewheel_frames: Option<u32>,
    ltc_level: Option<f32>,
    inputs: Option<InputConfig>,
    on_fault: OnFault,
    units: Units,
    panel: PanelConfig,
    preset_match: PresetMatch,
    stage: Stage,
    set_list: Vec<String>,
    follow_code: bool,
    /// Absent in a file from before cameras; an empty list is a mistake, kept so it can be refused.
    cameras: Option<Vec<CameraConfig>>,
    axes: Option<AxisTable>,
    keep_out: Option<Vec<KeepOutRule>>,
    rig: Option<RigConfig>,
}

impl From<ProjectFile> for Project {
    fn from(f: ProjectFile) -> Project {
        let d = Project::default();
        let mut cameras = f.cameras.unwrap_or_else(|| {
            vec![CameraConfig {
                axes: f.axes.unwrap_or_default(),
                keep_out: f.keep_out.unwrap_or_default(),
                rig: f.rig.unwrap_or_default(),
                ..CameraConfig::default()
            }]
        });
        // However a project arrives, an inverted camera has the tilt range of an inverted camera.
        cameras.iter_mut().for_each(CameraConfig::settle_tilt_range);
        Project {
            rate: f.rate.unwrap_or(d.rate),
            routing: f.routing.unwrap_or(d.routing),
            audio_device: f.audio_device,
            autopod_calibration: f.autopod_calibration,
            freewheel_frames: f.freewheel_frames.unwrap_or(d.freewheel_frames),
            ltc_level: f.ltc_level.unwrap_or(d.ltc_level),
            inputs: f.inputs.unwrap_or(d.inputs),
            on_fault: f.on_fault,
            units: f.units,
            panel: f.panel,
            preset_match: f.preset_match,
            stage: f.stage,
            set_list: f.set_list,
            follow_code: f.follow_code,
            cameras,
        }
    }
}

impl Project {
    /// The AutoPod+ height map a camera is read through: its own when it has one, the
    /// project's otherwise.
    pub fn height_map<'a>(&'a self, camera: &'a CameraConfig) -> &'a [CalPoint] {
        if camera.rig.autopod_calibration.is_empty() {
            &self.autopod_calibration
        } else {
            &camera.rig.autopod_calibration
        }
    }

    /// Lowest and highest height measured on this camera's column, in millimetres.
    pub fn measured_span(&self, camera: &CameraConfig) -> Option<(f64, f64)> {
        let map = self.height_map(camera);
        let measured = map.len() >= 2 && map.iter().any(|p| p.raw != map[0].raw);
        if !(camera.has_column() && measured) {
            return None;
        }
        let low = map.iter().map(|p| p.mm).fold(f64::INFINITY, f64::min);
        let high = map.iter().map(|p| p.mm).fold(f64::NEG_INFINITY, f64::max);
        (high - low > 1.0).then_some((low, high))
    }

    /// 0 is the lowest measured height and 100 the highest. A value already inside that
    /// travel is left as millimetres; one from 0 to 100 is a share of it.
    pub fn column_mm(&self, camera: &CameraConfig, elevation: f64) -> Option<f64> {
        let (low, high) = self.measured_span(camera)?;
        if elevation >= low - 50.0 {
            Some(elevation)
        } else if (0.0..=100.0).contains(&elevation) {
            Some(low + elevation / 100.0 * (high - low))
        } else {
            None
        }
    }

    /// The rig the server builds for a camera: the camera's own, with the project's height
    /// map where it has none.
    pub fn rig_of(&self, camera: &CameraConfig) -> RigConfig {
        let mut rig = camera.rig.clone();
        rig.autopod_calibration = self.height_map(camera).to_vec();
        rig
    }

    /// The limits a camera is held to. A column's height map says how its height is read, and
    /// its elevation limits say how far it may travel: they stand as they are set whenever the
    /// measured heights reach into them. Limits that every measured height lies beyond are on
    /// another scale than the map, as the untouched default is against heights taken from the
    /// floor. They say nothing about this column, and its travel is then from the lowest to
    /// the highest height that was measured.
    pub fn limits_of(&self, camera: &CameraConfig) -> AxisTable {
        let mut axes = camera.axes;
        let map = self.height_map(camera);
        let measured = map.iter().any(|p| p.raw != map[0].raw);
        if camera.has_column() && measured {
            let lowest = map.iter().map(|p| p.mm).fold(f64::INFINITY, f64::min);
            let highest = map.iter().map(|p| p.mm).fold(f64::NEG_INFINITY, f64::max);
            let set = &mut axes.elevation;
            if highest < set.min || lowest > set.max {
                (set.min, set.max) = (lowest, highest);
            }
        }
        axes
    }

    pub fn camera(&self, id: &str) -> Option<&CameraConfig> {
        self.cameras.iter().find(|c| c.id == id)
    }

    pub fn camera_mut(&mut self, id: &str) -> Option<&mut CameraConfig> {
        self.cameras.iter_mut().find(|c| c.id == id)
    }

    pub fn camera_index(&self, id: &str) -> Option<usize> {
        self.cameras.iter().position(|c| c.id == id)
    }

    /// The first camera. A project always has one.
    pub fn first(&self) -> &CameraConfig {
        &self.cameras[0]
    }

    pub fn first_mut(&mut self) -> &mut CameraConfig {
        &mut self.cameras[0]
    }

    /// Limits of a camera. A camera that is no longer in the project gets the defaults,
    /// so its clips can still be drawn and judged.
    pub fn axes_of(&self, id: &str) -> AxisTable {
        self.camera(id).map(|c| self.limits_of(c)).unwrap_or_default()
    }

    /// "3 Stage left": the camera's number in the list and its name.
    pub fn label(&self, id: &str) -> String {
        match self.camera_index(id) {
            Some(i) => format!("{} {}", i + 1, self.cameras[i].name),
            None => "Missing camera".into(),
        }
    }

    /// `"3 Stage left: "` in front of a message when there is more than one camera to tell apart.
    pub fn prefix(&self, id: &str) -> String {
        if self.cameras.len() > 1 {
            format!("{}: ", self.label(id))
        } else {
            String::new()
        }
    }

    /// Where a song stands in the set list, counting from 0.
    pub fn set_position(&self, song: &str) -> Option<usize> {
        self.set_list.iter().position(|id| id == song)
    }

    /// The song `by` places on from `song` in the set list: 1 is the next, -1 the one before.
    /// From a song that is not in the list, the first or the last. None past either end.
    pub fn set_neighbour(&self, song: Option<&str>, by: i64) -> Option<&str> {
        let at = match song.and_then(|id| self.set_position(id)) {
            Some(at) => at as i64 + by,
            None if by >= 0 => 0,
            None => self.set_list.len() as i64 - 1,
        };
        usize::try_from(at).ok().and_then(|at| self.set_list.get(at)).map(String::as_str)
    }

    /// The lowest `camN` no camera uses.
    pub fn free_camera_id(&self) -> String {
        (1..).map(|n| format!("cam{n}")).find(|id| self.camera(id).is_none()).unwrap_or_default()
    }

    /// Why this project cannot be used, as operator text.
    pub fn validate(&self) -> Result<(), String> {
        if self.cameras.is_empty() {
            return Err("A project needs at least one camera".into());
        }
        if self.cameras.len() > MAX_CAMERAS {
            return Err(format!("A project can hold {MAX_CAMERAS} cameras; this one has {}", self.cameras.len()));
        }
        self.panel.validate()?;
        self.preset_match.validate()?;
        for (i, cam) in self.cameras.iter().enumerate() {
            let label = format!("Camera {}", i + 1);
            let id_ok = !cam.id.is_empty() && cam.id.len() <= 32 && cam.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !id_ok {
                return Err(format!("{label} has an id that cannot be used"));
            }
            if self.cameras[..i].iter().any(|other| other.id == cam.id) {
                return Err(format!("{label} shares its id with another camera"));
            }
            if cam.name.trim().is_empty() {
                return Err(format!("{label} needs a name"));
            }
            let label = format!("{} {}", i + 1, cam.name.trim());
            for a in Axis::ALL {
                let l = cam.axes.get(a);
                let usable = l.min < l.max && l.max_speed > 0.0 && l.max_accel > 0.0 && l.settle_s >= 0.0;
                if !usable || ![l.min, l.max, l.max_speed, l.max_accel, l.settle_s].iter().all(|v| v.is_finite()) {
                    return Err(format!(
                        "{}{} limits are not usable: min must be below max, speeds above zero",
                        if self.cameras.len() > 1 { format!("{label}: ") } else { String::new() },
                        a.label()
                    ));
                }
            }
            cam.lens.validate().map_err(|why| format!("{label}: {why}"))?;
            cam.mount.validate().map_err(|why| format!("{label}: {why}"))?;
        }
        if let Some(model) = &self.stage.model {
            model.validate()?;
        }
        Ok(())
    }
}

/// A cached camera pose that can be dropped on the timeline.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub pose: Pose,
    /// The camera this pose was read on. A pose only means something on that rig.
    #[serde(default = "first_camera")]
    pub camera: String,
    /// Camera preset number when this came from an FR7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_number: Option<u32>,
    #[serde(default)]
    pub has_thumbnail: bool,
}

impl Preset {
    /// A preset of the first camera with no picture, as the simulator and the tests make them.
    pub fn new(id: &str, name: &str, pose: Pose) -> Preset {
        Preset { id: id.into(), name: name.into(), pose, camera: first_camera(), camera_number: None, has_thumbnail: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::FIRST_CAMERA;

    #[test]
    fn a_captured_pose_counts_as_the_nearest_preset_within_range_on_every_axis() {
        let rule = PresetMatch::default();
        let at = |pan: f64, tilt: f64| Pose { pan, tilt, ..Pose::home() };
        let preset = |id: &str, camera: &str, pose: Pose| Preset { camera: camera.into(), ..Preset::new(id, id, pose) };
        let presets = [
            preset("wide", "cam1", at(10.0, 0.0)),
            preset("close", "cam1", at(10.8, 0.0)),
            preset("other", "cam2", at(10.5, 0.0)),
        ];
        let found = |pose: Pose| rule.nearest(&presets, "cam1", &pose).map(|p| p.id.as_str());

        assert_eq!(found(at(10.1, 0.0)), Some("wide"));
        assert_eq!(found(at(10.5, 0.0)), Some("close"), "0.3 from close and 0.5 from wide: the nearer one, not another camera's");
        assert_eq!(found(at(9.0, 0.0)), Some("wide"), "exactly at the edge of the range still counts");
        assert_eq!(found(at(11.9, 0.0)), None);
        assert_eq!(found(at(10.0, 1.5)), None, "one axis out of range is enough");
        assert_eq!(found(Pose { zoom: 0.5, ..at(10.0, 0.0) }), None);
        assert_eq!(rule.nearest(&presets, "cam3", &at(10.0, 0.0)), None);

        // A range of zero asks for the same value; a wide one ignores the axis.
        let exact = PresetMatch { range: Pose { pan: 0.0, ..rule.range }, ..rule };
        assert_eq!(exact.nearest(&presets, "cam1", &at(10.0, 0.5)).map(|p| p.id.as_str()), Some("wide"));
        assert_eq!(exact.nearest(&presets, "cam1", &at(10.01, 0.0)), None);
        let any_focus = PresetMatch { range: Pose { focus: 1.0, ..rule.range }, ..rule };
        assert!(any_focus.nearest(&presets, "cam1", &Pose { focus: 0.9, ..at(10.0, 0.0) }).is_some());

        let off = PresetMatch { enabled: false, ..rule };
        assert_eq!(off.nearest(&presets, "cam1", &at(10.0, 0.0)), None);

        // An old project file has the rule with its defaults, and a bad range is refused.
        let project: Project = serde_json::from_str(OLD).unwrap();
        assert_eq!(project.preset_match, PresetMatch::default());
        let mut bad = Project::default();
        bad.preset_match.range.tilt = -1.0;
        assert_eq!(bad.validate().unwrap_err(), "The preset range for tilt must be zero or more");
        bad.preset_match.range.tilt = f64::NAN;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn mirroring_fills_a_silent_output_and_nothing_else() {
        use ChannelRoute::{Ltc, Program, Silent};
        let both = Routing { left: Program, right: Program };
        assert_eq!(Routing { left: Program, right: Silent }.mirrored(), both);
        assert_eq!(Routing { left: Silent, right: Program }.mirrored(), both);
        // Generated timecode keeps its output, and an output that was turned off on both sides stays off.
        for kept in [
            Routing { left: Program, right: Ltc },
            Routing { left: Ltc, right: Program },
            Routing { left: Silent, right: Silent },
            Routing { left: Silent, right: Ltc },
            both,
        ] {
            assert_eq!(kept.mirrored(), kept);
        }
    }

    /// `project.json` as stage 5 wrote it: one rig at the top level.
    const OLD: &str = r#"{
      "rate": "25", "routing": { "left": "program", "right": "ltc" },
      "axes": {
        "pan": { "min": -100, "max": 100, "max_speed": 40, "max_accel": 200, "settle_s": 0.2 },
        "tilt": { "min": -30, "max": 90, "max_speed": 60, "max_accel": 240, "settle_s": 0.2 },
        "zoom": { "min": 0, "max": 1, "max_speed": 0.25, "max_accel": 1.5, "settle_s": 0.1 },
        "focus": { "min": 0, "max": 1, "max_speed": 0.5, "max_accel": 3, "settle_s": 0.1 },
        "elevation": { "min": 0, "max": 500, "max_speed": 43, "max_accel": 120, "settle_s": 0.3 }
      },
      "keep_out": [{ "name": "Desk", "when_axis": "elevation", "when": "below", "when_value": 200,
                     "then_axis": "tilt", "must_be": "above", "must_value": -10 }],
      "freewheel_frames": 12, "ltc_level": 0.4,
      "inputs": { "ltc_device": "", "ltc_channel": 1, "mtc_port": "", "artnet_bind": "0.0.0.0:6454" },
      "rig": { "kind": "hardware", "fr7_host": "192.168.60.201", "autopod_host": "192.168.60.204" }
    }"#;

    #[test]
    fn a_one_rig_project_becomes_a_one_camera_project_with_nothing_lost() {
        let project: Project = serde_json::from_str(OLD).unwrap();
        assert_eq!(project.cameras.len(), 1);
        let cam = project.first();
        assert_eq!((cam.id.as_str(), cam.name.as_str()), (FIRST_CAMERA, "Camera 1"));
        assert_eq!((cam.axes.pan.max, cam.axes.pan.max_speed, cam.axes.elevation.max), (100.0, 40.0, 500.0));
        assert_eq!(cam.keep_out.len(), 1);
        assert_eq!(cam.keep_out[0].name, "Desk");
        assert_eq!((cam.rig.kind, cam.rig.fr7_host.as_str()), (RigKind::Hardware, "192.168.60.201"));
        assert_eq!(cam.rig.autopod_host, "192.168.60.204");
        assert_eq!((project.freewheel_frames, project.ltc_level, project.inputs.ltc_channel), (12, 0.4, 1));
        assert_eq!(project.routing.right, ChannelRoute::Ltc);
        assert_eq!(cam.lens, crate::lens::Lens::default());
        assert_eq!(cam.mount, crate::camera::Mount::default());
        assert!(project.stage.show_builtin && project.stage.model.is_none());
        assert!(project.validate().is_ok());

        // Written back, it is in the new shape and reads the same.
        let text = serde_json::to_string(&project).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(v.get("axes").is_none() && v.get("rig").is_none() && v["cameras"][0]["rig"]["fr7_host"] == "192.168.60.201");
        assert_eq!(serde_json::from_str::<Project>(&text).unwrap(), project);

        // An empty file is a default project.
        assert_eq!(serde_json::from_str::<Project>("{}").unwrap(), Project::default());
    }

    #[test]
    fn a_project_that_cannot_be_used_says_which_camera() {
        let mut p = Project::default();
        assert!(p.validate().is_ok());
        assert_eq!(p.label(FIRST_CAMERA), "1 Camera 1");
        assert_eq!(p.prefix(FIRST_CAMERA), "", "one camera needs no telling apart");
        for n in 2..=12 {
            let id = p.free_camera_id();
            assert_eq!(id, format!("cam{n}"));
            p.cameras.push(CameraConfig::new(&id, &format!("Camera {n}")));
        }
        assert!(p.validate().is_ok());
        assert_eq!(p.prefix("cam3"), "3 Camera 3: ");
        assert_eq!(p.label("gone"), "Missing camera");
        assert_eq!(p.axes_of("gone"), AxisTable::default());

        let mut thirteen = p.clone();
        thirteen.cameras.push(CameraConfig::new("cam13", "One too many"));
        assert_eq!(thirteen.validate().unwrap_err(), "A project can hold 12 cameras; this one has 13");
        let mut none = p.clone();
        none.cameras.clear();
        assert_eq!(none.validate().unwrap_err(), "A project needs at least one camera");

        let mut twin = p.clone();
        twin.cameras[4].id = "cam2".into();
        assert_eq!(twin.validate().unwrap_err(), "Camera 5 shares its id with another camera");
        let mut blank = p.clone();
        blank.cameras[2].name = "  ".into();
        assert_eq!(blank.validate().unwrap_err(), "Camera 3 needs a name");
        let mut odd = p.clone();
        odd.cameras[1].id = "cam 2/x".into();
        assert_eq!(odd.validate().unwrap_err(), "Camera 2 has an id that cannot be used");

        let mut limits = p.clone();
        limits.cameras[6].name = "Balcony".into();
        limits.cameras[6].axes.tilt.min = 200.0;
        assert_eq!(
            limits.validate().unwrap_err(),
            "7 Balcony: Tilt limits are not usable: min must be below max, speeds above zero"
        );
        let mut lens = p.clone();
        lens.cameras[0].lens.tele_mm = 10.0;
        assert_eq!(lens.validate().unwrap_err(), "1 Camera 1: the tele end cannot be shorter than the wide end");
        let mut mount = p.clone();
        mount.cameras[11].mount.base_height_mm = -1.0;
        assert_eq!(mount.validate().unwrap_err(), "12 Camera 12: the base height must be between 0 and 5000 mm");

        // Removing a camera frees its id for the next one added.
        p.cameras.remove(3);
        assert_eq!(p.free_camera_id(), "cam4");
    }

    #[test]
    fn a_preset_from_before_cameras_belongs_to_the_first() {
        let preset: Preset = serde_json::from_str(
            r#"{ "id": "wide", "name": "Wide", "pose": { "pan": 0, "tilt": 0, "zoom": 0, "focus": 0.5, "elevation": 0 } }"#,
        )
        .unwrap();
        assert_eq!(preset.camera, FIRST_CAMERA);
    }
}
