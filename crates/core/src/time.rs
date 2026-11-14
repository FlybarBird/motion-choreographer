//! Frame rates, SMPTE timecode and frame/sample conversion.
//!
//! A timeline position is a whole frame counted from the song start. Timecode is
//! only a label for a frame count at a given rate.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum FrameRate {
    F23976,
    F24,
    F25,
    F2997Df,
    F30,
}

impl FrameRate {
    pub const ALL: [FrameRate; 5] = [FrameRate::F23976, FrameRate::F24, FrameRate::F25, FrameRate::F2997Df, FrameRate::F30];

    /// Real frames per second as a fraction `num / den`.
    pub fn num(self) -> i64 {
        match self {
            FrameRate::F23976 => 24000,
            FrameRate::F24 => 24,
            FrameRate::F25 => 25,
            FrameRate::F2997Df => 30000,
            FrameRate::F30 => 30,
        }
    }

    pub fn den(self) -> i64 {
        match self {
            FrameRate::F23976 | FrameRate::F2997Df => 1001,
            _ => 1,
        }
    }

    /// Frame numbers counted per timecode second.
    pub fn nominal(self) -> i64 {
        match self {
            FrameRate::F23976 | FrameRate::F24 => 24,
            FrameRate::F25 => 25,
            FrameRate::F2997Df | FrameRate::F30 => 30,
        }
    }

    pub fn drop_frame(self) -> bool {
        self == FrameRate::F2997Df
    }

    pub fn fps(self) -> f64 {
        self.num() as f64 / self.den() as f64
    }

    pub fn label(self) -> &'static str {
        match self {
            FrameRate::F23976 => "23.976",
            FrameRate::F24 => "24",
            FrameRate::F25 => "25",
            FrameRate::F2997Df => "29.97df",
            FrameRate::F30 => "30",
        }
    }

    pub fn parse(s: &str) -> Option<FrameRate> {
        FrameRate::ALL.into_iter().find(|r| r.label() == s)
    }

    /// Number of distinct frame labels in 24 hours.
    pub fn frames_per_day(self) -> i64 {
        if self.drop_frame() {
            // 17982 frames per ten minutes.
            17982 * 6 * 24
        } else {
            self.nominal() * 86400
        }
    }

    /// Seconds for a frame count, for display and speed math.
    pub fn frames_to_seconds(self, frames: f64) -> f64 {
        frames * self.den() as f64 / self.num() as f64
    }

    pub fn seconds_to_frames(self, seconds: f64) -> f64 {
        seconds * self.num() as f64 / self.den() as f64
    }
}

impl Serialize for FrameRate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.label())
    }
}

impl<'de> Deserialize<'de> for FrameRate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        FrameRate::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown frame rate '{s}'")))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Timecode {
    pub h: u8,
    pub m: u8,
    pub s: u8,
    pub f: u8,
    pub drop_frame: bool,
}

impl Timecode {
    pub fn new(h: u8, m: u8, s: u8, f: u8) -> Self {
        Timecode { h, m, s, f, drop_frame: false }
    }

    /// Label for a frame count since midnight. Wraps at 24 hours.
    pub fn from_frames(frames: i64, rate: FrameRate) -> Timecode {
        let day = rate.frames_per_day();
        let mut n = frames.rem_euclid(day);
        let nominal = rate.nominal();
        if rate.drop_frame() {
            let d = n / 17982;
            let m = n % 17982;
            n += 18 * d + if m >= 2 { 2 * ((m - 2) / 1798) } else { 0 };
        }
        Timecode {
            f: (n % nominal) as u8,
            s: ((n / nominal) % 60) as u8,
            m: ((n / (nominal * 60)) % 60) as u8,
            h: ((n / (nominal * 3600)) % 24) as u8,
            drop_frame: rate.drop_frame(),
        }
    }

    /// Frame count since midnight at `rate`.
    pub fn to_frames(self, rate: FrameRate) -> i64 {
        let nominal = rate.nominal();
        let minutes = self.h as i64 * 60 + self.m as i64;
        let mut n = (minutes * 60 + self.s as i64) * nominal + self.f as i64;
        if rate.drop_frame() {
            n -= 2 * (minutes - minutes / 10);
        }
        n
    }

    /// True when the label exists at `rate` (frame in range, not a dropped number).
    pub fn is_valid(self, rate: FrameRate) -> bool {
        if self.h > 23 || self.m > 59 || self.s > 59 || self.f as i64 >= rate.nominal() {
            return false;
        }
        if rate.drop_frame() && self.s == 0 && self.f < 2 && !self.m.is_multiple_of(10) {
            return false;
        }
        true
    }

    pub fn parse(s: &str) -> Option<Timecode> {
        let drop_frame = s.contains(';');
        let parts: Vec<&str> = s.split([':', ';']).collect();
        if parts.len() != 4 {
            return None;
        }
        let mut v = [0u8; 4];
        for (i, p) in parts.iter().enumerate() {
            if p.is_empty() || p.len() > 2 || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            v[i] = p.parse().ok()?;
        }
        Some(Timecode { h: v[0], m: v[1], s: v[2], f: v[3], drop_frame })
    }

    /// Parse and check against a rate. The separator is ignored; the rate decides
    /// whether numbering is drop-frame.
    pub fn parse_at(s: &str, rate: FrameRate) -> Option<Timecode> {
        let mut tc = Timecode::parse(s)?;
        tc.drop_frame = rate.drop_frame();
        tc.is_valid(rate).then_some(tc)
    }
}

impl fmt::Display for Timecode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sep = if self.drop_frame { ';' } else { ':' };
        write!(f, "{:02}:{:02}:{:02}{}{:02}", self.h, self.m, self.s, sep, self.f)
    }
}

/// Where a received code sits relative to a song.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SongPos {
    Before,
    In(i64),
    After,
}

/// Maps between song frames and timecode for one song.
#[derive(Clone, Copy, Debug)]
pub struct SongClock {
    pub rate: FrameRate,
    pub start_frames: i64,
    pub length_frames: i64,
}

impl SongClock {
    pub fn new(rate: FrameRate, start: Timecode, length_frames: i64) -> Self {
        SongClock { rate, start_frames: start.to_frames(rate), length_frames }
    }

    pub fn timecode_at(&self, frame: i64) -> Timecode {
        Timecode::from_frames(self.start_frames + frame, self.rate)
    }

    /// Song position for a code expressed as frames since midnight.
    /// Code earlier than the song start is `Before`; it never becomes a negative frame.
    /// Like `offset_of`, it takes the short way round midnight.
    pub fn locate(&self, code_frames: i64) -> SongPos {
        let day = self.rate.frames_per_day();
        let rel = (code_frames.rem_euclid(day) - self.start_frames).rem_euclid(day);
        let rel = if rel > day / 2 { rel - day } else { rel };
        if rel < 0 {
            SongPos::Before
        } else if rel > self.length_frames {
            SongPos::After
        } else {
            SongPos::In(rel)
        }
    }

    pub fn locate_tc(&self, tc: Timecode) -> SongPos {
        self.locate(tc.to_frames(self.rate))
    }

    /// Frames from the song start to a code, taking the short way round midnight.
    /// Negative when the code is before the song.
    pub fn offset_of(&self, code_frames: f64) -> f64 {
        let day = self.rate.frames_per_day() as f64;
        let rel = (code_frames - self.start_frames as f64).rem_euclid(day);
        if rel > day / 2.0 {
            rel - day
        } else {
            rel
        }
    }

    /// The song frame a file's first sample belongs on, given the file's start timecode.
    pub fn frame_of_stamp(&self, start_tc: &str) -> Option<i64> {
        let tc = Timecode::parse_at(start_tc, self.rate)?;
        Some(self.offset_of(tc.to_frames(self.rate) as f64).round() as i64)
    }
}

/// Start timecode for a time of day counted in audio samples since midnight,
/// which is how a broadcast WAV carries it. The nearest frame is taken.
pub fn timecode_of_samples(samples: u64, sample_rate: u32, rate: FrameRate) -> Timecode {
    let n = samples as i128 * rate.num() as i128 * 2;
    let d = sample_rate.max(1) as i128 * rate.den() as i128;
    Timecode::from_frames(((n + d) / (2 * d)) as i64, rate)
}

/// First audio sample at or after the start of `frame`. Integer math only.
pub fn frame_to_sample(frame: i64, sample_rate: u32, rate: FrameRate) -> i64 {
    let n = frame as i128 * sample_rate as i128 * rate.den() as i128;
    let d = rate.num() as i128;
    (n.div_euclid(d) + if n.rem_euclid(d) != 0 { 1 } else { 0 }) as i64
}

/// Frame that contains `sample`.
pub fn sample_to_frame(sample: i64, sample_rate: u32, rate: FrameRate) -> i64 {
    let n = sample as i128 * rate.num() as i128;
    let d = sample_rate as i128 * rate.den() as i128;
    n.div_euclid(d) as i64
}

/// Fractional frame position for a sample count.
pub fn sample_to_frame_f(sample: i64, sample_rate: u32, rate: FrameRate) -> f64 {
    sample as f64 * rate.num() as f64 / (sample_rate as f64 * rate.den() as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rate_round_trips_across_a_day() {
        for rate in FrameRate::ALL {
            let day = rate.frames_per_day();
            let mut prev: Option<Timecode> = None;
            for n in 0..day {
                let tc = Timecode::from_frames(n, rate);
                assert!(tc.is_valid(rate), "{tc} invalid at {}", rate.label());
                assert_eq!(tc.to_frames(rate), n, "{tc} at {}", rate.label());
                if let Some(p) = prev {
                    assert_ne!(p, tc);
                }
                prev = Some(tc);
            }
            assert_eq!(Timecode::from_frames(day, rate), Timecode::from_frames(0, rate));
        }
    }

    #[test]
    fn drop_frame_known_values() {
        let r = FrameRate::F2997Df;
        let last = Timecode::parse_at("00:00:59;29", r).unwrap();
        let next = Timecode::from_frames(last.to_frames(r) + 1, r);
        assert_eq!(next.to_string(), "00:01:00;02");

        let last = Timecode::parse_at("00:09:59;29", r).unwrap();
        let next = Timecode::from_frames(last.to_frames(r) + 1, r);
        assert_eq!(next.to_string(), "00:10:00;00");

        assert!(Timecode::parse_at("00:01:00;00", r).is_none());
        assert!(Timecode::parse_at("00:10:00;00", r).is_some());
        // One hour of drop-frame code is 107892 frames.
        assert_eq!(Timecode::parse_at("01:00:00;00", r).unwrap().to_frames(r), 107892);
    }

    #[test]
    fn song_start_maps_frames_to_code() {
        let rate = FrameRate::F25;
        let clock = SongClock::new(rate, Timecode::parse_at("01:00:00:00", rate).unwrap(), 4500);
        assert_eq!(clock.timecode_at(250).to_string(), "01:00:10:00");
        let tc = Timecode::parse_at("01:00:10:00", rate).unwrap();
        assert_eq!(clock.locate_tc(tc), SongPos::In(250));
        let early = Timecode::parse_at("00:59:59:24", rate).unwrap();
        assert_eq!(clock.locate_tc(early), SongPos::Before);
        let late = Timecode::parse_at("01:10:00:00", rate).unwrap();
        assert_eq!(clock.locate_tc(late), SongPos::After);
    }

    #[test]
    fn a_file_stamp_lands_on_its_code_either_side_of_the_song_start() {
        let rate = FrameRate::F25;
        let clock = SongClock::new(rate, Timecode::parse_at("01:00:00:00", rate).unwrap(), 4500);
        assert_eq!(clock.frame_of_stamp("01:00:10:00"), Some(250));
        assert_eq!(clock.frame_of_stamp("00:59:58:00"), Some(-50));
        assert_eq!(clock.frame_of_stamp("01:00:10:25"), None);
        // A song that starts just before midnight and a file stamped just after it.
        let late = SongClock::new(rate, Timecode::parse_at("23:59:50:00", rate).unwrap(), 4500);
        assert_eq!(late.frame_of_stamp("00:00:10:00"), Some(500));
        assert_eq!(late.offset_of(Timecode::parse_at("23:59:40:00", rate).unwrap().to_frames(rate) as f64), -250.0);
    }

    #[test]
    fn a_broadcast_wav_time_reference_reads_as_the_code_it_was_stamped_with() {
        for rate in FrameRate::ALL {
            let tc = Timecode::parse_at("01:00:10:00", rate).unwrap();
            for sr in [44_100u32, 48_000, 96_000] {
                // Writers round the frame boundary up or down; both read back as the frame.
                let exact = frame_to_sample(tc.to_frames(rate), sr, rate) as u64;
                assert_eq!(timecode_of_samples(exact, sr, rate), tc, "{} at {sr}", rate.label());
                assert_eq!(timecode_of_samples(exact - 1, sr, rate), tc, "{} at {sr}", rate.label());
            }
        }
        assert_eq!(timecode_of_samples(173_280_000, 48_000, FrameRate::F25).to_string(), "01:00:10:00");
        assert_eq!(timecode_of_samples(173_280_308, 48_000, FrameRate::F2997Df).to_string(), "01:00:10;00");
    }

    #[test]
    fn frame_sample_conversion_is_exact() {
        for rate in FrameRate::ALL {
            for sr in [44100u32, 48000] {
                // Ten minutes and a full day boundary region.
                let frames = (0..20000).chain(rate.frames_per_day() - 50..rate.frames_per_day());
                for f in frames {
                    let s = frame_to_sample(f, sr, rate);
                    assert_eq!(sample_to_frame(s, sr, rate), f);
                    if s > 0 {
                        assert_eq!(sample_to_frame(s - 1, sr, rate), f - 1);
                    }
                }
            }
        }
        assert_eq!(frame_to_sample(250, 48000, FrameRate::F25), 480000);
    }

    #[test]
    fn parse_rejects_bad_input() {
        assert!(Timecode::parse("01:00:00").is_none());
        assert!(Timecode::parse("aa:00:00:00").is_none());
        assert!(Timecode::parse_at("01:00:00:25", FrameRate::F25).is_none());
        assert!(Timecode::parse_at("24:00:00:00", FrameRate::F25).is_none());
    }

    #[test]
    fn a_code_just_after_midnight_is_found_inside_a_song_that_started_before_it() {
        let rate = FrameRate::F25;
        let clock = SongClock::new(rate, Timecode::parse_at("23:59:50:00", rate).unwrap(), 4500);
        let at = |s: &str| clock.locate_tc(Timecode::parse_at(s, rate).unwrap());
        assert_eq!(at("23:59:50:00"), SongPos::In(0));
        assert_eq!(at("23:59:59:24"), SongPos::In(249));
        assert_eq!(at("00:00:00:00"), SongPos::In(250));
        assert_eq!(at("00:00:10:00"), SongPos::In(500));
        assert_eq!(at("23:59:49:24"), SongPos::Before);
        assert_eq!(at("00:10:00:00"), SongPos::After);
        // It agrees with the offset the server plays from.
        let code = Timecode::parse_at("00:00:10:00", rate).unwrap().to_frames(rate);
        assert_eq!(clock.offset_of(code as f64), 500.0);
        // A song that starts just after midnight: code from just before it is before the song.
        let early = SongClock::new(rate, Timecode::parse_at("00:00:05:00", rate).unwrap(), 4500);
        assert_eq!(early.locate_tc(Timecode::parse_at("23:59:59:00", rate).unwrap()), SongPos::Before);
    }
}
