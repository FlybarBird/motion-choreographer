//! The rig: whatever moves the camera. The simulator is the default; the FR7
//! and AutoPod+ adapters implement the same trait. No method here blocks on the
//! network: hardware adapters do their I/O on their own threads.

use ptz_core::api::{AdapterStatus, Jog};
use ptz_core::pose::{Axis, AxisTable, Pose, Preset};
use std::time::Instant;

/// A plain pan and tilt move the head may run by itself: where it ends and how long it has.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ride {
    pub pan: f64,
    pub tilt: f64,
    pub seconds: f64,
}

/// One command to the rig: where it should be now, and how fast that is changing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Drive {
    pub target: Pose,
    /// Units per second on each axis.
    pub velocity: Pose,
    /// Share of each axis's top speed the rig may use. Positioning runs slow.
    pub scale: f64,
    /// Set while the playhead is inside a plain preset move and the project asks for
    /// absolute moves. An adapter that cannot do that follows `target` as usual.
    pub ride: Option<Ride>,
    /// False when the song has no elevation for this camera: the column is left where it stands.
    pub column: bool,
}

impl Drive {
    pub fn to(target: Pose, scale: f64) -> Drive {
        Drive { target, velocity: Pose::default(), scale, ride: None, column: true }
    }
}

/// A pose and the moment the rig was there, axis by axis: the devices are read at different times.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PoseReading {
    pub pose: Pose,
    pub at: [Instant; 5],
}

impl PoseReading {
    pub fn all_at(pose: Pose, at: Instant) -> PoseReading {
        PoseReading { pose, at: [at; 5] }
    }
}

/// A preset read from the device, with its picture when the device has one.
#[derive(Clone, PartialEq, Debug)]
pub struct DevicePreset {
    pub preset: Preset,
    pub thumbnail: Option<Vec<u8>>,
}

pub trait RigAdapter: Send {
    fn is_simulator(&self) -> bool {
        false
    }

    fn status(&self) -> Vec<AdapterStatus>;

    /// Why the rig cannot go live, if it cannot.
    fn problem(&self) -> Option<String>;

    /// Why the rig cannot go live when its column is left where it stands: the column's own
    /// troubles do not count, because nothing is asked of it.
    fn head_problem(&self) -> Option<String> {
        self.problem()
    }

    /// Latest known pose.
    fn pose(&self) -> Pose;

    /// Latest known pose and when it was read. Hardware answers a few tens of
    /// milliseconds late; the live loop compares against where the target was then.
    fn reading(&self) -> PoseReading {
        PoseReading::all_at(self.pose(), Instant::now())
    }

    /// Advance by `dt` seconds. Called every engine tick.
    fn tick(&mut self, _dt: f64, _axes: &AxisTable) {}

    /// Follow the command. Called every engine tick while live; an adapter that
    /// stops hearing it stops the rig.
    fn drive(&mut self, cmd: &Drive);

    /// Stop every axis where it is.
    fn stop(&mut self);

    fn is_moving(&self) -> bool;

    /// Presets stored on the device, with poses, when they can be read without moving it.
    fn presets(&mut self) -> Result<Vec<Preset>, String>;

    /// Start reading the device's presets by recalling each one, which moves the rig.
    fn begin_preset_sync(&mut self) -> Result<(), String> {
        Err("This rig's presets are read without moving it".into())
    }

    /// The outcome of a preset sync, once, when it has finished.
    fn take_preset_sync(&mut self) -> Option<Result<Vec<DevicePreset>, String>> {
        None
    }

    /// What a running preset sync is doing, for the operator.
    fn preset_sync_progress(&self) -> Option<String> {
        None
    }

    /// Hand control. Only the simulator accepts it.
    fn jog(&mut self, _jog: Jog) -> Result<(), String> {
        Err("The rig controller drives the simulator only. Use the camera's own controller for hardware".into())
    }

    /// How old a pose reading is when it arrives, in seconds. The recorder
    /// shifts samples back by this much.
    fn query_delay_s(&self) -> f64 {
        0.0
    }

    /// Put the given axes of a virtual rig at `pose` (Rehearse monitor). Hardware ignores this.
    fn sync_virtual(&mut self, _pose: &Pose, _axes: &[Axis]) {}
}

pub fn seeded_presets() -> Vec<Preset> {
    let preset =
        |number: u32, id: &str, name: &str, pose: Pose| Preset { camera_number: Some(number), ..Preset::new(id, name, pose) };
    vec![
        preset(1, "stage-wide", "Stage Wide", Pose { pan: 0.0, tilt: 2.0, zoom: 0.05, focus: 0.45, elevation: 120.0 }),
        preset(2, "lectern-mcu", "Lectern MCU", Pose { pan: -24.0, tilt: 1.5, zoom: 0.62, focus: 0.5, elevation: 310.0 }),
        preset(3, "awards-two-shot", "Awards Two-Shot", Pose { pan: 18.0, tilt: 0.5, zoom: 0.38, focus: 0.48, elevation: 220.0 }),
    ]
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Idle,
    Drive { target: Pose, scale: f64 },
    Jog(Jog),
}

pub struct Simulator {
    pose: Pose,
    mode: Mode,
    moving: bool,
    /// Speed of each axis under hand control, in its own units per second.
    jog_speed: [f64; 5],
    /// When the pose last advanced.
    read_at: Instant,
}

/// Share of an axis's acceleration limit used to ramp a jog up and down. A real head ramps
/// too, and a pass recorded from one that did not would fail its own feasibility check.
pub const JOG_ACCEL: f64 = 0.5;

impl Default for Simulator {
    fn default() -> Self {
        Simulator { pose: Pose::home(), mode: Mode::Idle, moving: false, jog_speed: [0.0; 5], read_at: Instant::now() }
    }
}

impl Simulator {
    pub fn new() -> Simulator {
        Simulator::default()
    }
}

impl RigAdapter for Simulator {
    fn is_simulator(&self) -> bool {
        true
    }

    fn status(&self) -> Vec<AdapterStatus> {
        vec![AdapterStatus {
            name: "Simulator".into(),
            connected: true,
            detail: "Built-in virtual rig. No hardware moves.".into(),
            calibrated: None,
            round_trip_ms: None,
            raw_position: None,
        }]
    }

    fn problem(&self) -> Option<String> {
        None
    }

    fn pose(&self) -> Pose {
        self.pose
    }

    fn reading(&self) -> PoseReading {
        PoseReading::all_at(self.pose, self.read_at)
    }

    fn tick(&mut self, dt: f64, axes: &AxisTable) {
        let before = self.pose;
        self.read_at = Instant::now();
        match self.mode {
            Mode::Idle => {}
            Mode::Drive { target, scale } => {
                for a in Axis::ALL {
                    let lim = axes.get(a);
                    let step = lim.max_speed * scale.clamp(0.0001, 1.0) * dt;
                    let cur = self.pose.get(a);
                    let want = target.get(a).clamp(lim.min, lim.max);
                    let next = cur + (want - cur).clamp(-step, step);
                    self.pose.set(a, next);
                }
            }
            Mode::Jog(jog) => {
                for a in Axis::ALL {
                    let lim = axes.get(a);
                    let want = jog.get(a).clamp(-1.0, 1.0) * lim.max_speed;
                    let ramp = lim.max_accel * JOG_ACCEL * dt;
                    let speed = &mut self.jog_speed[a.index()];
                    *speed += (want - *speed).clamp(-ramp, ramp);
                    let next = self.pose.get(a) + *speed * dt;
                    if next <= lim.min || next >= lim.max {
                        *speed = 0.0;
                    }
                    self.pose.set(a, next.clamp(lim.min, lim.max));
                }
                if jog.is_zero() && self.jog_speed.iter().all(|v| *v == 0.0) {
                    self.mode = Mode::Idle;
                }
            }
        }
        self.moving = before != self.pose;
    }

    fn drive(&mut self, cmd: &Drive) {
        self.jog_speed = [0.0; 5];
        self.mode = Mode::Drive { target: cmd.target, scale: cmd.scale };
    }

    fn stop(&mut self) {
        self.mode = Mode::Idle;
        self.jog_speed = [0.0; 5];
        self.moving = false;
    }

    fn is_moving(&self) -> bool {
        self.moving
    }

    fn presets(&mut self) -> Result<Vec<Preset>, String> {
        Ok(seeded_presets())
    }

    fn jog(&mut self, jog: Jog) -> Result<(), String> {
        // Letting go ramps down to a stop; `stop` is the one that halts on the spot.
        self.mode = Mode::Jog(jog);
        Ok(())
    }

    fn sync_virtual(&mut self, pose: &Pose, axes: &[Axis]) {
        // An axis being driven by hand, or still slowing down, is left alone; the rest follow.
        let held = match self.mode {
            Mode::Jog(jog) => jog,
            _ => Jog::default(),
        };
        for a in axes {
            if held.get(*a) == 0.0 && self.jog_speed[a.index()] == 0.0 {
                self.pose.set(*a, pose.get(*a));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simulator_moves_within_speed_limits_and_stops() {
        let axes = AxisTable::default();
        let mut sim = Simulator::new();
        let target = Pose { pan: 90.0, tilt: 10.0, zoom: 1.0, focus: 0.5, elevation: 300.0 };
        sim.drive(&Drive::to(target, 1.0));
        let mut last = sim.pose();
        for _ in 0..60 {
            sim.tick(1.0 / 60.0, &axes);
            let p = sim.pose();
            assert!((p.pan - last.pan).abs() <= 60.0 / 60.0 + 1e-9);
            assert!((p.elevation - last.elevation).abs() <= 43.0 / 60.0 + 1e-9);
            last = p;
        }
        assert!((sim.pose().pan - 60.0).abs() < 1e-6);
        assert!(sim.is_moving());
        sim.stop();
        let held = sim.pose();
        sim.tick(1.0, &axes);
        assert_eq!(sim.pose(), held);
        assert!(!sim.is_moving());
    }

    #[test]
    fn jog_respects_range() {
        let axes = AxisTable::default();
        let mut sim = Simulator::new();
        sim.jog(Jog { pan: 1.0, ..Jog::default() }).unwrap();
        for _ in 0..600 {
            sim.tick(0.1, &axes);
        }
        assert_eq!(sim.pose().pan, 170.0);
        sim.sync_virtual(&Pose::home(), &Axis::ALL);
        assert_eq!(sim.pose().pan, 170.0, "a jogged rig is not overwritten by the timeline");
        sim.jog(Jog::default()).unwrap();
        sim.sync_virtual(&Pose::home(), &[Axis::Pan]);
        assert_eq!(sim.pose().pan, 0.0);
    }

    #[test]
    fn a_jog_ramps_up_and_down_inside_the_acceleration_limit() {
        let axes = AxisTable::default();
        let lim = axes.get(Axis::Pan);
        let mut sim = Simulator::new();
        let dt = 1.0 / 100.0;
        let mut trace = vec![sim.pose().pan];
        sim.jog(Jog { pan: 0.5, ..Jog::default() }).unwrap();
        for _ in 0..100 {
            sim.tick(dt, &axes);
            trace.push(sim.pose().pan);
        }
        let cruise = (trace[100] - trace[99]) / dt;
        assert!((cruise - 0.5 * lim.max_speed).abs() < 1e-6, "{cruise}");
        sim.jog(Jog::default()).unwrap();
        for _ in 0..100 {
            sim.tick(dt, &axes);
            trace.push(sim.pose().pan);
        }
        assert_eq!(trace[200], trace[199], "the rig has come to rest");
        assert!(!sim.is_moving());
        let worst = trace.windows(3).map(|w| ((w[2] - 2.0 * w[1] + w[0]) / (dt * dt)).abs()).fold(0.0, f64::max);
        assert!(worst <= lim.max_accel * JOG_ACCEL + 1e-6, "{worst}");
        // Slowing down is not overwritten by the timeline; a rig at rest is.
        sim.jog(Jog { pan: 0.5, ..Jog::default() }).unwrap();
        sim.tick(dt, &axes);
        sim.jog(Jog::default()).unwrap();
        sim.sync_virtual(&Pose::home(), &Axis::ALL);
        assert!(sim.pose().pan > 1.0);
        sim.stop();
        sim.sync_virtual(&Pose::home(), &Axis::ALL);
        assert_eq!(sim.pose().pan, 0.0);
    }
}
