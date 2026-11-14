//! Auto time, per-frame compile and feasibility issues.

use crate::camera::{first_camera, is_first_camera};
use crate::eval::{ease_keys, peak_factor, segment_value, Evaluator};
use crate::pose::{Axis, AxisLimits, AxisTable, KeepOutRule, Pose, Project};
use crate::song::{ClipBody, Ease, FromPose, MotionClip, Song};
use crate::time::FrameRate;
use serde::{Deserialize, Serialize};

pub const SLACK: f64 = 1.001;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Issue {
    TooFast {
        clip: String,
        axis: Axis,
        frame: i64,
        speed: f64,
        limit: f64,
    },
    TooMuchAcceleration {
        clip: String,
        axis: Axis,
        frame: i64,
        accel: f64,
        limit: f64,
    },
    OutOfRange {
        clip: Option<String>,
        #[serde(default = "first_camera", skip_serializing_if = "is_first_camera")]
        camera: String,
        axis: Axis,
        frame: i64,
        value: f64,
    },
    Jump {
        clip: String,
        axis: Axis,
        frame: i64,
        distance: f64,
    },
    KeepOut {
        #[serde(default = "first_camera", skip_serializing_if = "is_first_camera")]
        camera: String,
        frame: i64,
        rule: String,
    },
    /// The clip moves a camera the project no longer has.
    MissingCamera {
        clip: String,
        camera: String,
        frame: i64,
    },
}

impl Issue {
    pub fn clip_id(&self) -> Option<&str> {
        match self {
            Issue::TooFast { clip, .. }
            | Issue::TooMuchAcceleration { clip, .. }
            | Issue::Jump { clip, .. }
            | Issue::MissingCamera { clip, .. } => Some(clip),
            Issue::OutOfRange { clip, .. } => clip.as_deref(),
            Issue::KeepOut { .. } => None,
        }
    }

    pub fn frame(&self) -> i64 {
        match self {
            Issue::TooFast { frame, .. }
            | Issue::TooMuchAcceleration { frame, .. }
            | Issue::OutOfRange { frame, .. }
            | Issue::Jump { frame, .. }
            | Issue::KeepOut { frame, .. }
            | Issue::MissingCamera { frame, .. } => *frame,
        }
    }

    /// The camera the issue is on.
    pub fn camera<'a>(&'a self, song: &'a Song) -> &'a str {
        match self {
            Issue::OutOfRange { camera, .. } | Issue::KeepOut { camera, .. } | Issue::MissingCamera { camera, .. } => camera,
            Issue::TooFast { clip, .. } | Issue::TooMuchAcceleration { clip, .. } | Issue::Jump { clip, .. } => {
                song.motion_clip(clip).map(|c| c.camera.as_str()).unwrap_or(crate::camera::FIRST_CAMERA)
            }
        }
    }

    /// Speed and acceleration overruns can be accepted by the operator.
    /// Range, jump, keep-out and missing-camera problems cannot.
    pub fn acceptable(&self) -> bool {
        matches!(self, Issue::TooFast { .. } | Issue::TooMuchAcceleration { .. })
    }

    pub fn message(&self) -> String {
        match self {
            Issue::TooFast { axis, speed, limit, .. } => {
                format!("{} is too fast: {} per second, limit {}", axis.label(), axis.format(*speed), axis.format(*limit))
            }
            Issue::TooMuchAcceleration { axis, .. } => {
                format!("{} starts or stops too sharply", axis.label())
            }
            Issue::OutOfRange { axis, value, .. } => {
                format!("{} reaches {}, outside its range", axis.label(), axis.format(*value))
            }
            Issue::Jump { axis, distance, .. } => {
                format!("{} steps by {} at the start of this move", axis.label(), axis.format(*distance))
            }
            Issue::KeepOut { rule, .. } => format!("Keep-out rule broken: {rule}"),
            Issue::MissingCamera { .. } => "This move is for a camera that is no longer in the project".into(),
        }
    }
}

/// One camera's pose on every frame.
#[derive(Clone, Debug, Default)]
pub struct PoseTrack {
    pub camera: String,
    /// From frame 0 to the song length inclusive.
    pub poses: Vec<Pose>,
}

#[derive(Clone, Debug, Default)]
pub struct Compiled {
    /// One per camera of the project, in the project's order.
    pub tracks: Vec<PoseTrack>,
    pub issues: Vec<Issue>,
}

impl Compiled {
    /// A camera's pose on every frame; empty for a camera the project does not have.
    pub fn poses(&self, camera: &str) -> &[Pose] {
        self.tracks.iter().find(|t| t.camera == camera).map(|t| t.poses.as_slice()).unwrap_or(&[])
    }

    /// Issues that stop Arm: everything except overruns on clips the operator accepted.
    pub fn blocking<'a>(&'a self, song: &'a Song) -> impl Iterator<Item = &'a Issue> {
        self.issues.iter().filter(move |i| {
            let accepted = i.clip_id().and_then(|id| song.motion_clip(id)).is_some_and(|c| c.accepted.is_some());
            !(i.acceptable() && accepted)
        })
    }

    pub fn blocking_count(&self, song: &Song) -> usize {
        self.blocking(song).count()
    }
}

pub fn compile(song: &Song, project: &Project) -> Compiled {
    let fps = project.rate.fps();
    let mut out = Compiled::default();
    for cam in &project.cameras {
        let poses = compile_camera(song, &cam.id, &cam.axes, &cam.keep_out, fps, &mut out.issues);
        out.tracks.push(PoseTrack { camera: cam.id.clone(), poses });
    }
    for c in song.motion.iter().filter(|c| project.camera(&c.camera).is_none()) {
        out.issues.push(Issue::MissingCamera { clip: c.id.clone(), camera: c.camera.clone(), frame: c.start_frame });
    }
    out.issues.sort_by_key(|i| i.frame());
    out
}

fn compile_camera(
    song: &Song,
    camera: &str,
    axes: &AxisTable,
    keep_out: &[KeepOutRule],
    fps: f64,
    issues: &mut Vec<Issue>,
) -> Vec<Pose> {
    let n = song.length_frames.max(0) as usize;
    let idle = !song.motion.iter().any(|c| c.camera == camera);
    let ev = Evaluator::new(song, camera);
    // A camera with no moves stands at home for the whole song: one pose to judge, not one per frame.
    let poses: Vec<Pose> = if idle { vec![song.home_pose; n + 1] } else { (0..=n).map(|f| ev.pose_at(f as f64)).collect() };
    let last = if idle { 0 } else { n };

    for axis in Axis::ALL {
        let lim = axes.get(axis);
        let jumps = ev.jumps(axis);
        let is_jump = |frame: i64| jumps.iter().any(|j| j.1 == frame);
        for (clip, frame, distance) in &jumps {
            // A step on frame 0 is covered by the positioning move on Arm.
            if *frame > 0 {
                issues.push(Issue::Jump { clip: song.motion[*clip].id.clone(), axis, frame: *frame, distance: *distance });
            }
        }

        // Worst speed, acceleration and range excursion per clip.
        let mut fast: Vec<Option<(i64, f64)>> = vec![None; song.motion.len()];
        let mut sharp: Vec<Option<(i64, f64)>> = vec![None; song.motion.len()];
        let mut range: Vec<Option<(i64, f64)>> = vec![None; song.motion.len() + 1];
        let v = |f: usize| poses[f].get(axis);

        for f in 0..=last {
            let fi = f as i64;
            let value = v(f);
            if value < lim.min - 1e-9 || value > lim.max + 1e-9 {
                let slot = ev.last_clip_index(axis, fi).unwrap_or(song.motion.len());
                let over = (lim.min - value).max(value - lim.max);
                if range[slot].is_none_or(|(_, worst)| over > worst) {
                    range[slot] = Some((fi, over));
                }
            }
            if f < last && !is_jump(fi + 1) {
                let speed = (v(f + 1) - value).abs() * fps;
                if speed > lim.max_speed * SLACK {
                    if let Some(c) = ev.clip_index_at(axis, fi) {
                        if fast[c].is_none_or(|(_, worst)| speed > worst) {
                            fast[c] = Some((fi, speed));
                        }
                    }
                }
            }
            if f > 0 && f < last && !is_jump(fi) && !is_jump(fi + 1) {
                let accel = (v(f + 1) - 2.0 * value + v(f - 1)).abs() * fps * fps;
                if accel > lim.max_accel * SLACK {
                    let c = ev.clip_index_at(axis, fi).or_else(|| ev.clip_index_at(axis, fi - 1));
                    if let Some(c) = c {
                        if sharp[c].is_none_or(|(_, worst)| accel > worst) {
                            sharp[c] = Some((fi, accel));
                        }
                    }
                }
            }
        }

        for (c, hit) in fast.iter().enumerate() {
            if let Some((frame, speed)) = hit {
                issues.push(Issue::TooFast {
                    clip: song.motion[c].id.clone(),
                    axis,
                    frame: *frame,
                    speed: *speed,
                    limit: lim.max_speed,
                });
            }
        }
        for (c, hit) in sharp.iter().enumerate() {
            // One message per axis per clip: speed wins when both apply.
            if let (Some((frame, accel)), None) = (hit, fast[c]) {
                issues.push(Issue::TooMuchAcceleration {
                    clip: song.motion[c].id.clone(),
                    axis,
                    frame: *frame,
                    accel: *accel,
                    limit: lim.max_accel,
                });
            }
        }
        for (c, hit) in range.iter().enumerate() {
            if let Some((frame, _)) = hit {
                issues.push(Issue::OutOfRange {
                    clip: song.motion.get(c).map(|m| m.id.clone()),
                    camera: camera.to_string(),
                    axis,
                    frame: *frame,
                    value: v(*frame as usize),
                });
            }
        }
    }

    for rule in keep_out {
        if let Some(f) = poses[..=last].iter().position(|p| rule.violated(p)) {
            issues.push(Issue::KeepOut { camera: camera.to_string(), frame: f as i64, rule: rule.describe() });
        }
    }
    poses
}

/// True when an eased move of `frames` stays inside the speed and acceleration
/// limits, sampled per frame the same way `compile` does. The rig stands still
/// before the move and after it.
fn move_fits(from: f64, to: f64, ease: Ease, frames: i64, lim: &AxisLimits, fps: f64) -> bool {
    if from == to {
        return true;
    }
    let [k0, k1] = ease_keys(from, to, frames, ease);
    let at = |f: i64| segment_value(&k0, &k1, f.clamp(0, frames) as f64);
    let over = |before: f64, here: f64, after: f64| {
        (after - here).abs() * fps > lim.max_speed * SLACK
            || (after - 2.0 * here + before).abs() * fps * fps > lim.max_accel * SLACK
    };
    // A move that is too short fails where it starts, where it stops or where it is
    // fastest, so those frames and a spread of the rest are looked at first. A candidate
    // that is far too short is then turned down without walking its whole length.
    let probe = |f: i64| over(at(f - 1), at(f), at(f + 1));
    const SPREAD: i64 = 64;
    if probe(0) || probe(frames) || (frames > 4 * SPREAD && (1..SPREAD).any(|i| probe(frames / SPREAD * i))) {
        return false;
    }
    let (mut before, mut here) = (at(-1), at(0));
    for f in 0..=frames {
        let after = at(f + 1);
        if over(before, here, after) {
            return false;
        }
        (before, here) = (here, after);
    }
    true
}

/// Shortest whole-frame duration that every axis can make: the slowest axis's
/// travel at its top speed through the ease, plus that axis's settle time.
///
/// Never longer than the longest song. A move the limits cannot be met for inside
/// that (a pose far outside any rig, or an ease that stops dead on an axis that may
/// hardly accelerate) gets that length, and shows as an issue when it is compiled.
pub fn auto_duration(from: &Pose, to: &Pose, axes: &[Axis], ease: Ease, limits: &AxisTable, rate: FrameRate) -> i64 {
    let fps = rate.fps();
    let peak = peak_factor(ease);
    let mut seconds: f64 = 0.0;
    for &axis in axes {
        let lim = limits.get(axis);
        let travel = (to.get(axis) - from.get(axis)).abs();
        if travel > 1e-9 && lim.max_speed > 0.0 {
            seconds = seconds.max(travel * peak / lim.max_speed + lim.settle_s);
        }
    }
    if seconds == 0.0 {
        // Nothing moves. One second keeps the clip visible and grabbable.
        return rate.nominal();
    }
    let longest = Song::max_length_frames(rate);
    let first = ((seconds * fps) - 1e-9).ceil().max(1.0);
    // Also true of a time that is not a number.
    if !(first < longest as f64) {
        return longest;
    }
    let mut frames = first as i64;
    let fits =
        |frames: i64| axes.iter().all(|&axis| move_fits(from.get(axis), to.get(axis), ease, frames, limits.get(axis), fps));
    let mut guard = 0;
    while !fits(frames) && guard < 400 {
        frames += (frames / 20).max(1);
        if frames >= longest {
            return longest;
        }
        guard += 1;
    }
    frames
}

/// Resolved start and end poses of a preset move, with linked axes taken from
/// the timeline. Axes the clip does not drive keep the surrounding value.
pub fn move_endpoints(song: &Song, clip: &MotionClip) -> Option<(Pose, Pose)> {
    let ClipBody::PresetMove { from, to, .. } = &clip.body else {
        return None;
    };
    let ev = Evaluator::new(song, &clip.camera);
    let start = Pose::from_fn(|a| match from {
        FromPose::Linked(_) => ev.value_before(clip, a),
        FromPose::Pinned(p) if clip.has_axis(a) => p.pose.get(a),
        FromPose::Pinned(_) => ev.value_before(clip, a),
    });
    let end = Pose::from_fn(|a| if clip.has_axis(a) { to.pose.get(a) } else { start.get(a) });
    Some((start, end))
}

/// Auto duration for a preset move where it currently sits.
pub fn suggested_duration(song: &Song, clip: &MotionClip, project: &Project) -> Option<i64> {
    let ClipBody::PresetMove { ease, .. } = &clip.body else {
        return None;
    };
    let (from, to) = move_endpoints(song, clip)?;
    Some(auto_duration(&from, &to, &clip.axes, *ease, &project.axes_of(&clip.camera), project.rate))
}

/// Recompute every auto-timed move, earliest first. A move that has no room
/// before the next clip of its camera on its axes, or before the end of the longest song,
/// is clamped to the room, which then shows as a too-fast issue instead of an overlap.
pub fn refresh_auto(song: &mut Song, project: &Project) {
    let mut order: Vec<usize> = (0..song.motion.len()).collect();
    order.sort_by_key(|i| song.motion[*i].start_frame);
    for i in order {
        let clip = song.motion[i].clone();
        let ClipBody::PresetMove { auto_duration: true, .. } = &clip.body else {
            continue;
        };
        let Some(want) = suggested_duration(song, &clip, project) else {
            continue;
        };
        let room = song
            .motion
            .iter()
            .filter(|o| {
                o.id != clip.id
                    && o.camera == clip.camera
                    && o.start_frame >= clip.start_frame
                    && o.axes.iter().any(|a| clip.has_axis(*a))
            })
            .map(|o| o.start_frame - clip.start_frame)
            .min()
            .unwrap_or(i64::MAX);
        // Nor past the end of the longest song there can be.
        let room = room.min(Song::max_length_frames(project.rate).saturating_sub(clip.start_frame));
        song.motion[i].duration_frames = want.min(room).max(1);
    }
    let end = song.content_end();
    if song.length_frames < end {
        song.length_frames = end;
    }
}

/// Per-frame speed of one axis in units per second, for the graph editor's
/// speed view. Entry `f` is the speed between frame `f` and `f + 1`.
pub fn speed_profile(poses: &[Pose], axis: Axis, rate: FrameRate) -> Vec<f64> {
    let fps = rate.fps();
    poses.windows(2).map(|w| (w[1].get(axis) - w[0].get(axis)) * fps).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::{AxisLimits, Cmp, KeepOutRule};
    use crate::song::{LinkedTag, PresetRef};

    use crate::camera::{CameraConfig, FIRST_CAMERA};

    fn project() -> Project {
        let mut p = Project::default();
        let pan = &mut p.first_mut().axes.pan;
        (pan.max_speed, pan.max_accel, pan.settle_s) = (60.0, 2000.0, 0.2);
        p
    }

    fn auto(from: &Pose, to: &Pose, axes: &[Axis], ease: Ease, p: &Project) -> i64 {
        auto_duration(from, to, axes, ease, &p.first().axes, p.rate)
    }

    fn pan_move(start: i64, dur: i64, pan: f64, auto: bool) -> MotionClip {
        MotionClip {
            id: format!("c{start}"),
            camera: FIRST_CAMERA.into(),
            start_frame: start,
            duration_frames: dur,
            axes: vec![Axis::Pan],
            body: ClipBody::PresetMove {
                auto_duration: auto,
                from: FromPose::Linked(LinkedTag::Linked),
                to: PresetRef { preset_id: "p".into(), name: "P".into(), pose: Pose { pan, ..Pose::home() } },
                ease: Ease::EaseInOut,
            },
            accepted: None,
            recorded_pass: None,
        }
    }

    #[test]
    fn sixty_degree_pan_takes_about_1_72_seconds_plus_settle() {
        let p = project();
        let from = Pose::home();
        let to = Pose { pan: 60.0, ..Pose::home() };
        let frames = auto(&from, &to, &[Axis::Pan], Ease::EaseInOut, &p);
        let seconds = frames as f64 / 25.0;
        assert!((seconds - (1.72 + 0.2)).abs() < 0.06, "got {seconds}");

        let linear = auto(&from, &to, &[Axis::Pan], Ease::Linear, &p);
        assert!(linear < frames);
    }

    #[test]
    fn slowest_axis_sets_the_time() {
        let p = Project::default();
        let from = Pose::home();
        let to = Pose { pan: 10.0, elevation: 430.0, ..Pose::home() };
        let both = auto(&from, &to, &[Axis::Pan, Axis::Elevation], Ease::EaseInOut, &p);
        let lift = auto(&from, &to, &[Axis::Elevation], Ease::EaseInOut, &p);
        let pan = auto(&from, &to, &[Axis::Pan], Ease::EaseInOut, &p);
        assert_eq!(both, lift);
        assert!(lift > pan * 5);
    }

    #[test]
    fn auto_time_is_feasible_and_shortening_it_is_too_fast() {
        let p = project();
        let mut song = Song::new("s", "S", "01:00:00:00", 500);
        song.motion.push(pan_move(50, 10, 60.0, true));
        refresh_auto(&mut song, &p);
        let auto = song.motion[0].duration_frames;
        assert!(compile(&song, &p).issues.is_empty(), "{:?}", compile(&song, &p).issues);

        song.motion[0].duration_frames = auto / 2;
        let c = compile(&song, &p);
        assert!(matches!(&c.issues[0], Issue::TooFast { axis: Axis::Pan, clip, .. } if clip == "c50"));
        assert_eq!(c.blocking_count(&song), 1);

        song.motion[0].accepted = Some(crate::song::Accepted { by: "luke".into(), at: "now".into() });
        assert_eq!(c.blocking_count(&song), 0);
    }

    #[test]
    fn out_of_range_and_jump_and_keep_out() {
        let mut p = project();
        let mut song = Song::new("s", "S", "01:00:00:00", 500);
        song.motion.push(pan_move(50, 200, 200.0, false));
        let c = compile(&song, &p);
        assert!(c.issues.iter().any(|i| matches!(i, Issue::OutOfRange { axis: Axis::Pan, .. })));
        assert!(c.issues.iter().all(|i| !matches!(i, Issue::OutOfRange { .. }) || !i.acceptable()));

        let mut song = Song::new("s", "S", "01:00:00:00", 500);
        let mut pinned = pan_move(50, 200, 30.0, false);
        if let ClipBody::PresetMove { from, .. } = &mut pinned.body {
            *from =
                FromPose::Pinned(PresetRef { preset_id: "x".into(), name: "X".into(), pose: Pose { pan: 15.0, ..Pose::home() } });
        }
        song.motion.push(pinned);
        let c = compile(&song, &p);
        assert!(matches!(&c.issues[0], Issue::Jump { frame: 50, distance, .. } if (*distance - 15.0).abs() < 1e-9));
        assert_eq!(c.issues.len(), 1);

        p.first_mut().keep_out.push(KeepOutRule {
            name: "desk".into(),
            when_axis: Axis::Elevation,
            when: Cmp::Below,
            when_value: 200.0,
            then_axis: Axis::Pan,
            must_be: Cmp::Below,
            must_value: 20.0,
        });
        let c = compile(&song, &p);
        assert!(c.issues.iter().any(|i| matches!(i, Issue::KeepOut { .. })));
    }

    #[test]
    fn auto_move_without_room_is_clamped_not_overlapped() {
        let p = project();
        let mut song = Song::new("s", "S", "01:00:00:00", 500);
        song.motion.push(pan_move(50, 10, 60.0, true));
        song.motion.push(pan_move(70, 30, 0.0, false));
        refresh_auto(&mut song, &p);
        assert_eq!(song.motion[0].duration_frames, 20);
        assert!(song.validate(p.rate).is_ok());
    }

    fn on(camera: &str, mut clip: MotionClip) -> MotionClip {
        clip.id = format!("{camera}-{}", clip.id);
        clip.camera = camera.into();
        clip
    }

    #[test]
    fn each_camera_is_judged_against_its_own_limits_and_rules() {
        let mut p = project();
        let mut slow = CameraConfig::new("cam2", "Slow head");
        slow.axes.pan.max_speed = 10.0;
        slow.keep_out.push(KeepOutRule {
            name: "wall".into(),
            when_axis: Axis::Elevation,
            when: Cmp::Below,
            when_value: 200.0,
            then_axis: Axis::Pan,
            must_be: Cmp::Below,
            must_value: 20.0,
        });
        p.cameras.push(slow);

        // The same two-second, 60 degree pan on both cameras, at the same time.
        let mut song = Song::new("s", "S", "01:00:00:00", 500);
        song.motion.push(pan_move(50, 50, 60.0, false));
        song.motion.push(on("cam2", pan_move(50, 50, 60.0, false)));
        assert!(song.validate(p.rate).is_ok(), "two cameras may move at once");
        let c = compile(&song, &p);
        assert_eq!(c.tracks.iter().map(|t| t.camera.as_str()).collect::<Vec<_>>(), ["cam1", "cam2"]);
        assert_eq!(c.poses("cam1").len(), 501);
        assert_eq!(c.poses("cam1")[100].pan, 60.0);
        assert_eq!(c.poses("cam2")[100].pan, 60.0);
        assert!(c.poses("nobody").is_empty());

        let of = |camera: &str| c.issues.iter().filter(|i| i.camera(&song) == camera).collect::<Vec<_>>();
        assert!(of("cam1").is_empty(), "inside the first camera's limits: {:?}", of("cam1"));
        let second = of("cam2");
        assert!(second.iter().any(|i| matches!(i, Issue::TooFast { clip, limit, .. } if clip == "cam2-c50" && *limit == 10.0)));
        assert!(second.iter().any(|i| matches!(i, Issue::KeepOut { camera, .. } if camera == "cam2")));
        assert_eq!(second.len(), 2, "{second:?}");

        // Auto time comes from the limits of the camera the move is on.
        let mut song = Song::new("s", "S", "01:00:00:00", 2000);
        song.motion.push(pan_move(50, 10, 15.0, true));
        song.motion.push(on("cam2", pan_move(50, 10, 15.0, true)));
        refresh_auto(&mut song, &p);
        let (fast, slow) = (song.motion[0].duration_frames, song.motion[1].duration_frames);
        assert!(slow > fast * 4, "{fast} against {slow}");
        assert!(compile(&song, &p).issues.is_empty());

        // Room is counted against the same camera's next clip only.
        let mut song = Song::new("s", "S", "01:00:00:00", 500);
        song.motion.push(pan_move(50, 10, 60.0, true));
        song.motion.push(on("cam2", pan_move(60, 30, 0.0, false)));
        refresh_auto(&mut song, &p);
        assert!(song.motion[0].duration_frames > 20, "another camera's clip does not squeeze this one");
    }

    #[test]
    fn a_camera_with_no_moves_is_judged_at_home_and_a_removed_camera_blocks() {
        let mut p = project();
        p.cameras.push(CameraConfig::new("cam2", "Idle"));
        p.cameras[1].axes.elevation.min = 100.0;
        let mut song = Song::new("s", "S", "01:00:00:00", 500);
        song.motion.push(pan_move(50, 200, 30.0, false));
        let c = compile(&song, &p);
        assert_eq!(c.poses("cam2").len(), 501);
        assert!(c.poses("cam2").iter().all(|pose| *pose == song.home_pose));
        let range: Vec<_> = c.issues.iter().filter(|i| matches!(i, Issue::OutOfRange { .. })).collect();
        assert_eq!(range.len(), 1, "home is below the idle camera's lowest height, said once: {range:?}");
        assert!(
            matches!(range[0], Issue::OutOfRange { camera, clip: None, frame: 0, axis: Axis::Elevation, .. } if camera == "cam2")
        );

        // The camera the move was made for is taken out of the project.
        song.motion.push(on("cam9", pan_move(300, 50, 10.0, false)));
        let before = compile(&song, &p).poses("cam1").to_vec();
        let c = compile(&song, &p);
        let missing: Vec<_> = c.issues.iter().filter(|i| matches!(i, Issue::MissingCamera { .. })).collect();
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].message(), "This move is for a camera that is no longer in the project");
        assert_eq!((missing[0].clip_id(), missing[0].camera(&song), missing[0].frame()), (Some("cam9-c300"), "cam9", 300));
        assert!(!missing[0].acceptable());
        song.motion[1].accepted = Some(crate::song::Accepted { by: "luke".into(), at: "now".into() });
        assert!(c.blocking(&song).any(|i| matches!(i, Issue::MissingCamera { .. })), "accepting does not make it playable");
        assert_eq!(c.poses("cam1"), before.as_slice());
        assert!(c.poses("cam9").is_empty());
    }

    #[test]
    fn an_issue_from_before_cameras_reads_as_the_first_cameras() {
        let old = r#"[{ "kind": "keep_out", "frame": 12, "rule": "r" },
                      { "kind": "out_of_range", "clip": null, "axis": "pan", "frame": 3, "value": 200.0 }]"#;
        let issues: Vec<Issue> = serde_json::from_str(old).unwrap();
        let song = Song::new("s", "S", "01:00:00:00", 10);
        assert!(issues.iter().all(|i| i.camera(&song) == FIRST_CAMERA));
    }

    /// The search as it was first written: every candidate length is walked frame by frame.
    fn plain_search(from: &Pose, to: &Pose, axes: &[Axis], ease: Ease, limits: &AxisTable, rate: FrameRate) -> i64 {
        let fps = rate.fps();
        let fits_axis = |from: f64, to: f64, frames: i64, lim: &AxisLimits| {
            if from == to {
                return true;
            }
            let [k0, k1] = ease_keys(from, to, frames, ease);
            let at = |f: i64| segment_value(&k0, &k1, f.clamp(0, frames) as f64);
            (0..=frames).all(|f| {
                (at(f + 1) - at(f)).abs() * fps <= lim.max_speed * SLACK
                    && (at(f + 1) - 2.0 * at(f) + at(f - 1)).abs() * fps * fps <= lim.max_accel * SLACK
            })
        };
        let mut seconds: f64 = 0.0;
        for &axis in axes {
            let lim = limits.get(axis);
            let travel = (to.get(axis) - from.get(axis)).abs();
            if travel > 1e-9 && lim.max_speed > 0.0 {
                seconds = seconds.max(travel * peak_factor(ease) / lim.max_speed + lim.settle_s);
            }
        }
        if seconds == 0.0 {
            return rate.nominal();
        }
        let mut frames = ((seconds * fps) - 1e-9).ceil().max(1.0) as i64;
        while !axes.iter().all(|&a| fits_axis(from.get(a), to.get(a), frames, limits.get(a))) {
            frames += (frames / 20).max(1);
        }
        frames
    }

    #[test]
    fn auto_time_is_what_walking_every_frame_of_every_candidate_finds() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let limits = AxisTable::default();
        for case in 0..40 {
            let rate = FrameRate::ALL[case % 5];
            let ease = Ease::ALL[case % 4];
            // Short travels as well as long ones, so the start, the middle and the stop each decide some.
            let reach = if case % 3 == 0 { 0.15 } else { 1.0 };
            let mut pose = || {
                Pose::from_fn(|a| {
                    let l = limits.get(a);
                    l.min + rnd() * (l.max - l.min) * reach
                })
            };
            let (from, to) = (pose(), pose());
            let axes: Vec<Axis> = Axis::ALL.into_iter().filter(|a| case % 7 != 0 || *a != Axis::Elevation).collect();
            assert_eq!(
                auto_duration(&from, &to, &axes, ease, &limits, rate),
                plain_search(&from, &to, &axes, ease, &limits, rate),
                "case {case}: {ease:?} at {} from {from:?} to {to:?}",
                rate.label()
            );
        }
    }

    #[test]
    fn auto_time_gives_up_at_a_day_when_the_limits_can_never_be_met() {
        // A legal project whose pedestal may hardly accelerate at all: an ease that stops
        // dead would need years. The answer is the longest a song can be, and it comes at once.
        let mut p = Project::default();
        p.first_mut().axes.elevation.max_accel = 1e-6;
        assert!(p.validate().is_ok());
        let day = p.rate.frames_per_day();
        let to = Pose { elevation: 670.0, ..Pose::home() };
        for ease in [Ease::EaseIn, Ease::EaseOut, Ease::Linear] {
            assert_eq!(auto(&Pose::home(), &to, &[Axis::Elevation], ease, &p), day, "{ease:?}");
        }
        // The same when the travel is what cannot be met.
        let far = Pose { pan: 1e12, ..Pose::home() };
        assert_eq!(auto(&Pose::home(), &far, &Axis::ALL, Ease::EaseInOut, &Project::default()), day);
    }

    #[test]
    fn a_pose_no_rig_could_reach_does_not_overflow_auto_time() {
        let p = Project::default();
        let day = p.rate.frames_per_day();
        for pan in [1e300, -1e300, f64::MAX, f64::INFINITY] {
            let far = Pose { pan, ..Pose::home() };
            assert_eq!(auto(&Pose::home(), &far, &Axis::ALL, Ease::EaseInOut, &p), day, "pan {pan}");
            assert_eq!(auto(&far, &Pose::home(), &Axis::ALL, Ease::Linear, &p), day, "pan {pan}");
        }
    }

    #[test]
    fn an_auto_move_that_can_never_be_made_stays_inside_the_longest_song() {
        let mut p = Project::default();
        p.first_mut().axes.elevation.max_accel = 1e-6;
        let day = p.rate.frames_per_day();
        let mut song = Song::new("s", "S", "01:00:00:00", 500);
        let mut clip = pan_move(100, 10, 0.0, true);
        clip.axes = vec![Axis::Elevation];
        if let ClipBody::PresetMove { to, ease, .. } = &mut clip.body {
            to.pose.elevation = 670.0;
            *ease = Ease::EaseIn;
        }
        song.motion.push(clip);
        refresh_auto(&mut song, &p);
        assert_eq!(song.motion[0].end_frame(), day, "clamped to the end of the longest song, where it shows as an issue");
        assert!(song.validate(p.rate).is_ok());
    }
}
