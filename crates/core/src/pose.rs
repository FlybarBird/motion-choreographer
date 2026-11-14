//! Axes, poses and project settings. Values are real units; hardware counts
//! appear only inside adapters.

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
    /// mounting). Elevation defaults to a single AutoPod column.
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
        format!(
            "when {} is {} {}, {} must be {} {}",
            self.when_axis.label().to_lowercase(),
            self.when.word(),
            self.when_axis.format(self.when_value),
            self.then_axis.label().to_lowercase(),
            self.must_be.word(),
            self.then_axis.format(self.must_value),
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

/// One measured point of the AutoPod calibration map.
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
    /// `host` or `host:port` of the AutoPod Plus leg box. Empty means no pedestal.
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

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Project {
    pub rate: FrameRate,
    pub routing: Routing,
    pub axes: AxisTable,
    pub keep_out: Vec<KeepOutRule>,
    pub freewheel_frames: u32,
    /// LTC generator level, 0 to 1.
    pub ltc_level: f32,
    pub inputs: InputConfig,
    pub rig: RigConfig,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            rate: FrameRate::F25,
            routing: Routing::default(),
            axes: AxisTable::default(),
            keep_out: Vec::new(),
            freewheel_frames: 8,
            ltc_level: 0.5,
            inputs: InputConfig::default(),
            rig: RigConfig::default(),
        }
    }
}

/// A cached camera pose that can be dropped on the timeline.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub pose: Pose,
    /// Camera preset number when this came from an FR7.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_number: Option<u32>,
    #[serde(default)]
    pub has_thumbnail: bool,
}
