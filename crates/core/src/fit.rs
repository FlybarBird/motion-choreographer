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

fn slope(samples: &[f64], i: usize) -> f64 {
    let n = samples.len();
    if n < 2 {
        return 0.0;
    }
    let lo = i.saturating_sub(2);
    let hi = (i + 2).min(n - 1);
    (samples[hi] - samples[lo]) / (hi - lo) as f64
}

fn segment_keys(samples: &[f64], a: usize, b: usize) -> (Key, Key) {
    let len = (b - a) as f64;
    let mut k0 = Key::new(a as i64, samples[a], Interp::Bezier);
    let mut k1 = Key::new(b as i64, samples[b], Interp::Bezier);
    // Handles a third of the way along make time linear in the curve
    // parameter, so the segment is a cubic Hermite through the two samples.
    k0.out_handle = [len / 3.0, slope(samples, a) * len / 3.0];
    k1.in_handle = [-len / 3.0, -slope(samples, b) * len / 3.0];
    (k0, k1)
}

fn worst(samples: &[f64], a: usize, b: usize) -> (usize, f64) {
    let (k0, k1) = segment_keys(samples, a, b);
    let pair = [k0, k1];
    let mut at = a;
    let mut err = 0.0;
    for (i, s) in samples.iter().enumerate().take(b).skip(a + 1) {
        let e = (keys_value(&pair, i as f64).unwrap_or(*s) - s).abs();
        if e > err {
            err = e;
            at = i;
        }
    }
    (at, err)
}

/// Fit keys to one value per frame. Key frames are indices into `samples`.
/// Every sample is reproduced within `tolerance`; a still section becomes two keys.
pub fn fit_keys(samples: &[f64], tolerance: f64) -> Vec<Key> {
    match samples.len() {
        0 => return Vec::new(),
        1 => return vec![Key::new(0, samples[0], Interp::Bezier)],
        _ => {}
    }
    let mut cuts = vec![0usize, samples.len() - 1];
    let mut stack = vec![(0usize, samples.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        if b - a < 2 {
            continue;
        }
        let (at, err) = worst(samples, a, b);
        if err > tolerance {
            cuts.push(at);
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut keys: Vec<Key> = Vec::with_capacity(cuts.len());
    for w in cuts.windows(2) {
        let (k0, k1) = segment_keys(samples, w[0], w[1]);
        match keys.last_mut() {
            Some(last) => last.out_handle = k0.out_handle,
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
        samples
            .iter()
            .enumerate()
            .map(|(i, s)| (keys_value(keys, i as f64).unwrap() - s).abs())
            .fold(0.0, f64::max)
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
        assert!(keys.len() < 20);
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
