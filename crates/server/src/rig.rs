//! The rig: whatever moves the camera. The simulator is the default; the FR7
//! and AutoPod adapters implement the same trait. No method here blocks on the
//! network: hardware adapters do their I/O on their own threads.

use ptz_core::api::{AdapterStatus, Jog};
use ptz_core::pose::{Axis, AxisTable, Pose, Preset};

pub trait RigAdapter: Send {
    fn is_simulator(&self) -> bool {
        false
    }

    fn status(&self) -> Vec<AdapterStatus>;

    /// Why the rig cannot go live, if it cannot.
    fn problem(&self) -> Option<String>;

    /// Latest known pose.
    fn pose(&self) -> Pose;

    /// Advance by `dt` seconds. Called every engine tick.
    fn tick(&mut self, _dt: f64, _axes: &AxisTable) {}

    /// Follow `target`. `speed_scale` limits top speed while positioning.
    fn drive(&mut self, target: &Pose, speed_scale: f64);

    /// Stop every axis where it is.
    fn stop(&mut self);

    fn is_moving(&self) -> bool;

    /// Presets stored on the device, with poses.
    fn presets(&mut self) -> Result<Vec<Preset>, String>;

    /// Hand control. Only the simulator accepts it.
    fn jog(&mut self, _jog: Jog) -> Result<(), String> {
        Err("The rig controller drives the simulator only. Use the camera's own controller for hardware".into())
    }

    /// Put the given axes of a virtual rig at `pose` (Rehearse monitor). Hardware ignores this.
    fn sync_virtual(&mut self, _pose: &Pose, _axes: &[Axis]) {}

    /// Thumbnail JPEG for a device preset, if the device has one.
    fn preset_thumbnail(&mut self, _camera_number: u32) -> Option<Vec<u8>> {
        None
    }
}

pub fn seeded_presets() -> Vec<Preset> {
    vec![
        Preset {
            id: "stage-wide".into(),
            name: "Stage Wide".into(),
            pose: Pose { pan: 0.0, tilt: 2.0, zoom: 0.05, focus: 0.45, elevation: 120.0 },
            camera_number: Some(1),
            has_thumbnail: false,
        },
        Preset {
            id: "lectern-mcu".into(),
            name: "Lectern MCU".into(),
            pose: Pose { pan: -24.0, tilt: 1.5, zoom: 0.62, focus: 0.5, elevation: 310.0 },
            camera_number: Some(2),
            has_thumbnail: false,
        },
        Preset {
            id: "awards-two-shot".into(),
            name: "Awards Two-Shot".into(),
            pose: Pose { pan: 18.0, tilt: 0.5, zoom: 0.38, focus: 0.48, elevation: 220.0 },
            camera_number: Some(3),
            has_thumbnail: false,
        },
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
}

impl Default for Simulator {
    fn default() -> Self {
        Simulator { pose: Pose::home(), mode: Mode::Idle, moving: false }
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
        }]
    }

    fn problem(&self) -> Option<String> {
        None
    }

    fn pose(&self) -> Pose {
        self.pose
    }

    fn tick(&mut self, dt: f64, axes: &AxisTable) {
        let before = self.pose;
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
                    let v = jog.get(a).clamp(-1.0, 1.0) * lim.max_speed;
                    self.pose.set(a, (self.pose.get(a) + v * dt).clamp(lim.min, lim.max));
                }
            }
        }
        self.moving = before != self.pose;
    }

    fn drive(&mut self, target: &Pose, speed_scale: f64) {
        self.mode = Mode::Drive { target: *target, scale: speed_scale };
    }

    fn stop(&mut self) {
        self.mode = Mode::Idle;
        self.moving = false;
    }

    fn is_moving(&self) -> bool {
        self.moving
    }

    fn presets(&mut self) -> Result<Vec<Preset>, String> {
        Ok(seeded_presets())
    }

    fn jog(&mut self, jog: Jog) -> Result<(), String> {
        self.mode = if jog.is_zero() { Mode::Idle } else { Mode::Jog(jog) };
        Ok(())
    }

    fn sync_virtual(&mut self, pose: &Pose, axes: &[Axis]) {
        if matches!(self.mode, Mode::Jog(_)) {
            return;
        }
        for a in axes {
            self.pose.set(*a, pose.get(*a));
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
        sim.drive(&target, 1.0);
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
}
