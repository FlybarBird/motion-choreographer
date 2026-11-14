//! Reduce per-frame recorded samples to bezier keys within a tolerance.

use crate::eval::keys_value;
use crate::pose::Axis;
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
            // Each still is made level at its own first sample. Two that shared a sample would be
            // level with each other, and a slow creep, which is a chain of them, would go flat.
            a = b + 1;
        } else {
            a = b.max(a + 1);
        }
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

    #[test]
    fn a_slow_creep_is_not_flattened() {
        // A slow push: 6 % of the zoom range over twelve minutes at 25 fps. Each frame moves far
        // less than the noise floor, so the pass reads as one still section after another.
        let samples: Vec<f64> = (0..18_000).map(|i| 0.2 + i as f64 * (0.06 / 18_000.0)).collect();
        let keys = fit_keys(&samples, 0.002);
        let e = max_error(&samples, &keys);
        assert!(e <= 0.002 + 1e-9, "the fitted pass is {e} away from what was recorded, {} keys", keys.len());
        assert!((keys.last().unwrap().value - samples[17_999]).abs() <= 0.002, "the push arrives where it was recorded");
        assert!(keys.windows(2).all(|w| w[0].frame < w[1].frame));
    }
}
