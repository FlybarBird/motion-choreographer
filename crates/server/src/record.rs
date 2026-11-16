//! Record passes: the rig's pose on every frame while Record is on, and the
//! raw samples kept beside the song so a pass can be refitted later.

use ptz_core::fit::{default_tolerance, fit_keys};
use ptz_core::pose::{Axis, Pose};
use ptz_core::song::Key;
use ptz_core::time::FrameRate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The raw samples of one pass, as stored in `songs/<id>/passes/<pass>.json`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct RecordedPass {
    pub id: String,
    pub song_id: String,
    /// The camera that was recorded. Passes from before a project could hold several are the first camera's.
    #[serde(default = "ptz_core::camera::first_camera")]
    pub camera: String,
    pub recorded_at: String,
    pub rate: FrameRate,
    /// Timeline frame of the first sample.
    pub start_frame: i64,
    /// How far the samples were shifted back to allow for the rig's answer time.
    pub query_delay_s: f64,
    /// One value per frame from `start_frame`, for each recorded axis.
    pub samples: BTreeMap<Axis, Vec<f64>>,
}

impl RecordedPass {
    pub fn frames(&self) -> usize {
        self.samples.values().map(|v| v.len()).min().unwrap_or(0)
    }
}

/// Fit every axis of a pass. `scale` multiplies the default tolerance.
pub fn fit_pass(samples: &BTreeMap<Axis, Vec<f64>>, scale: f64) -> BTreeMap<Axis, Vec<Key>> {
    samples.iter().map(|(axis, values)| (*axis, fit_keys(values, default_tolerance(*axis) * scale))).collect()
}

/// A pass in progress. Readings arrive at the engine's tick rate, stamped with
/// the transport position they belong to; the capture keeps one pose per whole
/// frame, interpolated between the readings either side of it.
#[derive(Clone, Debug)]
pub struct Capture {
    pub camera: String,
    pub axes: Vec<Axis>,
    start_frame: i64,
    poses: Vec<Pose>,
    last: Option<(f64, Pose)>,
    /// The server was told to drive this camera by hand during the pass.
    driven: bool,
    /// The lowest and the highest reading of each axis so far.
    span: Option<(Pose, Pose)>,
}

fn lerp(a: &Pose, b: &Pose, t: f64) -> Pose {
    Pose::from_fn(|ax| a.get(ax) + (b.get(ax) - a.get(ax)) * t)
}

impl Capture {
    pub fn new(camera: &str, axes: Vec<Axis>) -> Capture {
        Capture { camera: camera.to_string(), axes, start_frame: 0, poses: Vec::new(), last: None, driven: false, span: None }
    }

    /// The camera was driven by hand during the pass.
    pub fn mark_driven(&mut self) {
        self.driven = true;
    }

    pub fn driven(&self) -> bool {
        self.driven
    }

    /// How far apart the readings of `axis` have been: nothing for a rig that stood where it was.
    pub fn travel(&self, axis: Axis) -> f64 {
        self.span.map(|(low, high)| high.get(axis) - low.get(axis)).unwrap_or(0.0)
    }

    /// The rig was at `pose` when the transport was at `stamp` frames.
    pub fn feed(&mut self, stamp: f64, pose: Pose) {
        self.span = Some(match self.span {
            None => (pose, pose),
            Some((low, high)) => {
                (Pose::from_fn(|a| low.get(a).min(pose.get(a))), Pose::from_fn(|a| high.get(a).max(pose.get(a))))
            }
        });
        match self.last {
            None => {
                self.start_frame = stamp.ceil() as i64;
                if stamp == self.start_frame as f64 {
                    self.poses.push(pose);
                }
            }
            Some((t0, p0)) => {
                if stamp <= t0 {
                    return;
                }
                let mut n = t0.floor() as i64 + 1;
                while n as f64 <= stamp {
                    if n >= self.start_frame {
                        self.poses.push(lerp(&p0, &pose, (n as f64 - t0) / (stamp - t0)));
                    }
                    n += 1;
                }
            }
        }
        self.last = Some((stamp, pose));
    }

    pub fn start_frame(&self) -> i64 {
        self.start_frame
    }

    /// Whole frames captured so far.
    pub fn frames(&self) -> usize {
        self.poses.len()
    }

    pub fn samples(&self) -> BTreeMap<Axis, Vec<f64>> {
        self.axes.iter().map(|a| (*a, self.poses.iter().map(|p| p.get(*a)).collect())).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::{RigAdapter, Simulator};
    use ptz_core::api::Jog;
    use ptz_core::edit;
    use ptz_core::eval::pose_at;
    use ptz_core::pose::Project;
    use ptz_core::song::Song;

    #[test]
    fn whole_frames_are_interpolated_between_readings() {
        let mut c = Capture::new(ptz_core::FIRST_CAMERA, vec![Axis::Pan]);
        let at = |pan: f64| Pose { pan, ..Pose::home() };
        // Readings at 60 Hz against 25 fps: 0.4 frames apart, value equal to the stamp.
        for i in 0..=50 {
            let stamp = 9.7 + i as f64 * (25.0 / 60.0);
            c.feed(stamp, at(stamp));
        }
        assert_eq!(c.start_frame(), 10);
        let pan = &c.samples()[&Axis::Pan];
        assert_eq!(pan.len(), 21);
        for (i, v) in pan.iter().enumerate() {
            assert!((v - (10 + i) as f64).abs() < 1e-9, "frame {} holds {v}", 10 + i);
        }
        // A reading that does not move time forward changes nothing.
        c.feed(12.0, at(99.0));
        assert_eq!(c.frames(), 21);
    }

    #[test]
    fn a_capture_knows_how_far_each_axis_travelled_and_whether_it_was_driven() {
        let mut c = Capture::new(ptz_core::FIRST_CAMERA, vec![Axis::Pan, Axis::Tilt]);
        assert_eq!((c.travel(Axis::Pan), c.driven()), (0.0, false));
        for (stamp, pan) in [(0.0, 4.0), (1.0, 4.0), (2.0, 1.5), (3.0, 6.0), (4.0, 4.0)] {
            c.feed(stamp, Pose { pan, tilt: -2.0, ..Pose::home() });
        }
        assert_eq!((c.travel(Axis::Pan), c.travel(Axis::Tilt)), (4.5, 0.0));
        c.mark_driven();
        assert!(c.driven());
    }

    #[test]
    fn a_pass_that_starts_on_a_frame_keeps_that_frame() {
        let mut c = Capture::new(ptz_core::FIRST_CAMERA, vec![Axis::Tilt]);
        c.feed(0.0, Pose { tilt: 3.0, ..Pose::home() });
        c.feed(2.5, Pose { tilt: 8.0, ..Pose::home() });
        assert_eq!(c.start_frame(), 0);
        assert_eq!(c.samples()[&Axis::Tilt], vec![3.0, 5.0, 7.0]);
    }

    /// The scripted jog from the plan: pan right at 10 degrees per second from
    /// frame 100 to frame 200, captured off the simulator at the engine's tick
    /// rate, fitted, punched in and read back frame by frame.
    #[test]
    fn a_scripted_jog_plays_back_on_the_frames_it_was_recorded_at() {
        let project = Project::default();
        let fps = project.rate.fps();
        let top = project.first().axes.get(Axis::Pan).max_speed;
        let mut sim = Simulator::new();
        let mut cap = Capture::new(ptz_core::FIRST_CAMERA, vec![Axis::Pan]);
        let dt = 1.0 / 60.0;
        let mut t = 0.0f64;
        while t * fps <= 260.0 {
            let frame = t * fps;
            let jog = if (100.0..200.0).contains(&frame) { 10.0 / top } else { 0.0 };
            cap.feed(frame, sim.pose());
            sim.jog(Jog { pan: jog, ..Jog::default() }).unwrap();
            sim.tick(dt, &project.first().axes);
            t += dt;
        }
        let samples = cap.samples();
        let frames = cap.frames() as i64;
        assert!(frames >= 260);
        let curves = fit_pass(&samples, 1.0);
        // Still before and after, one ramp between: a handful of keys, not one per frame.
        assert!(curves[&Axis::Pan].len() <= 10, "{} keys", curves[&Axis::Pan].len());

        let song = Song::new("s", "S", "01:00:00:00", 400);
        let (song, clip) =
            edit::punch_keys(&song, &project, ptz_core::FIRST_CAMERA, cap.start_frame(), frames - 1, curves, Some("p".into()))
                .unwrap();
        assert!(song.motion_clip(&clip).is_some_and(|c| c.recorded_pass.as_deref() == Some("p")));
        // The simulator ramps up to speed and back down like a real head, so the move
        // trails the script by half a ramp.
        let ramp_s = 10.0 / (project.first().axes.get(Axis::Pan).max_accel * crate::rig::JOG_ACCEL);
        let trail = 10.0 * ramp_s / 2.0;
        let per_frame = 10.0 / fps;
        for f in 0..frames {
            let scripted = ((f as f64).clamp(100.0, 200.0) - 100.0) * per_frame;
            let played = pose_at(&song, ptz_core::FIRST_CAMERA, f).pan;
            assert!(played <= scripted + 0.05 + 1e-9, "frame {f}: played {played} is ahead of the script at {scripted}");
            // The fit tolerance, the trail, plus the tick the jog took to arrive at each end.
            assert!(played >= scripted - trail - 0.05 - 10.0 * dt - 1e-9, "frame {f}: played {played}, scripted {scripted}");
            // Against what the rig actually did, the fit tolerance alone.
            assert!((played - samples[&Axis::Pan][f as usize]).abs() <= 0.05 + 1e-9, "frame {f}");
        }
        // What the rig did by hand, the rig can do again.
        let issues = ptz_core::compile::compile(&song, &project).issues;
        assert!(issues.is_empty(), "{issues:?}");
        assert!((pose_at(&song, ptz_core::FIRST_CAMERA, 150).pan - (20.0 - trail)).abs() < 0.25);
    }
}
