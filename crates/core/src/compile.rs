//! Auto time, per-frame compile and feasibility issues.

use crate::eval::{ease_keys, peak_factor, segment_value, Evaluator};
use crate::pose::{Axis, AxisLimits, Pose, Project};
use crate::song::{ClipBody, Ease, FromPose, MotionClip, Song};
use crate::time::FrameRate;
use serde::{Deserialize, Serialize};

pub const SLACK: f64 = 1.001;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Issue {
    TooFast { clip: String, axis: Axis, frame: i64, speed: f64, limit: f64 },
    TooMuchAcceleration { clip: String, axis: Axis, frame: i64, accel: f64, limit: f64 },
    OutOfRange { clip: Option<String>, axis: Axis, frame: i64, value: f64 },
    Jump { clip: String, axis: Axis, frame: i64, distance: f64 },
    KeepOut { frame: i64, rule: String },
}

impl Issue {
    pub fn clip_id(&self) -> Option<&str> {
        match self {
            Issue::TooFast { clip, .. } | Issue::TooMuchAcceleration { clip, .. } | Issue::Jump { clip, .. } => Some(clip),
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
            | Issue::KeepOut { frame, .. } => *frame,
        }
    }

    /// Speed and acceleration overruns can be accepted by the operator.
    /// Range, jump and keep-out problems cannot.
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
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Compiled {
    /// Pose on every frame from 0 to the song length inclusive.
    pub poses: Vec<Pose>,
    pub issues: Vec<Issue>,
}

impl Compiled {
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
    let ev = Evaluator::new(song);
    let n = song.length_frames.max(0) as usize;
    let poses: Vec<Pose> = (0..=n).map(|f| ev.pose_at(f as f64)).collect();
    let mut issues = Vec::new();

    for axis in Axis::ALL {
        let lim = project.axes.get(axis);
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

        for f in 0..=n {
            let fi = f as i64;
            let value = v(f);
            if value < lim.min - 1e-9 || value > lim.max + 1e-9 {
                let slot = ev.last_clip_index(axis, fi).unwrap_or(song.motion.len());
                let over = (lim.min - value).max(value - lim.max);
                if range[slot].is_none_or(|(_, worst)| over > worst) {
                    range[slot] = Some((fi, over));
                }
            }
            if f < n && !is_jump(fi + 1) {
                let speed = (v(f + 1) - value).abs() * fps;
                if speed > lim.max_speed * SLACK {
                    if let Some(c) = ev.clip_index_at(axis, fi) {
                        if fast[c].is_none_or(|(_, worst)| speed > worst) {
                            fast[c] = Some((fi, speed));
                        }
                    }
                }
            }
            if f > 0 && f < n && !is_jump(fi) && !is_jump(fi + 1) {
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
                    axis,
                    frame: *frame,
                    value: v(*frame as usize),
                });
            }
        }
    }

    for rule in &project.keep_out {
        if let Some(f) = poses.iter().position(|p| rule.violated(p)) {
            issues.push(Issue::KeepOut { frame: f as i64, rule: rule.describe() });
        }
    }

    issues.sort_by_key(|i| i.frame());
    Compiled { poses, issues }
}

/// True when an eased move of `frames` stays inside the speed and acceleration
/// limits, sampled per frame the same way `compile` does.
fn move_fits(from: f64, to: f64, ease: Ease, frames: i64, lim: &AxisLimits, fps: f64) -> bool {
    if from == to {
        return true;
    }
    let [k0, k1] = ease_keys(from, to, frames, ease);
    let at = |f: i64| segment_value(&k0, &k1, f.clamp(0, frames) as f64);
    for f in 0..=frames {
        let speed = (at(f + 1) - at(f)).abs() * fps;
        if speed > lim.max_speed * SLACK {
            return false;
        }
        let accel = (at(f + 1) - 2.0 * at(f) + at(f - 1)).abs() * fps * fps;
        if accel > lim.max_accel * SLACK {
            return false;
        }
    }
    true
}

/// Shortest whole-frame duration that every axis can make: the slowest axis's
/// travel at its top speed through the ease, plus that axis's settle time.
pub fn auto_duration(from: &Pose, to: &Pose, axes: &[Axis], ease: Ease, project: &Project) -> i64 {
    let rate = project.rate;
    let fps = rate.fps();
    let peak = peak_factor(ease);
    let mut seconds: f64 = 0.0;
    for &axis in axes {
        let lim = project.axes.get(axis);
        let travel = (to.get(axis) - from.get(axis)).abs();
        if travel > 1e-9 && lim.max_speed > 0.0 {
            seconds = seconds.max(travel * peak / lim.max_speed + lim.settle_s);
        }
    }
    if seconds == 0.0 {
        // Nothing moves. One second keeps the clip visible and grabbable.
        return rate.nominal();
    }
    let mut frames = ((seconds * fps) - 1e-9).ceil().max(1.0) as i64;
    let fits =
        |frames: i64| axes.iter().all(|&axis| move_fits(from.get(axis), to.get(axis), ease, frames, project.axes.get(axis), fps));
    let mut guard = 0;
    while !fits(frames) && guard < 400 {
        frames += (frames / 20).max(1);
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
    let ev = Evaluator::new(song);
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
    Some(auto_duration(&from, &to, &clip.axes, *ease, project))
}

/// Recompute every auto-timed move, earliest first. A move that has no room
/// before the next clip on its axes is clamped to the room, which then shows as
/// a too-fast issue instead of an overlap.
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
            .filter(|o| o.id != clip.id && o.start_frame >= clip.start_frame && o.axes.iter().any(|a| clip.has_axis(*a)))
            .map(|o| o.start_frame - clip.start_frame)
            .min()
            .unwrap_or(i64::MAX);
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
    use crate::pose::{Cmp, KeepOutRule};
    use crate::song::{LinkedTag, PresetRef};

    fn project() -> Project {
        let mut p = Project::default();
        p.axes.pan.max_speed = 60.0;
        p.axes.pan.max_accel = 2000.0;
        p.axes.pan.settle_s = 0.2;
        p
    }

    fn pan_move(start: i64, dur: i64, pan: f64, auto: bool) -> MotionClip {
        MotionClip {
            id: format!("c{start}"),
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
        let frames = auto_duration(&from, &to, &[Axis::Pan], Ease::EaseInOut, &p);
        let seconds = frames as f64 / 25.0;
        assert!((seconds - (1.72 + 0.2)).abs() < 0.06, "got {seconds}");

        let linear = auto_duration(&from, &to, &[Axis::Pan], Ease::Linear, &p);
        assert!(linear < frames);
    }

    #[test]
    fn slowest_axis_sets_the_time() {
        let p = Project::default();
        let from = Pose::home();
        let to = Pose { pan: 10.0, elevation: 430.0, ..Pose::home() };
        let both = auto_duration(&from, &to, &[Axis::Pan, Axis::Elevation], Ease::EaseInOut, &p);
        let lift = auto_duration(&from, &to, &[Axis::Elevation], Ease::EaseInOut, &p);
        let pan = auto_duration(&from, &to, &[Axis::Pan], Ease::EaseInOut, &p);
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

        p.keep_out.push(KeepOutRule {
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
}
