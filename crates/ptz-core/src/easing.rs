//! Interpolation and easing for PTZ/pedestal moves.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EaseKind {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
    Smooth, // smootherstep
}

impl EaseKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Linear => "Linear",
            Self::EaseIn => "Ease In",
            Self::EaseOut => "Ease Out",
            Self::EaseInOut => "Ease In-Out",
            Self::Smooth => "Smooth",
        }
    }

    pub fn all() -> &'static [EaseKind] {
        &[
            Self::Linear,
            Self::EaseIn,
            Self::EaseOut,
            Self::EaseInOut,
            Self::Smooth,
        ]
    }
}

/// Map linear t in [0,1] through easing curve → [0,1].
pub fn ease(kind: EaseKind, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match kind {
        EaseKind::Linear => t,
        EaseKind::EaseIn => t * t,
        EaseKind::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        EaseKind::EaseInOut => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
            }
        }
        EaseKind::Smooth => {
            // Ken Perlin smootherstep
            t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
        }
    }
}

/// Interpolate scalar with easing.
pub fn lerp_eased(a: f64, b: f64, t: f64, kind: EaseKind) -> f64 {
    a + (b - a) * ease(kind, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints() {
        for kind in EaseKind::all() {
            assert!((ease(*kind, 0.0) - 0.0).abs() < 1e-12);
            assert!((ease(*kind, 1.0) - 1.0).abs() < 1e-12);
        }
    }

    #[test]
    fn linear_mid() {
        assert!((ease(EaseKind::Linear, 0.5) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn ease_in_below_linear() {
        assert!(ease(EaseKind::EaseIn, 0.5) < 0.5);
    }

    #[test]
    fn ease_out_above_linear() {
        assert!(ease(EaseKind::EaseOut, 0.5) > 0.5);
    }

    #[test]
    fn lerp() {
        assert!((lerp_eased(10.0, 20.0, 0.5, EaseKind::Linear) - 15.0).abs() < 1e-12);
    }
}
