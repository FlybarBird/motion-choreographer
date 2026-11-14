//! SMPTE timecode math including drop-frame 29.97 and signed offsets.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum TimecodeError {
    #[error("invalid timecode string: {0}")]
    InvalidString(String),
    #[error("frame out of range for rate")]
    FrameOutOfRange,
}

/// Supported project frame rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameRate {
    Fps23976,
    Fps24,
    Fps25,
    Fps2997Df,
    Fps30,
}

impl FrameRate {
    pub fn label(self) -> &'static str {
        match self {
            Self::Fps23976 => "23.976",
            Self::Fps24 => "24",
            Self::Fps25 => "25",
            Self::Fps2997Df => "29.97DF",
            Self::Fps30 => "30",
        }
    }

    pub fn all() -> &'static [FrameRate] {
        &[
            Self::Fps23976,
            Self::Fps24,
            Self::Fps25,
            Self::Fps2997Df,
            Self::Fps30,
        ]
    }

    /// Nominal integer frames-per-second used for HH:MM:SS:FF display.
    pub fn nominal_fps(self) -> u32 {
        match self {
            Self::Fps23976 => 24,
            Self::Fps24 => 24,
            Self::Fps25 => 25,
            Self::Fps2997Df => 30,
            Self::Fps30 => 30,
        }
    }

    /// True wall-clock frames per second.
    pub fn exact_fps(self) -> f64 {
        match self {
            Self::Fps23976 => 24000.0 / 1001.0,
            Self::Fps24 => 24.0,
            Self::Fps25 => 25.0,
            Self::Fps2997Df => 30000.0 / 1001.0,
            Self::Fps30 => 30.0,
        }
    }

    pub fn is_drop_frame(self) -> bool {
        matches!(self, Self::Fps2997Df)
    }

    pub fn seconds_to_frames(self, seconds: f64) -> i64 {
        (seconds * self.exact_fps()).round() as i64
    }

    pub fn frames_to_seconds(self, frames: i64) -> f64 {
        frames as f64 / self.exact_fps()
    }
}

/// Signed SMPTE address. Negative values are valid offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timecode {
    /// Total frames from 00:00:00:00, may be negative.
    pub total_frames: i64,
    pub rate: FrameRate,
}

impl Timecode {
    pub fn zero(rate: FrameRate) -> Self {
        Self {
            total_frames: 0,
            rate,
        }
    }

    pub fn from_hmsf(
        hours: i64,
        minutes: i64,
        seconds: i64,
        frames: i64,
        rate: FrameRate,
    ) -> Result<Self, TimecodeError> {
        let fps = rate.nominal_fps() as i64;
        if frames.abs() >= fps {
            return Err(TimecodeError::FrameOutOfRange);
        }
        let sign = if hours < 0 || minutes < 0 || seconds < 0 || frames < 0 {
            -1
        } else {
            1
        };
        let h = hours.abs();
        let m = minutes.abs();
        let s = seconds.abs();
        let f = frames.abs();

        let nondrop = ((h * 3600) + (m * 60) + s) * fps + f;
        let total = if rate.is_drop_frame() {
            // Convert DF display address → absolute frame count.
            // Drop 2 frames every minute except every 10th minute.
            let total_minutes = h * 60 + m;
            let drop_frames = 2 * (total_minutes - total_minutes / 10);
            nondrop - drop_frames
        } else {
            nondrop
        };

        Ok(Self {
            total_frames: sign * total,
            rate,
        })
    }

    pub fn parse(s: &str, rate: FrameRate) -> Result<Self, TimecodeError> {
        let trimmed = s.trim();
        let negative = trimmed.starts_with('-');
        let body = trimmed.trim_start_matches(['-', '+']);
        let sep = if body.contains(';') { ';' } else { ':' };
        let parts: Vec<&str> = body.split(|c| c == ':' || c == ';').collect();
        if parts.len() != 4 {
            return Err(TimecodeError::InvalidString(s.to_string()));
        }
        let h: i64 = parts[0]
            .parse()
            .map_err(|_| TimecodeError::InvalidString(s.to_string()))?;
        let m: i64 = parts[1]
            .parse()
            .map_err(|_| TimecodeError::InvalidString(s.to_string()))?;
        let sec: i64 = parts[2]
            .parse()
            .map_err(|_| TimecodeError::InvalidString(s.to_string()))?;
        let f: i64 = parts[3]
            .parse()
            .map_err(|_| TimecodeError::InvalidString(s.to_string()))?;
        let mut tc = Self::from_hmsf(h, m, sec, f, rate)?;
        if negative {
            tc.total_frames = -tc.total_frames;
        }
        // Drop-frame notation with ';' is only valid for DF rates; accept either.
        let _ = sep;
        Ok(tc)
    }

    pub fn components(self) -> (i64, i64, i64, i64) {
        let fps = self.rate.nominal_fps() as i64;
        let negative = self.total_frames < 0;
        let mut frames = self.total_frames.abs();

        if self.rate.is_drop_frame() {
            // Absolute frames → DF display components.
            let drop_frames = 2i64;
            let frames_per_10min = 17982; // 10*60*29.97 rounded DF count
            let frames_per_min = 1798; // 60*29.97 - 2

            let d = frames / frames_per_10min;
            let m = frames % frames_per_10min;

            frames = if m < drop_frames {
                frames + drop_frames * 9 * d
            } else {
                frames + drop_frames * 9 * d + drop_frames * ((m - drop_frames) / frames_per_min)
            };
        }

        let ff = frames % fps;
        let total_secs = frames / fps;
        let ss = total_secs % 60;
        let total_mins = total_secs / 60;
        let mm = total_mins % 60;
        let hh = total_mins / 60;
        if negative {
            (-hh, -mm, -ss, -ff)
        } else {
            (hh, mm, ss, ff)
        }
    }

    pub fn display(self) -> String {
        let (h, m, s, f) = self.components();
        let sep = if self.rate.is_drop_frame() { ';' } else { ':' };
        let sign = if self.total_frames < 0 { "-" } else { "" };
        format!(
            "{sign}{:02}:{:02}:{:02}{sep}{:02}",
            h.abs(),
            m.abs(),
            s.abs(),
            f.abs()
        )
    }

    pub fn offset(self, delta_frames: i64) -> Self {
        Self {
            total_frames: self.total_frames + delta_frames,
            rate: self.rate,
        }
    }

    pub fn add_signed_offset(self, offset: Timecode) -> Self {
        // Convert offset into self's rate by wall-clock seconds.
        let secs = offset.rate.frames_to_seconds(offset.total_frames);
        let frames = self.rate.seconds_to_frames(secs);
        self.offset(frames)
    }

    pub fn to_seconds(self) -> f64 {
        self.rate.frames_to_seconds(self.total_frames)
    }

    pub fn from_seconds(seconds: f64, rate: FrameRate) -> Self {
        Self {
            total_frames: rate.seconds_to_frames(seconds),
            rate,
        }
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_display_25() {
        let tc = Timecode::parse("01:00:00:00", FrameRate::Fps25).unwrap();
        assert_eq!(tc.display(), "01:00:00:00");
        assert_eq!(tc.total_frames, 25 * 3600);
    }

    #[test]
    fn signed_offset() {
        let base = Timecode::parse("01:00:00:00", FrameRate::Fps25).unwrap();
        let off = Timecode::parse("-00:00:01:00", FrameRate::Fps25).unwrap();
        let out = base.add_signed_offset(off);
        assert_eq!(out.display(), "00:59:59:00");
    }

    #[test]
    fn df_roundtrip_common_addresses() {
        let rate = FrameRate::Fps2997Df;
        for s in ["00:00:00;00", "00:01:00;02", "00:10:00;00", "01:00:00;00"] {
            let tc = Timecode::parse(s, rate).unwrap();
            assert_eq!(tc.display(), s, "roundtrip {s}");
        }
    }

    #[test]
    fn df_minute_skips_two_frames() {
        let rate = FrameRate::Fps2997Df;
        let a = Timecode::parse("00:00:59;29", rate).unwrap();
        let b = a.offset(1);
        assert_eq!(b.display(), "00:01:00;02");
    }

    #[test]
    fn seconds_conversion_25() {
        let tc = Timecode::from_seconds(12.4, FrameRate::Fps25);
        assert_eq!(tc.total_frames, 310);
        assert!((tc.to_seconds() - 12.4).abs() < 1e-9);
    }

    #[test]
    fn exact_fps_values() {
        assert!((FrameRate::Fps2997Df.exact_fps() - 30000.0 / 1001.0).abs() < 1e-12);
        assert!((FrameRate::Fps23976.exact_fps() - 24000.0 / 1001.0).abs() < 1e-12);
    }
}
