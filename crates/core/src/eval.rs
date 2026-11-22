//! `pose_at` is the single definition of where the rig should be. The monitor,
//! the compiler, the recorder and the hardware loop all use it.

use crate::pose::{Axis, Pose};
use crate::song::{ClipBody, Ease, FromPose, Interp, Key, MotionClip, Song};

/// Value of the segment between two keys at `frame` (same units as key frames).
pub fn segment_value(k0: &Key, k1: &Key, frame: f64) -> f64 {
    let f0 = k0.frame as f64;
    let f1 = k1.frame as f64;
    let len = f1 - f0;
    if len <= 0.0 || frame <= f0 {
        return k0.value;
    }
    if frame >= f1 {
        return k1.value;
    }
    match k0.interp {
        Interp::Hold => k0.value,
        Interp::Linear => k0.value + (k1.value - k0.value) * ((frame - f0) / len),
        Interp::Bezier => {
            let (p0, p1, p2, p3) = control_points(k0, k1);
            let t = solve_t(p0.0, p1.0, p2.0, p3.0, frame);
            cubic(p0.1, p1.1, p2.1, p3.1, t)
        }
    }
}

/// Control points of the bezier between two keys. Handle times are clamped so
/// time never runs backwards.
pub fn control_points(k0: &Key, k1: &Key) -> ((f64, f64), (f64, f64), (f64, f64), (f64, f64)) {
    let f0 = k0.frame as f64;
    let f1 = k1.frame as f64;
    let len = (f1 - f0).max(0.0);
    let out_x = k0.out_handle[0].clamp(0.0, len);
    let in_x = k1.in_handle[0].clamp(-len, 0.0);
    ((f0, k0.value), (f0 + out_x, k0.value + k0.out_handle[1]), (f1 + in_x, k1.value + k1.in_handle[1]), (f1, k1.value))
}

pub fn cubic(a: f64, b: f64, c: f64, d: f64, t: f64) -> f64 {
    let u = 1.0 - t;
    u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d
}

/// Solve `x(t) = x` for a monotonic cubic by bisection. Cannot diverge.
pub fn solve_t(x0: f64, x1: f64, x2: f64, x3: f64, x: f64) -> f64 {
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if cubic(x0, x1, x2, x3, mid) < x {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

/// Value of a key list at a clip-local frame. Holds before the first key and
/// after the last. `None` when there are no keys.
pub fn keys_value(keys: &[Key], local: f64) -> Option<f64> {
    let first = keys.first()?;
    if local <= first.frame as f64 {
        return Some(first.value);
    }
    let last = keys.last()?;
    if local >= last.frame as f64 {
        return Some(last.value);
    }
    let i = keys.partition_point(|k| (k.frame as f64) <= local);
    Some(segment_value(&keys[i - 1], &keys[i], local))
}

/// The two keys that reproduce a preset move's ease exactly.
pub fn ease_keys(from: f64, to: f64, duration: i64, ease: Ease) -> [Key; 2] {
    let d = duration as f64;
    let delta = to - from;
    match ease {
        Ease::Linear => [Key::new(0, from, Interp::Linear), Key::new(duration, to, Interp::Linear)],
        _ => {
            let (x1, y1, x2, y2) = ease.bezier();
            let mut k0 = Key::new(0, from, Interp::Bezier);
            k0.out_handle = [x1 * d, y1 * delta];
            let mut k1 = Key::new(duration, to, Interp::Bezier);
            k1.in_handle = [(x2 - 1.0) * d, (y2 - 1.0) * delta];
            [k0, k1]
        }
    }
}

/// Elevation preset moves always ease away from and back to rest.
pub(crate) fn preset_ease(axis: Axis, ease: Ease) -> Ease {
    if axis == Axis::Elevation {
        Ease::EaseInOut
    } else {
        ease
    }
}

/// Eased progress 0..1 at normalised time `t`.
pub fn ease_value(ease: Ease, t: f64) -> f64 {
    let [k0, k1] = ease_keys(0.0, 1.0, 1_000_000, ease);
    segment_value(&k0, &k1, t.clamp(0.0, 1.0) * 1_000_000.0)
}

/// Steepest slope of the ease curve: how much faster than a linear move the
/// peak speed is.
pub fn peak_factor(ease: Ease) -> f64 {
    use std::sync::OnceLock;
    static CACHE: OnceLock<[f64; 4]> = OnceLock::new();
    let table = CACHE.get_or_init(|| {
        let mut out = [1.0; 4];
        for (i, e) in Ease::ALL.into_iter().enumerate() {
            const N: usize = 4000;
            let mut prev = 0.0;
            let mut peak: f64 = 0.0;
            for s in 1..=N {
                let v = ease_value(e, s as f64 / N as f64);
                peak = peak.max((v - prev) * N as f64);
                prev = v;
            }
            out[i] = peak;
        }
        out
    });
    table[Ease::ALL.iter().position(|e| *e == ease).unwrap_or(0)]
}

enum SegKind<'a> {
    Move([Key; 2]),
    Keys(&'a [Key]),
}

struct Seg<'a> {
    start: i64,
    duration: i64,
    clip: usize,
    /// Value the axis holds before the clip.
    before: f64,
    /// Value the axis holds once the clip has ended.
    end: f64,
    kind: SegKind<'a>,
}

impl Seg<'_> {
    fn value(&self, local: f64) -> f64 {
        match &self.kind {
            SegKind::Move(keys) => segment_value(&keys[0], &keys[1], local),
            SegKind::Keys(keys) => keys_value(keys, local).unwrap_or(self.before),
        }
    }
}

/// Resolved, per-axis view of a song for repeated evaluation.
pub struct Evaluator<'a> {
    song: &'a Song,
    lanes: [Vec<Seg<'a>>; 5],
    /// Where each axis stands before its first move.
    rest: [f64; 5],
}

impl<'a> Evaluator<'a> {
    /// One camera's motion. Clip indices still index `song.motion`.
    pub fn new(song: &'a Song, camera: &str) -> Evaluator<'a> {
        let mut order: Vec<usize> = (0..song.motion.len()).filter(|i| song.motion[*i].camera == camera).collect();
        order.sort_by_key(|i| (song.motion[*i].start_frame, *i));
        // The camera's rest, unless its first move on the axis is a recorded pass: the rig was
        // already standing where the pass starts, so that is where the timeline has it wait.
        let rest_pose = song.rest_pose(camera);
        let rest = Axis::ALL.map(|axis| {
            let first = order.iter().map(|i| &song.motion[*i]).find(|c| c.has_axis(axis));
            let recorded = first.filter(|c| c.recorded_pass.is_some()).and_then(|c| c.keys(axis).first());
            recorded.map(|k| k.value).unwrap_or(rest_pose.get(axis))
        });
        let lanes = Axis::ALL.map(|axis| {
            let mut lane = Vec::new();
            let mut prev_end = rest[axis.index()];
            for &i in &order {
                let c = &song.motion[i];
                if !c.has_axis(axis) {
                    continue;
                }
                let seg = match &c.body {
                    ClipBody::PresetMove { from, to, ease, .. } => {
                        let start = match from {
                            FromPose::Linked(_) => prev_end,
                            FromPose::Pinned(p) => p.pose.get(axis),
                        };
                        let end = to.pose.get(axis);
                        Seg {
                            start: c.start_frame,
                            duration: c.duration_frames,
                            clip: i,
                            before: prev_end,
                            end,
                            kind: SegKind::Move(ease_keys(start, end, c.duration_frames, preset_ease(axis, *ease))),
                        }
                    }
                    ClipBody::Autofocus { .. } | ClipBody::AutoFollow { .. } => continue,
                    ClipBody::Keys { curves } => {
                        let keys = curves.get(&axis).map(|v| v.as_slice()).unwrap_or(&[]);
                        let end = keys.last().map(|k| k.value).unwrap_or(prev_end);
                        Seg {
                            start: c.start_frame,
                            duration: c.duration_frames,
                            clip: i,
                            before: prev_end,
                            end,
                            kind: SegKind::Keys(keys),
                        }
                    }
                };
                prev_end = seg.end;
                lane.push(seg);
            }
            lane
        });
        Evaluator { song, lanes, rest }
    }

    fn seg_at(&self, axis: Axis, frame: f64) -> Option<&Seg<'a>> {
        let lane = &self.lanes[axis.index()];
        let i = lane.partition_point(|s| (s.start as f64) <= frame);
        if i == 0 {
            None
        } else {
            Some(&lane[i - 1])
        }
    }

    pub fn axis_at(&self, axis: Axis, frame: f64) -> f64 {
        match self.seg_at(axis, frame) {
            None => self.rest[axis.index()],
            Some(s) => {
                let local = frame - s.start as f64;
                if local >= s.duration as f64 {
                    s.end
                } else {
                    s.value(local)
                }
            }
        }
    }

    pub fn pose_at(&self, frame: f64) -> Pose {
        Pose::from_fn(|a| self.axis_at(a, frame))
    }

    /// Where the axis waits before its first move: the start of a recorded pass, else the camera's rest.
    pub fn rest(&self, axis: Axis) -> f64 {
        self.rest[axis.index()]
    }

    /// Start frame of the axis's first move, if the song has one.
    pub fn first_start(&self, axis: Axis) -> Option<i64> {
        self.lanes[axis.index()].first().map(|s| s.start)
    }

    /// Index into `song.motion` of the clip driving `axis` at `frame`.
    pub fn clip_index_at(&self, axis: Axis, frame: i64) -> Option<usize> {
        self.seg_at(axis, frame as f64).filter(|s| frame < s.start + s.duration).map(|s| s.clip)
    }

    /// Index of the clip that most recently started driving `axis` at or before
    /// `frame`, whether or not it has ended.
    pub fn last_clip_index(&self, axis: Axis, frame: i64) -> Option<usize> {
        self.seg_at(axis, frame as f64).map(|s| s.clip)
    }

    /// Value the axis holds just before the given clip begins.
    pub fn value_before(&self, clip: &MotionClip, axis: Axis) -> f64 {
        self.lanes[axis.index()]
            .iter()
            .find(|s| self.song.motion[s.clip].id == clip.id)
            .map(|s| s.before)
            .unwrap_or_else(|| self.axis_at(axis, clip.start_frame as f64))
    }

    /// Value of the axis on the clip's first frame.
    pub fn value_at_start(&self, clip: &MotionClip, axis: Axis) -> f64 {
        self.lanes[axis.index()]
            .iter()
            .find(|s| self.song.motion[s.clip].id == clip.id)
            .map(|s| s.value(0.0))
            .unwrap_or_else(|| self.axis_at(axis, clip.start_frame as f64))
    }

    /// Clip starts on `axis` where the value steps: `(clip index, frame, distance)`.
    pub fn jumps(&self, axis: Axis) -> Vec<(usize, i64, f64)> {
        self.lanes[axis.index()]
            .iter()
            .filter_map(|s| {
                let d = (s.value(0.0) - s.before).abs();
                (d > 1e-9).then_some((s.clip, s.start, d))
            })
            .collect()
    }
}

/// Where a camera should be on a whole frame.
pub fn pose_at(song: &Song, camera: &str, frame: i64) -> Pose {
    Evaluator::new(song, camera).pose_at(frame as f64)
}

/// Where a camera should be between frames, for smooth output above the frame rate.
pub fn pose_at_f(song: &Song, camera: &str, frame: f64) -> Pose {
    Evaluator::new(song, camera).pose_at(frame)
}

/// Split a key list at a clip-local frame so both halves reproduce the original
/// curve exactly. The left half ends with a key at `at`; the right half starts
/// with one. Right-half frames stay in the original clip-local numbering.
pub fn split_keys(keys: &[Key], at: i64) -> (Vec<Key>, Vec<Key>) {
    if keys.is_empty() {
        return (Vec::new(), Vec::new());
    }
    if let Some(i) = keys.iter().position(|k| k.frame == at) {
        let mut left = keys[..=i].to_vec();
        let right = keys[i..].to_vec();
        if let Some(l) = left.last_mut() {
            l.out_handle = [0.0, 0.0];
        }
        return (left, right);
    }
    let first = keys[0];
    let last = keys[keys.len() - 1];
    if at < first.frame {
        let edge = Key::new(at, first.value, Interp::Hold);
        let mut right = vec![edge];
        right.extend_from_slice(keys);
        return (vec![Key::new(at, first.value, Interp::Hold)], right);
    }
    if at > last.frame {
        let mut left = keys.to_vec();
        if let Some(l) = left.last_mut() {
            l.interp = Interp::Hold;
        }
        left.push(Key::new(at, last.value, Interp::Hold));
        return (left, vec![Key::new(at, last.value, Interp::Hold)]);
    }
    let i = keys.partition_point(|k| k.frame < at);
    let k0 = keys[i - 1];
    let k1 = keys[i];
    let mut left = keys[..i].to_vec();
    let mut right = keys[i..].to_vec();
    match k0.interp {
        Interp::Hold => {
            left.push(Key::new(at, k0.value, Interp::Hold));
            right.insert(0, Key::new(at, k0.value, Interp::Hold));
        }
        Interp::Linear => {
            let v = segment_value(&k0, &k1, at as f64);
            left.push(Key::new(at, v, Interp::Linear));
            right.insert(0, Key::new(at, v, Interp::Linear));
        }
        Interp::Bezier => {
            let (p0, p1, p2, p3) = control_points(&k0, &k1);
            let t = solve_t(p0.0, p1.0, p2.0, p3.0, at as f64);
            let lerp = |a: (f64, f64), b: (f64, f64)| (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            let q0 = lerp(p0, p1);
            let q1 = lerp(p1, p2);
            let q2 = lerp(p2, p3);
            let r0 = lerp(q0, q1);
            let r1 = lerp(q1, q2);
            let s = lerp(r0, r1);
            let at_f = at as f64;
            if let Some(l) = left.last_mut() {
                l.out_handle = [q0.0 - p0.0, q0.1 - p0.1];
            }
            let mut mid_l = Key::new(at, s.1, Interp::Bezier);
            mid_l.in_handle = [r0.0 - at_f, r0.1 - s.1];
            left.push(mid_l);
            let mut mid_r = Key::new(at, s.1, Interp::Bezier);
            mid_r.out_handle = [r1.0 - at_f, r1.1 - s.1];
            right.insert(0, mid_r);
            right[1].in_handle = [q2.0 - p3.0, q2.1 - p3.1];
        }
    }
    (left, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::Preset;
    use crate::song::{LinkedTag, PresetRef};

    const CAM: &str = crate::camera::FIRST_CAMERA;

    fn preset(name: &str, pan: f64) -> Preset {
        Preset::new(&name.to_lowercase(), name, Pose { pan, tilt: 1.0, zoom: 0.2, focus: 0.5, elevation: 100.0 })
    }

    fn mv(id: &str, start: i64, dur: i64, to: &Preset) -> MotionClip {
        MotionClip {
            id: id.into(),
            camera: CAM.into(),
            start_frame: start,
            duration_frames: dur,
            axes: Axis::ALL.to_vec(),
            body: ClipBody::PresetMove {
                auto_duration: false,
                from: FromPose::Linked(LinkedTag::Linked),
                to: PresetRef::of(to),
                ease: Ease::EaseInOut,
            },
            accepted: None,
            recorded_pass: None,
        }
    }

    #[test]
    fn gaps_hold_and_home_applies_before_the_first_clip() {
        let mut song = Song::new("s", "S", "01:00:00:00", 1000);
        song.motion.push(mv("a", 100, 50, &preset("A", 30.0)));
        assert_eq!(pose_at(&song, CAM, 0).pan, 0.0);
        assert_eq!(pose_at(&song, CAM, 100).pan, 0.0);
        assert!((pose_at(&song, CAM, 125).pan - 15.0).abs() < 1e-9);
        assert_eq!(pose_at(&song, CAM, 150).pan, 30.0);
        assert_eq!(pose_at(&song, CAM, 900).pan, 30.0);
        assert_eq!(pose_at(&song, CAM, 900).focus, 0.5);
    }

    fn keyed(id: &str, start: i64, pan: [f64; 2], pass: Option<&str>) -> MotionClip {
        let keys = vec![Key::new(0, pan[0], Interp::Linear), Key::new(100, pan[1], Interp::Linear)];
        MotionClip {
            id: id.into(),
            camera: CAM.into(),
            start_frame: start,
            duration_frames: 100,
            axes: vec![Axis::Pan],
            body: ClipBody::Keys { curves: [(Axis::Pan, keys)].into() },
            accepted: None,
            recorded_pass: pass.map(str::to_string),
        }
    }

    #[test]
    fn a_camera_stands_where_its_first_recorded_move_starts() {
        // A pass recorded from frame 200 with the head already at 25 degrees.
        let mut song = Song::new("s", "S", "01:00:00:00", 1000);
        song.motion.push(keyed("take", 200, [25.0, 40.0], Some("pass-0001")));
        assert_eq!(pose_at(&song, CAM, 0).pan, 25.0, "the camera waits at the start of the take, not at home");
        assert_eq!(pose_at(&song, CAM, 199).pan, 25.0);
        assert_eq!(pose_at(&song, CAM, 0).tilt, 0.0, "an axis the take does not drive stays at home");
        let ev = Evaluator::new(&song, CAM);
        assert!(ev.jumps(Axis::Pan).is_empty());
        assert_eq!(ev.value_before(&song.motion[0], Axis::Pan), 25.0);

        // A second take that starts somewhere else is still a step in the middle of the song.
        song.motion.push(keyed("later", 500, [-10.0, 0.0], Some("pass-0002")));
        assert_eq!(Evaluator::new(&song, CAM).jumps(Axis::Pan), vec![(1, 500, 50.0)]);

        // Keys made by hand say where the camera goes, not where it was: before them it is at home.
        let mut by_hand = Song::new("s", "S", "01:00:00:00", 1000);
        by_hand.motion.push(keyed("keys", 200, [25.0, 40.0], None));
        assert_eq!(pose_at(&by_hand, CAM, 199).pan, 0.0);
        assert_eq!(Evaluator::new(&by_hand, CAM).jumps(Axis::Pan), vec![(0, 200, 25.0)]);
    }

    #[test]
    fn linked_from_follows_an_earlier_edit() {
        let mut song = Song::new("s", "S", "01:00:00:00", 1000);
        song.motion.push(mv("a", 100, 50, &preset("A", 30.0)));
        song.motion.push(mv("b", 300, 50, &preset("B", -10.0)));
        assert_eq!(pose_at(&song, CAM, 300).pan, 30.0);
        if let ClipBody::PresetMove { to, .. } = &mut song.motion[0].body {
            to.pose.pan = 50.0;
        }
        assert_eq!(pose_at(&song, CAM, 300).pan, 50.0);
        assert_eq!(pose_at(&song, CAM, 350).pan, -10.0);
        // Pinning From stops it following.
        if let ClipBody::PresetMove { from, .. } = &mut song.motion[1].body {
            *from = FromPose::Pinned(PresetRef::of(&preset("C", 5.0)));
        }
        assert_eq!(pose_at(&song, CAM, 300).pan, 5.0);
    }

    #[test]
    fn each_camera_follows_only_its_own_clips() {
        let mut song = Song::new("s", "S", "01:00:00:00", 1000);
        song.motion.push(mv("a", 100, 50, &preset("A", 30.0)));
        let mut other = mv("b", 120, 50, &preset("B", -40.0));
        other.camera = "cam2".into();
        song.motion.push(other);
        song.motion.push(mv("c", 300, 50, &preset("C", 10.0)));
        assert_eq!(pose_at(&song, CAM, 200).pan, 30.0);
        assert_eq!(pose_at(&song, "cam2", 200).pan, -40.0);
        assert_eq!(pose_at(&song, "cam2", 119).pan, 0.0, "the second camera starts from home, not from the first camera's move");
        // The first camera's next move is linked to its own last pose.
        assert_eq!(pose_at(&song, CAM, 300).pan, 30.0);
        assert_eq!(pose_at(&song, "cam2", 900).pan, -40.0);
        let ev = Evaluator::new(&song, "cam2");
        assert_eq!(ev.clip_index_at(Axis::Pan, 130), Some(1));
        assert_eq!(ev.clip_index_at(Axis::Pan, 110), None);
        assert_eq!(ev.value_before(&song.motion[1], Axis::Pan), 0.0);
        // A camera with no clips holds the song's home pose.
        assert_eq!(pose_at(&song, "cam7", 500), song.home_pose);
    }

    #[test]
    fn ease_shapes() {
        assert!((ease_value(Ease::Linear, 0.3) - 0.3).abs() < 1e-9);
        assert!((ease_value(Ease::EaseInOut, 0.5) - 0.5).abs() < 1e-9);
        assert!(ease_value(Ease::EaseIn, 0.25) < 0.25);
        assert!(ease_value(Ease::EaseOut, 0.25) > 0.25);
        assert!((peak_factor(Ease::Linear) - 1.0).abs() < 1e-6);
        assert!((peak_factor(Ease::EaseInOut) - 1.72).abs() < 0.01);
    }

    #[test]
    fn bezier_time_is_monotonic_for_random_handles() {
        let mut seed = 0x1234_5678_9abc_def0u64;
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        for _ in 0..300 {
            let mut k0 = Key::new(0, rnd() * 10.0, Interp::Bezier);
            let mut k1 = Key::new(40, rnd() * 10.0, Interp::Bezier);
            // Handles far outside the legal range on purpose.
            k0.out_handle = [rnd() * 200.0 - 50.0, rnd() * 40.0 - 20.0];
            k1.in_handle = [rnd() * 200.0 - 150.0, rnd() * 40.0 - 20.0];
            let (p0, p1, p2, p3) = control_points(&k0, &k1);
            let mut prev = -1.0;
            for f in 0..=400 {
                let frame = f as f64 / 10.0;
                let t = solve_t(p0.0, p1.0, p2.0, p3.0, frame);
                assert!(t >= prev, "time ran backwards");
                assert!(segment_value(&k0, &k1, frame).is_finite());
                prev = t;
            }
        }
    }

    #[test]
    fn split_keeps_the_curve() {
        let mut k0 = Key::new(0, -12.0, Interp::Bezier);
        k0.out_handle = [30.0, 4.0];
        let mut k1 = Key::new(100, 20.0, Interp::Bezier);
        k1.in_handle = [-45.0, -9.0];
        let k2 = Key::new(160, 5.0, Interp::Linear);
        let keys = vec![k0, k1, k2];
        for at in [37, 100, 130] {
            let (l, r) = split_keys(&keys, at);
            for f in 0..=160 {
                let orig = keys_value(&keys, f as f64).unwrap();
                let got = if f <= at { keys_value(&l, f as f64) } else { keys_value(&r, f as f64) }.unwrap();
                assert!((orig - got).abs() < 1e-7, "frame {f} at split {at}: {orig} vs {got}");
            }
        }
    }

    #[test]
    fn a_new_song_rests_each_camera_at_its_stored_default_and_an_old_song_does_not() {
        use crate::camera::CameraConfig;
        use crate::compile::compile;
        use crate::pose::Project;

        let stored = Pose { pan: 18.5, tilt: -6.0, zoom: 0.5, focus: 0.25, elevation: 240.0 };
        let mut project = Project::default();
        project.cameras[0].default_pose = Some(stored);
        project.cameras.push(CameraConfig::new("cam2", "Centre"));

        let mut song = Song::new("new", "New", "01:00:00:00", 500);
        song.seed_defaults(&project.cameras);
        assert_eq!(pose_at(&song, CAM, 0), stored);
        assert_eq!(pose_at(&song, CAM, 499), stored);
        assert_eq!(pose_at(&song, "cam2", 0), Pose::home());
        assert_eq!(song.home_pose, Pose::home());

        // The first move leaves from the stored default, not from pan 0.
        song.motion.push(mv("a", 100, 50, &preset("A", 40.0)));
        assert_eq!(pose_at(&song, CAM, 50).pan, stored.pan);
        assert_eq!(Evaluator::new(&song, CAM).value_before(&song.motion[0], Axis::Pan), stored.pan);
        assert_eq!(pose_at(&song, "cam2", 50), Pose::home());

        let compiled = compile(&song, &project);
        assert_eq!(compiled.poses(CAM)[0], pose_at(&song, CAM, 0));
        assert!(compiled.poses("cam2").iter().all(|pose| *pose == Pose::home()));

        // Nothing programmed: the whole song is the default, and the other camera stays at home.
        let mut idle = Song::new("idle", "Idle", "01:00:00:00", 10);
        idle.seed_defaults(&project.cameras);
        let compiled = compile(&idle, &project);
        assert!(compiled.issues.is_empty(), "{:?}", compiled.issues);
        assert!(compiled.poses(CAM).iter().all(|pose| *pose == stored));
        assert!(compiled.poses("cam2").iter().all(|pose| *pose == Pose::home()));

        // A song from before defaults, including one whose home was set, is unchanged.
        let mut old = Song::new("old", "Old", "01:00:00:00", 10);
        old.home_pose = Pose { pan: 3.0, tilt: 1.0, ..Pose::home() };
        assert!(old.camera_homes.is_empty());
        assert_eq!(pose_at(&old, CAM, 4), old.home_pose);
        assert!(compile(&old, &project).poses("cam2").iter().all(|pose| *pose == old.home_pose));

        // Seeding copies what is stored then. Changing the camera later does not rewrite the song.
        project.cameras[0].default_pose = Some(Pose { pan: 1.0, ..Pose::home() });
        assert_eq!(pose_at(&idle, CAM, 0), stored);

        let legacy: Song = serde_json::from_str(
            r#"{"id":"s","name":"S","start_tc":"01:00:00:00","length_frames":10,"home_pose":{"pan":0,"tilt":0,"zoom":0,"focus":0.5,"elevation":0}}"#,
        )
        .unwrap();
        assert!(legacy.camera_homes.is_empty());
        assert_eq!(legacy.rest_pose(CAM), Pose::home());
        assert!(serde_json::to_value(Song::new("s", "S", "01:00:00:00", 10)).unwrap().get("camera_homes").is_none());
        let written = serde_json::to_value(&idle).unwrap();
        let home = &written["camera_homes"]["cam1"];
        assert_eq!(home["pan"].as_f64(), Some(18.5));
        assert_eq!(home["tilt"].as_f64(), Some(-6.0));
        assert_eq!(home["zoom"].as_f64(), Some(0.5));
        assert_eq!(home["focus"].as_f64(), Some(0.25));
        assert_eq!(home["elevation"].as_f64(), Some(240.0));
        assert!(written["camera_homes"].get("cam2").is_none());
    }
}
