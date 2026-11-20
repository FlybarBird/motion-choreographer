//! The cameras of a project and the room they stand in. Each camera has its own
//! rig, limits, lens and place on the floor; the stage is the model they look at.

use crate::lens::Lens;
use crate::pose::{AxisTable, KeepOutRule, Pose, RigConfig};
use serde::{Deserialize, Serialize};

pub const MAX_CAMERAS: usize = 12;
/// The camera every song and preset belonged to before a project could hold several.
pub const FIRST_CAMERA: &str = "cam1";

pub fn first_camera() -> String {
    FIRST_CAMERA.to_string()
}

pub fn is_first_camera(id: &str) -> bool {
    id == FIRST_CAMERA
}

/// Where a camera is attached and which way it faces. The world is in metres,
/// Z up, with the floor at zero. Pose angles keep their world-normalized signs
/// for either mounting orientation; device count inversion belongs to the rig.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct Mount {
    pub x: f64,
    pub y: f64,
    /// Height of the attachment: floor or riser for an upright camera, ceiling or truss for an inverted one.
    pub z: f64,
    /// Direction of pan 0: 0 looks toward +Y, positive turns counter-clockwise seen from above.
    pub heading_deg: f64,
    /// World pitch at tilt 0: positive looks up.
    pub tilt_deg: f64,
    /// The camera hangs below its attachment. The camera's image flip is assumed
    /// to keep its output upright, as the simulated view is.
    pub inverted: bool,
    /// Distance from the attachment to the lens with the column at its lowest setting.
    pub base_height_mm: f64,
}

impl Default for Mount {
    fn default() -> Self {
        Mount { x: 0.0, y: 0.0, z: 0.0, heading_deg: 0.0, tilt_deg: 0.0, inverted: false, base_height_mm: 1200.0 }
    }
}

impl Mount {
    /// Where the lens is for a column extended by `elevation_mm` away from its attachment.
    pub fn lens_position(&self, elevation_mm: f64) -> [f64; 3] {
        let direction = if self.inverted { -1.0 } else { 1.0 };
        [self.x, self.y, self.z + direction * (self.base_height_mm + elevation_mm) / 1000.0]
    }

    /// Where the lens is when `height_mm` is its height above the floor, as a measured column gives it.
    pub fn lens_at_height(&self, height_mm: f64) -> [f64; 3] {
        let direction = if self.inverted { -1.0 } else { 1.0 };
        [self.x, self.y, self.z + direction * height_mm / 1000.0]
    }

    /// Yaw and pitch of the view in degrees for a pose: positive pan turns left, positive tilt looks up.
    pub fn view(&self, pose: &Pose) -> (f64, f64) {
        (self.heading_deg + pose.pan, self.tilt_deg + pose.tilt)
    }

    /// Pose pan and tilt that face a world yaw and pitch. Pan takes the shortest
    /// turn from the mount heading; callers still apply the camera's axis limits.
    pub fn pose_angles(&self, yaw_deg: f64, pitch_deg: f64) -> (f64, f64) {
        ((yaw_deg - self.heading_deg + 180.0).rem_euclid(360.0) - 180.0, pitch_deg - self.tilt_deg)
    }

    pub fn validate(&self) -> Result<(), String> {
        let all = [self.x, self.y, self.z, self.heading_deg, self.tilt_deg, self.base_height_mm];
        if all.iter().any(|v| !v.is_finite()) || [self.x, self.y, self.z].iter().any(|v| v.abs() > 10_000.0) {
            return Err("its position is not a usable number".into());
        }
        if !(0.0..=5000.0).contains(&self.base_height_mm) {
            return Err("the base height must be between 0 and 5000 mm".into());
        }
        if !(-90.0..=90.0).contains(&self.tilt_deg) {
            return Err("the mount tilt must be between -90 and 90 degrees".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct CameraConfig {
    /// Stable and never shown. Clips and presets refer to it.
    pub id: String,
    pub name: String,
    pub axes: AxisTable,
    pub keep_out: Vec<KeepOutRule>,
    pub rig: RigConfig,
    pub lens: Lens,
    pub mount: Mount,
    /// Where a new song starts this camera. The whole pose, so focus and the
    /// column height stay with the position and zoom. None keeps today's home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_pose: Option<Pose>,
    #[serde(skip_serializing_if = "crate::framing::FollowComposition::is_default")]
    pub follow_composition: crate::framing::FollowComposition,
}

impl Default for CameraConfig {
    fn default() -> Self {
        CameraConfig::new(FIRST_CAMERA, "Camera 1")
    }
}

impl CameraConfig {
    /// An FR7 hung upside down travels the mirror of its upright tilt range: it looks straight
    /// down at -90 degrees, not +90. A camera whose tilt limits are still the default for the
    /// other way up is given the default for the way it is mounted. Limits someone has set
    /// are left as they are.
    pub fn settle_tilt_range(&mut self) {
        let upright = AxisTable::default().tilt;
        let (upright, hung) = ((upright.min, upright.max), (-upright.max, -upright.min));
        let (now, fits) = ((self.axes.tilt.min, self.axes.tilt.max), if self.mount.inverted { hung } else { upright });
        if now != fits && (now == upright || now == hung) {
            (self.axes.tilt.min, self.axes.tilt.max) = fits;
        }
    }

    /// Whether the camera stands on a column: an AutoPod+ is named for it.
    pub fn has_column(&self) -> bool {
        !self.rig.autopod_host.trim().is_empty()
    }

    pub fn new(id: &str, name: &str) -> CameraConfig {
        CameraConfig {
            id: id.into(),
            name: name.into(),
            axes: AxisTable::default(),
            keep_out: Vec::new(),
            rig: RigConfig::default(),
            lens: Lens::default(),
            mount: Mount::default(),
            default_pose: None,
            follow_composition: Default::default(),
        }
    }
}

/// What happens when a camera faults while the show is live, and whether being off the
/// timeline counts as a fault at all.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum OnFault {
    /// The camera that faulted is stopped and held; the others carry on.
    #[default]
    HoldCamera,
    /// Output is cut for every camera.
    HoldAll,
    /// A camera that is off the timeline is not a fault: it is driven on, however far off it
    /// is, and Arm does not wait for ever for a rig that cannot reach the playhead. Only
    /// E-Stop stops it. A rig that stops answering, a limit or a keep-out rule still holds
    /// that camera.
    CarryOn,
}

impl OnFault {
    pub const ALL: [OnFault; 3] = [OnFault::HoldCamera, OnFault::HoldAll, OnFault::CarryOn];

    pub fn label(self) -> &'static str {
        match self {
            OnFault::HoldCamera => "Hold that camera, the others carry on",
            OnFault::HoldAll => "Hold every camera",
            OnFault::CarryOn => "Never hold a camera for being off the timeline",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum Unit {
    Mm,
    Cm,
    #[default]
    M,
    In,
    Ft,
}

impl Unit {
    pub const ALL: [Unit; 5] = [Unit::M, Unit::Cm, Unit::Mm, Unit::In, Unit::Ft];

    pub fn label(self) -> &'static str {
        match self {
            Unit::Mm => "Millimetres",
            Unit::Cm => "Centimetres",
            Unit::M => "Metres",
            Unit::In => "Inches",
            Unit::Ft => "Feet",
        }
    }

    /// Metres in one of this unit.
    pub fn metres(self) -> f64 {
        match self {
            Unit::Mm => 0.001,
            Unit::Cm => 0.01,
            Unit::M => 1.0,
            Unit::In => 0.0254,
            Unit::Ft => 0.3048,
        }
    }

    /// The unit that makes a model whose largest side is `largest` a believable venue
    /// (3 m to 300 m): metres, then millimetres, then centimetres. Model files do not
    /// say, and inches or feet cannot be told from those by size, so they are chosen by hand.
    pub fn guess(largest: f64) -> Unit {
        [Unit::M, Unit::Mm, Unit::Cm].into_iter().find(|u| (3.0..=300.0).contains(&(largest * u.metres()))).unwrap_or(Unit::M)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpAxis {
    #[default]
    Z,
    Y,
}

impl UpAxis {
    pub const ALL: [UpAxis; 2] = [UpAxis::Z, UpAxis::Y];

    pub fn label(self) -> &'static str {
        match self {
            UpAxis::Z => "Z is up",
            UpAxis::Y => "Y is up",
        }
    }
}

/// The imported model, as the server holds it, and how it sits in the world.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct StageModel {
    /// File name it was imported from.
    pub name: String,
    /// Goes up with every upload, so a client knows to fetch the mesh again.
    pub revision: u64,
    pub triangles: u64,
    /// Lowest and highest corner as read from the file, in the file's own units and axes.
    pub min: [f64; 3],
    pub max: [f64; 3],
    pub unit: Unit,
    pub up: UpAxis,
    /// Metres, applied after unit, up axis and rotation.
    pub offset: [f64; 3],
    /// About the vertical axis, counter-clockwise seen from above.
    pub rotation_deg: f64,
}

impl StageModel {
    /// The parts only the server may set: they describe the file it holds.
    pub fn same_file(&self, other: &StageModel) -> bool {
        (&self.name, self.revision, self.triangles, self.min, self.max)
            == (&other.name, other.revision, other.triangles, other.min, other.max)
    }

    /// Width, depth and height in metres once unit and up axis are applied (before rotation).
    pub fn size_m(&self) -> [f64; 3] {
        let s = self.unit.metres();
        let d = [0, 1, 2].map(|i| (self.max[i] - self.min[i]) * s);
        match self.up {
            UpAxis::Z => d,
            UpAxis::Y => [d[0], d[2], d[1]],
        }
    }

    /// Where a point of the file stands in the world, in metres: unit, then up axis, then rotation, then offset.
    pub fn place(&self, p: [f64; 3]) -> [f64; 3] {
        let s = self.unit.metres();
        let [x, y, z] = match self.up {
            UpAxis::Z => [p[0] * s, p[1] * s, p[2] * s],
            // A quarter turn about X: the file's Y becomes height and nothing is mirrored.
            UpAxis::Y => [p[0] * s, -p[2] * s, p[1] * s],
        };
        let (sin, cos) = self.rotation_deg.to_radians().sin_cos();
        [x * cos - y * sin + self.offset[0], x * sin + y * cos + self.offset[1], z + self.offset[2]]
    }

    /// Lowest and highest corner of the placed model's box, in metres.
    pub fn bounds_m(&self) -> ([f64; 3], [f64; 3]) {
        let (mut min, mut max) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
        for corner in 0..8 {
            let p = self.place([0, 1, 2].map(|k| if corner >> k & 1 == 1 { self.max[k] } else { self.min[k] }));
            for k in 0..3 {
                min[k] = min[k].min(p[k]);
                max[k] = max[k].max(p[k]);
            }
        }
        (min, max)
    }

    /// Move the model up or down so its lowest point is on the floor.
    pub fn sit_on_floor(&mut self) {
        self.offset[2] -= self.bounds_m().0[2];
    }

    /// Move the model sideways so the middle of its footprint is at the origin.
    pub fn centre(&mut self) {
        let (min, max) = self.bounds_m();
        self.offset[0] -= (min[0] + max[0]) / 2.0;
        self.offset[1] -= (min[1] + max[1]) / 2.0;
    }

    pub fn validate(&self) -> Result<(), String> {
        let finite = self.offset.iter().chain(&self.min).chain(&self.max).chain([&self.rotation_deg]).all(|v| v.is_finite());
        if !finite || self.offset.iter().any(|v| v.abs() > 100_000.0) {
            return Err("The stage model's position is not a usable number".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Stage {
    /// Draw the simple built-in stage: floor, platform, lectern, awards table, back wall.
    pub show_builtin: bool,
    pub model: Option<StageModel>,
}

impl Default for Stage {
    fn default() -> Self {
        Stage { show_builtin: true, model: None }
    }
}

impl Stage {
    /// Both stages hold the same imported file, or none. How it is placed may differ.
    pub fn same_model(&self, other: &Stage) -> bool {
        match (&self.model, &other.model) {
            (None, None) => true,
            (Some(a), Some(b)) => a.same_file(b),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_is_placed_by_unit_up_axis_rotation_and_offset_in_that_order() {
        // A box 4000 by 3000 by 2000 in millimetres, Y up, its low corner 1000 above and 500 behind the file's origin.
        let mut model = StageModel {
            min: [0.0, 1000.0, -3500.0],
            max: [4000.0, 3000.0, -500.0],
            unit: Unit::Mm,
            up: UpAxis::Y,
            ..StageModel::default()
        };
        assert_eq!(model.size_m(), [4.0, 3.0, 2.0]);
        // The file's Y is height, and its -Z is depth away from the camera.
        assert_eq!(model.place([4000.0, 3000.0, -3500.0]), [4.0, 3.5, 3.0]);
        assert_eq!(model.bounds_m(), ([0.0, 0.5, 1.0], [4.0, 3.5, 3.0]));

        model.sit_on_floor();
        model.centre();
        assert_eq!(model.offset, [-2.0, -2.0, -1.0]);
        assert_eq!(model.bounds_m(), ([-2.0, -1.5, 0.0], [2.0, 1.5, 2.0]));

        // A quarter turn swaps width and depth; the offset is applied after it, so the box is no longer centred.
        model.rotation_deg = 90.0;
        let (min, max) = model.bounds_m();
        let near = |a: [f64; 3], b: [f64; 3]| a.iter().zip(&b).all(|(a, b)| (a - b).abs() < 1e-9);
        assert!(near(min, [-5.5, -2.0, 0.0]) && near(max, [-2.5, 2.0, 2.0]), "{min:?} {max:?}");
        model.centre();
        let (min, max) = model.bounds_m();
        assert!(near(min, [-1.5, -2.0, 0.0]) && near(max, [1.5, 2.0, 2.0]), "{min:?} {max:?}");

        // In metres with Z up the file is taken as it is.
        let plain = StageModel { min: [0.0; 3], max: [40.0, 25.0, 9.0], ..StageModel::default() };
        assert_eq!(plain.place([40.0, 25.0, 9.0]), [40.0, 25.0, 9.0]);
        assert_eq!(plain.size_m(), [40.0, 25.0, 9.0]);
    }

    #[test]
    fn the_lens_rises_with_the_column_and_turns_with_the_heading() {
        let mount = Mount { x: 2.0, y: -3.0, z: 0.3, heading_deg: 90.0, base_height_mm: 1200.0, ..Mount::default() };
        let p = mount.lens_position(400.0);
        assert_eq!((p[0], p[1]), (2.0, -3.0));
        assert!((p[2] - 1.9).abs() < 1e-12);
        let pose = Pose { pan: -20.0, tilt: 5.0, ..Pose::home() };
        assert_eq!(mount.view(&pose), (70.0, 5.0));
        assert!(mount.validate().is_ok());
        assert!(Mount { base_height_mm: 6000.0, ..mount }.validate().is_err());
        assert!(Mount { x: f64::NAN, ..mount }.validate().is_err());
    }

    #[test]
    fn an_inverted_camera_has_the_mirrored_tilt_range_unless_its_limits_were_set() {
        // A project from before: the overhead camera is marked inverted, its limits still upright.
        let mut project = crate::pose::Project::default();
        project.cameras.push(CameraConfig::new("cam2", "Overhead"));
        project.cameras[1].mount.inverted = true;
        let range = |c: &CameraConfig| (c.axes.tilt.min, c.axes.tilt.max);
        assert_eq!(range(&project.cameras[1]), (-30.0, 195.0));
        let opened: crate::pose::Project = serde_json::from_str(&serde_json::to_string(&project).unwrap()).unwrap();
        assert_eq!(range(&opened.cameras[0]), (-30.0, 195.0), "an upright camera is as it was");
        assert_eq!(range(&opened.cameras[1]), (-195.0, 30.0), "straight down is -90 for a camera that hangs");
        assert_eq!(opened.cameras[1].axes.tilt.max_speed, project.cameras[1].axes.tilt.max_speed);

        // Turned upright again, it gets the upright range back.
        let mut camera = opened.cameras[1].clone();
        camera.mount.inverted = false;
        camera.settle_tilt_range();
        assert_eq!(range(&camera), (-30.0, 195.0));
        // Limits someone measured are theirs, whichever way up the camera is.
        for inverted in [false, true] {
            let mut camera = CameraConfig::default();
            (camera.axes.tilt.min, camera.axes.tilt.max, camera.mount.inverted) = (-100.0, 20.0, inverted);
            camera.settle_tilt_range();
            assert_eq!(range(&camera), (-100.0, 20.0));
        }
    }

    #[test]
    fn an_inverted_camera_hangs_below_its_attachment_without_reversing_calibrated_angles() {
        let mount = Mount { x: 2.0, y: -3.0, z: 6.0, heading_deg: 90.0, tilt_deg: -12.0, inverted: true, base_height_mm: 1200.0 };
        assert_eq!(mount.lens_position(0.0), [2.0, -3.0, 4.8]);
        assert_eq!(mount.lens_position(400.0), [2.0, -3.0, 4.4]);
        let pose = Pose { pan: -20.0, tilt: 5.0, ..Pose::home() };
        assert_eq!(mount.view(&pose), (70.0, -7.0));
        assert_eq!(mount.view(&pose), Mount { inverted: false, ..mount }.view(&pose));
        assert!(mount.validate().is_ok());
    }

    #[test]
    fn world_aim_accounts_for_the_mount_angles_and_wraps_across_north() {
        for inverted in [false, true] {
            let mount = Mount { heading_deg: 170.0, tilt_deg: -15.0, inverted, ..Mount::default() };
            let (pan, tilt) = mount.pose_angles(-170.0, 8.0);
            assert_eq!((pan, tilt), (20.0, 23.0));
            let view = mount.view(&Pose { pan, tilt, ..Pose::home() });
            assert_eq!(view, (190.0, 8.0));
            for (pan, tilt) in [(-90.0, -20.0), (0.0, 0.0), (45.0, 30.0)] {
                let (yaw, pitch) = mount.view(&Pose { pan, tilt, ..Pose::home() });
                assert_eq!(mount.pose_angles(yaw, pitch), (pan, tilt));
            }
        }
    }

    #[test]
    fn older_mounts_stay_upright_and_new_mount_settings_round_trip() {
        let legacy = r#"{"x":2.0,"y":-3.0,"z":0.3,"heading_deg":90.0,"base_height_mm":1200.0}"#;
        let mount: Mount = serde_json::from_str(legacy).unwrap();
        assert!(!mount.inverted);
        assert_eq!(mount.tilt_deg, 0.0);
        let empty: Mount = serde_json::from_str("{}").unwrap();
        assert_eq!(empty, Mount::default());

        let camera =
            CameraConfig { mount: Mount { inverted: true, tilt_deg: -35.0, z: 6.0, ..mount }, ..CameraConfig::default() };
        let json = serde_json::to_string(&camera).unwrap();
        assert_eq!(serde_json::from_str::<CameraConfig>(&json).unwrap(), camera);
    }

    #[test]
    fn mount_tilt_is_finite_and_within_a_quarter_turn() {
        for tilt_deg in [-90.0, 0.0, 90.0] {
            assert!(Mount { tilt_deg, ..Mount::default() }.validate().is_ok());
        }
        for tilt_deg in [-90.1, 90.1, f64::INFINITY, f64::NEG_INFINITY, f64::NAN] {
            assert!(Mount { tilt_deg, ..Mount::default() }.validate().is_err());
        }
    }

    #[test]
    fn units_are_guessed_from_the_size_of_a_venue() {
        assert_eq!(Unit::guess(42.0), Unit::M);
        assert_eq!(Unit::guess(42_000.0), Unit::Mm);
        assert_eq!(Unit::guess(4_200.0), Unit::Mm, "4.2 m in millimetres before 42 m in centimetres");
        assert_eq!(Unit::guess(1_650.0), Unit::Cm, "too small a room in millimetres, 16.5 m in centimetres");
        assert_eq!(Unit::guess(0.5), Unit::M, "nothing fits: metres, and the stated size shows it is wrong");
        assert_eq!(Unit::guess(400_000.0), Unit::M);
        assert!((Unit::Ft.metres() * 100.0 - 30.48).abs() < 1e-12 && (Unit::In.metres() * 12.0 - 0.3048).abs() < 1e-12);
    }

    #[test]
    fn a_y_up_model_reports_its_height_as_height() {
        let model = StageModel {
            min: [0.0, 0.0, 0.0],
            max: [4200.0, 910.0, 2850.0],
            unit: Unit::Cm,
            up: UpAxis::Y,
            ..Default::default()
        };
        let size = model.size_m();
        assert!((size[0] - 42.0).abs() < 1e-9 && (size[1] - 28.5).abs() < 1e-9 && (size[2] - 9.1).abs() < 1e-9, "{size:?}");
    }
}
