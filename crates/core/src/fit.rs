//! Reduce per-frame recorded samples to bezier keys within a tolerance.

use crate::compile::SLACK;
use crate::eval::keys_value;
use crate::pose::{Axis, AxisLimits};
use crate::song::{Interp, Key};

/// Default fit tolerance per axis, in axis units.
pub fn default_tolerance(axis: Axis) -> f64 {
    match axis {
        Axis::Pan | Axis::Tilt => 0.05,
        Axis::Zoom | Axis::Focus => 0.002,
        Axis::Elevation => 1.0,
    }
}

/// Slope at `i` from the samples either side of it.
fn central(samples: &[f64], i: usize) -> f64 {
    let n = samples.len();
    if n < 2 {
        return 0.0;
    }
    let lo = i.saturating_sub(2);
    let hi = (i + 2).min(n - 1);
    (samples[hi] - samples[lo]) / (hi - lo) as f64
}

/// Slope at `from` looking only toward `toward`. At a corner (a move that
/// starts or stops abruptly) this is the slope the segment actually has.
fn inward(samples: &[f64], from: usize, toward: usize) -> f64 {
    let j = if toward > from { (from + 2).min(toward) } else { from.saturating_sub(2).max(toward) };
    if j == from {
        return 0.0;
    }
    (samples[j] - samples[from]) / (j as f64 - from as f64)
}

fn hermite(samples: &[f64], a: usize, b: usize, slope_a: f64, slope_b: f64) -> [Key; 2] {
    let len = (b - a) as f64;
    let mut k0 = Key::new(a as i64, samples[a], Interp::Bezier);
    let mut k1 = Key::new(b as i64, samples[b], Interp::Bezier);
    // Handles a third of the way along make time linear in the curve
    // parameter, so the segment is a cubic Hermite through the two samples.
    k0.out_handle = [len / 3.0, slope_a * len / 3.0];
    k1.in_handle = [-len / 3.0, -slope_b * len / 3.0];
    [k0, k1]
}

fn worst(samples: &[f64], pair: &[Key; 2], a: usize, b: usize) -> (usize, f64) {
    let mut at = a;
    let mut err = 0.0;
    for (i, s) in samples.iter().enumerate().take(b).skip(a + 1) {
        let e = (keys_value(pair, i as f64).unwrap_or(*s) - s).abs();
        if e > err {
            err = e;
            at = i;
        }
    }
    (at, err)
}

/// The two keys for `a..=b`, where the fit is worst, and by how much. Each end
/// takes the smooth slope or the one-sided slope, whichever fits better, so a
/// corner gets a broken handle instead of a cluster of keys.
fn segment(samples: &[f64], a: usize, b: usize) -> ([Key; 2], usize, f64) {
    let mut best = (hermite(samples, a, b, central(samples, a), central(samples, b)), a, f64::INFINITY);
    for slope_a in [central(samples, a), inward(samples, a, b)] {
        for slope_b in [central(samples, b), inward(samples, b, a)] {
            let pair = hermite(samples, a, b, slope_a, slope_b);
            let (at, err) = worst(samples, &pair, a, b);
            if err < best.2 {
                best = (pair, at, err);
            }
        }
    }
    best
}

/// A rig at rest for at least this many frames is a still section, not part of a move.
const MIN_STILL: usize = 6;

/// Runs of samples that do not move, as inclusive index pairs.
fn stills(samples: &[f64], tolerance: f64) -> Vec<(usize, usize)> {
    let eps = tolerance * 0.02;
    let mut runs = Vec::new();
    let mut a = 0;
    while a + 1 < samples.len() {
        let mut b = a;
        while b + 1 < samples.len() && (samples[b + 1] - samples[a]).abs() <= eps {
            b += 1;
        }
        if b - a >= MIN_STILL {
            runs.push((a, b));
        }
        a = b.max(a + 1);
    }
    runs
}

/// Fit keys to one value per frame. Key frames are indices into `samples`.
/// Every sample is reproduced within `tolerance`; a still section becomes two
/// keys with a flat line between them.
pub fn fit_keys(samples: &[f64], tolerance: f64) -> Vec<Key> {
    match samples.len() {
        0 => return Vec::new(),
        1 => return vec![Key::new(0, samples[0], Interp::Bezier)],
        _ => {}
    }
    let last = samples.len() - 1;
    let still = stills(samples, tolerance);
    let is_still = |a: usize, b: usize| still.iter().any(|(s, e)| *s <= a && b <= *e);
    // The ends of the pass and of every still section are keys whatever the fit says.
    let mut fixed = vec![0, last];
    fixed.extend(still.iter().flat_map(|(a, b)| [*a, *b]));
    fixed.sort_unstable();
    fixed.dedup();

    let mut cuts = fixed.clone();
    let mut stack: Vec<(usize, usize)> = fixed.windows(2).map(|w| (w[0], w[1])).filter(|(a, b)| !is_still(*a, *b)).collect();
    while let Some((a, b)) = stack.pop() {
        if b - a < 2 {
            continue;
        }
        let (_, at, err) = segment(samples, a, b);
        if err > tolerance {
            cuts.push(at);
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    // Splitting at the worst sample overshoots; drop every cut the fit does not need.
    loop {
        let before = cuts.len();
        let mut i = 1;
        while i + 1 < cuts.len() {
            if fixed.binary_search(&cuts[i]).is_err() && segment(samples, cuts[i - 1], cuts[i + 1]).2 <= tolerance {
                cuts.remove(i);
            } else {
                i += 1;
            }
        }
        if cuts.len() == before {
            break;
        }
    }
    let mut keys: Vec<Key> = Vec::with_capacity(cuts.len());
    for w in cuts.windows(2) {
        let [k0, mut k1] =
            if is_still(w[0], w[1]) { hermite(samples, w[0], w[1], 0.0, 0.0) } else { segment(samples, w[0], w[1]).0 };
        match keys.last_mut() {
            Some(prev) => {
                prev.out_handle = k0.out_handle;
                if is_still(w[0], w[1]) {
                    // Dead level, not level within the noise floor.
                    k1.value = prev.value;
                }
            }
            None => keys.push(k0),
        }
        keys.push(k1);
    }
    keys
}

/// How far smoothing may spread a move in time: the width of the blur, in seconds.
const SMOOTH_SPREAD_S: f64 = 0.25;
/// Smoothing aims a little inside the limits, so the keys fitted to it stay inside them too.
const SMOOTH_MARGIN: f64 = 0.9;

/// A move with the frame before it and the frame after it: on the timeline the rig
/// stands still until a move begins and from where it ends.
fn held(samples: &[f64]) -> Vec<f64> {
    let mut values = Vec::with_capacity(samples.len() + 2);
    values.extend(samples.first());
    values.extend_from_slice(samples);
    values.extend(samples.last());
    values
}

/// True when a move, one value per frame, breaks the speed or acceleration limit, measured
/// frame to frame the way the compile measures it.
pub fn overruns(samples: &[f64], limits: &AxisLimits, fps: f64) -> bool {
    let values = held(samples);
    let fast = values.windows(2).any(|w| (w[1] - w[0]).abs() * fps > limits.max_speed * SLACK);
    fast || values.windows(3).any(|w| (w[2] - 2.0 * w[1] + w[0]).abs() * fps * fps > limits.max_accel * SLACK)
}

/// Smooth a move, one value per frame, until its speed and acceleration are inside the
/// limits. Only the stretches that break a limit are changed, each by as little as brings
/// it inside, so what already fits is returned as it was and smoothing twice changes nothing
/// more. The first and last value never move; a take that starts or ends with the rig
/// moving is eased away from them. A move that stays too fast for longer than the blur can
/// spread is left over the limit.
pub fn smooth_to_limits(samples: &[f64], limits: &AxisLimits, fps: f64) -> Vec<f64> {
    if samples.len() < 3 {
        return samples.to_vec();
    }
    let mut values = held(samples);
    let n = values.len();
    let step = limits.max_speed / fps * SMOOTH_MARGIN;
    let bend = limits.max_accel / (fps * fps) * SMOOTH_MARGIN;
    // Each pass widens the blur a little: this many reach the widest it may get.
    let passes = (2.0 * (SMOOTH_SPREAD_S * fps).powi(2)).ceil() as usize;
    let mut over = vec![false; n];
    let mut was = values.clone();
    for _ in 0..passes {
        // A take that begins or ends with the rig already moving: on the timeline the rig is at
        // rest there, so from each end the steps grow no faster than the rig can pick up speed.
        // Held a hair inside what the check below allows, so rounding never sets the two at odds,
        // and reaching no further in than the blur may spread.
        let furthest = (2.0 * SMOOTH_SPREAD_S * fps).ceil() as i64;
        for (end, inward) in [(1, 1), (n as i64 - 2, -1)] {
            for k in 1..(n as i64 - 3).min(furthest) {
                let (a, b) = ((end + inward * (k - 1)) as usize, (end + inward * k) as usize);
                let (reach, first) = ((k as f64 * bend).min(step) * 0.999, values[b] - values[a]);
                if first.abs() <= reach {
                    break;
                }
                values[b] = values[a] + first.clamp(-reach, reach);
            }
        }
        over.fill(false);
        for f in 0..n - 1 {
            if (values[f + 1] - values[f]).abs() > step {
                over[f] = true;
                over[f + 1] = true;
            }
        }
        for f in 1..n - 1 {
            if (values[f + 1] - 2.0 * values[f] + values[f - 1]).abs() > bend {
                over[f - 1..=f + 1].fill(true);
            }
        }
        if !over.contains(&true) {
            break;
        }
        was.copy_from_slice(&values);
        // The ends of the move stay where they are, so it still meets what is either side of it.
        for f in 2..n - 2 {
            if over[f] {
                values[f] = 0.25 * was[f - 1] + 0.5 * was[f] + 0.25 * was[f + 1];
            }
        }
    }
    values[1..n - 1].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::ease_value;
    use crate::song::Ease;

    fn max_error(samples: &[f64], keys: &[Key]) -> f64 {
        samples.iter().enumerate().map(|(i, s)| (keys_value(keys, i as f64).unwrap() - s).abs()).fold(0.0, f64::max)
    }

    #[test]
    fn sine_is_reproduced_within_tolerance_with_few_keys() {
        let samples: Vec<f64> = (0..500).map(|i| 30.0 * (i as f64 * 0.02).sin()).collect();
        let keys = fit_keys(&samples, 0.05);
        assert!(max_error(&samples, &keys) <= 0.05 + 1e-9);
        assert!(keys.len() < 60, "{} keys", keys.len());
        assert!(keys.windows(2).all(|w| w[0].frame < w[1].frame));
    }

    #[test]
    fn ease_in_out_is_reproduced() {
        let samples: Vec<f64> = (0..=120).map(|i| 45.0 * ease_value(Ease::EaseInOut, i as f64 / 120.0)).collect();
        let keys = fit_keys(&samples, 0.05);
        assert!(max_error(&samples, &keys) <= 0.05 + 1e-9);
        assert!(keys.len() <= 12, "{} keys", keys.len());
    }

    #[test]
    fn a_still_section_becomes_two_keys() {
        let samples = vec![12.5; 400];
        let keys = fit_keys(&samples, 0.05);
        assert_eq!(keys.len(), 2);
        assert_eq!((keys[0].frame, keys[1].frame), (0, 399));

        // Still, then a move, then still.
        let mut s = vec![0.0; 100];
        s.extend((0..=100).map(|i| 20.0 * ease_value(Ease::EaseInOut, i as f64 / 100.0)));
        s.extend(vec![20.0; 100]);
        let keys = fit_keys(&s, 0.05);
        assert!(max_error(&s, &keys) <= 0.05 + 1e-9);
        assert!(keys.len() <= 8, "{} keys", keys.len());
        // The stills are dead level between their two keys, not level within the tolerance.
        assert_eq!((keys[0].frame, keys[1].frame), (0, 100));
        assert!((0..=100).all(|i| keys_value(&keys, i as f64) == Some(0.0)));
        let rest = keys_value(&keys, 299.0).unwrap();
        assert!((rest - 20.0).abs() < 0.002);
        assert!((200..300).all(|i| (keys_value(&keys, i as f64).unwrap() - rest).abs() < 1e-12));
    }

    #[test]
    fn a_move_that_starts_and_stops_abruptly_gets_corner_keys() {
        // Still, a constant-speed ramp, still: the shape of a jog.
        let mut s = vec![4.0; 101];
        s.extend((1..=100).map(|i| 4.0 + 0.4 * i as f64));
        s.extend(vec![44.0; 60]);
        let keys = fit_keys(&s, 0.05);
        assert!(max_error(&s, &keys) <= 0.05 + 1e-9);
        assert_eq!(keys.iter().map(|k| k.frame).collect::<Vec<_>>(), vec![0, 100, 200, 260]);
        // The corner key is flat coming in and at ramp speed going out.
        let corner = keys[1];
        assert!(corner.in_handle[1].abs() < 1e-9);
        assert!((corner.out_handle[1] / corner.out_handle[0] - 0.4).abs() < 1e-9);
    }

    /// A move as a real head reports it: read at uneven moments, each frame holding the last
    /// reading, so the take stands for a frame and then jumps to catch up.
    fn late_readings(truth: impl Fn(f64) -> f64, frames: usize) -> Vec<f64> {
        let mut seed = 7u32;
        let (mut next, mut held) = (0.0, truth(0.0));
        (0..frames)
            .map(|f| {
                while next <= f as f64 {
                    held = truth(next);
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    next += 0.4 + 1.6 * (seed >> 8) as f64 / (1 << 24) as f64;
                }
                held
            })
            .collect()
    }

    const FR7: AxisLimits = AxisLimits { min: -170.0, max: 170.0, max_speed: 60.0, max_accel: 240.0, settle_s: 0.2 };

    #[test]
    fn a_take_made_of_late_readings_is_smoothed_inside_the_limits_and_only_where_it_broke_them() {
        // Still, a 40 degree pan in two seconds, still: well inside what the head can do.
        let truth = |f: f64| -13.3 - 40.0 * ease_value(Ease::EaseInOut, (f - 100.0) / 60.0);
        let clean: Vec<f64> = (0..300).map(|f| truth(f as f64)).collect();
        assert!(!overruns(&clean, &FR7, 30.0));
        assert_eq!(smooth_to_limits(&clean, &FR7, 30.0), clean, "a move the rig can make is not touched");

        let take = late_readings(truth, 300);
        assert!(overruns(&take, &FR7, 30.0), "the readings alone make the pan look too fast and too sharp");
        let smooth = smooth_to_limits(&take, &FR7, 30.0);
        assert!(!overruns(&smooth, &FR7, 30.0));
        assert_eq!((smooth[0], smooth[299]), (take[0], take[299]), "it starts and ends where the take does");
        assert!(smooth[..95] == take[..95] && smooth[170..] == take[170..], "the stills either side are as recorded");
        let off = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max);
        assert!(off(&smooth, &take) < 2.5, "{} degrees from the take", off(&smooth, &take));
        assert!(off(&smooth, &clean) <= off(&take, &clean), "smoothing is no further from what the head really did");
        assert_eq!(smooth_to_limits(&smooth, &FR7, 30.0), smooth, "smoothing again changes nothing");
        // The keys fitted to it play inside the limits too.
        let keys = fit_keys(&smooth, 0.025);
        let played: Vec<f64> = (0..300).map(|f| keys_value(&keys, f as f64).unwrap()).collect();
        assert!(!overruns(&played, &FR7, 30.0));
    }

    #[test]
    fn a_move_too_fast_for_too_long_is_left_over_the_limit_and_one_that_starts_moving_is_eased_in() {
        // Three seconds at one and a half times the head's top speed: no blur short of seconds hides that.
        let mut fast = vec![-80.0; 30];
        fast.extend((1..=90).map(|i| -80.0 + 3.0 * i as f64));
        fast.extend(vec![190.0; 30]);
        let smooth = smooth_to_limits(&fast, &FR7, 30.0);
        assert!(overruns(&smooth, &FR7, 30.0));
        assert_eq!((smooth[0], smooth[149]), (-80.0, 190.0));
        assert!(smooth.iter().all(|v| (-80.0..=190.0).contains(v)), "smoothing never reaches past the take");

        // A take that begins with the head already moving: on the timeline the rig stands
        // still before it, so the first frames are an abrupt start.
        let moving: Vec<f64> = (0..120).map(|i| 0.5 * i as f64).collect();
        assert!(overruns(&moving, &FR7, 30.0));
        let eased = smooth_to_limits(&moving, &FR7, 30.0);
        assert!(!overruns(&eased, &FR7, 30.0));
        assert_eq!((eased[0], eased[119]), (moving[0], moving[119]));
    }

    #[test]
    fn noise_never_loops_forever() {
        let mut seed = 7u32;
        let samples: Vec<f64> = (0..300)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f64 / (1 << 24) as f64
            })
            .collect();
        let keys = fit_keys(&samples, 0.01);
        assert!(max_error(&samples, &keys) <= 0.01 + 1e-9);
        assert!(keys.len() <= 300);
    }
}
