//! Chase engine: decides when an external code source can be trusted and where
//! the code is between readings. Pure; the caller supplies time in seconds.

use crate::codes::{CodeRate, CodeReading};
use crate::time::FrameRate;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum ChaseState {
    #[default]
    NoCode,
    Syncing,
    Locked,
    Freewheel,
    /// The source announces a rate family that does not match the project.
    RateMismatch,
}

impl ChaseState {
    pub fn label(self) -> &'static str {
        match self {
            ChaseState::NoCode => "No code",
            ChaseState::Syncing => "Syncing",
            ChaseState::Locked => "Locked",
            ChaseState::Freewheel => "Freewheel",
            ChaseState::RateMismatch => "Rate mismatch",
        }
    }
}

/// Frames in sequence needed before a source is trusted.
const SYNC_FRAMES: u32 = 4;
/// Code further than this from where it should be is a relocation.
const JUMP_FRAMES: f64 = 2.0;

pub struct Chase {
    rate: FrameRate,
    freewheel_frames: f64,
    /// Expected frames between readings: 1 for LTC and Art-Net, 2 for MTC.
    interval_frames: f64,
    state: ChaseState,
    last_frames: f64,
    last_time: f64,
    in_sequence: u32,
    announced: Option<CodeRate>,
    have_reading: bool,
    jumped: bool,
}

impl Chase {
    pub fn new(rate: FrameRate, freewheel_frames: u32, interval_frames: f64) -> Chase {
        Chase {
            rate,
            freewheel_frames: freewheel_frames as f64,
            interval_frames,
            state: ChaseState::NoCode,
            last_frames: 0.0,
            last_time: 0.0,
            in_sequence: 0,
            announced: None,
            have_reading: false,
            jumped: false,
        }
    }

    pub fn state(&self) -> ChaseState {
        self.state
    }

    pub fn set_freewheel(&mut self, frames: u32) {
        self.freewheel_frames = frames as f64;
    }

    pub fn announced(&self) -> Option<CodeRate> {
        self.announced
    }

    /// Last code received, in frames since midnight, with the time it arrived.
    pub fn last_reading(&self) -> Option<(f64, f64)> {
        self.have_reading.then_some((self.last_frames, self.last_time))
    }

    /// True once after the code relocated while locked.
    pub fn take_jump(&mut self) -> bool {
        std::mem::take(&mut self.jumped)
    }

    /// Where the code should be at `t`, in frames since midnight.
    fn expected(&self, t: f64) -> f64 {
        let day = self.rate.frames_per_day() as f64;
        (self.last_frames + (t - self.last_time) * self.rate.fps()).rem_euclid(day)
    }

    /// How far `a` is ahead of `b`, the short way round midnight.
    fn ahead(&self, a: f64, b: f64) -> f64 {
        let day = self.rate.frames_per_day() as f64;
        (a - b + day / 2.0).rem_euclid(day) - day / 2.0
    }

    pub fn on_reading(&mut self, r: CodeReading, t: f64) {
        self.announced = Some(r.rate);
        if !r.rate.matches(self.rate) {
            self.state = ChaseState::RateMismatch;
            self.have_reading = true;
            self.last_frames = r.frames;
            self.last_time = t;
            self.in_sequence = 0;
            return;
        }
        // Rolling code advances; a source that repeats one frame is parked.
        let in_sequence = self.have_reading
            && !r.locate
            && self.ahead(r.frames, self.last_frames) > 0.25
            && self.ahead(r.frames, self.expected(t)).abs() <= JUMP_FRAMES;
        match self.state {
            ChaseState::NoCode | ChaseState::RateMismatch => {
                self.state = ChaseState::Syncing;
                self.in_sequence = 1;
            }
            ChaseState::Syncing => {
                if in_sequence {
                    self.in_sequence += 1;
                    if self.in_sequence >= SYNC_FRAMES {
                        self.state = ChaseState::Locked;
                    }
                } else {
                    self.in_sequence = 1;
                }
            }
            ChaseState::Locked | ChaseState::Freewheel => {
                if in_sequence {
                    self.state = ChaseState::Locked;
                } else {
                    self.state = ChaseState::Syncing;
                    self.in_sequence = 1;
                    self.jumped = true;
                }
            }
        }
        self.have_reading = true;
        self.last_frames = r.frames;
        self.last_time = t;
    }

    /// Advance time without a reading: detects dropouts.
    pub fn tick(&mut self, t: f64) {
        let silent_frames = (t - self.last_time) * self.rate.fps();
        match self.state {
            ChaseState::Locked => {
                if silent_frames > self.interval_frames + 1.0 {
                    self.state = ChaseState::Freewheel;
                }
            }
            ChaseState::Freewheel => {
                if silent_frames > self.interval_frames + self.freewheel_frames {
                    self.state = ChaseState::NoCode;
                    self.in_sequence = 0;
                }
            }
            ChaseState::Syncing | ChaseState::RateMismatch => {
                if silent_frames > self.interval_frames + self.freewheel_frames.max(4.0) {
                    self.state = ChaseState::NoCode;
                    self.in_sequence = 0;
                }
            }
            ChaseState::NoCode => {}
        }
    }

    /// Where the code is now, when the source is trusted. Advances at the
    /// nominal rate between readings.
    pub fn code_at(&self, t: f64) -> Option<f64> {
        match self.state {
            ChaseState::Locked | ChaseState::Freewheel => Some(self.expected(t)),
            _ => None,
        }
    }

    /// Frames since the last reading.
    pub fn silent_frames(&self, t: f64) -> f64 {
        if self.have_reading {
            (t - self.last_time) * self.rate.fps()
        } else {
            f64::INFINITY
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: FrameRate = FrameRate::F25;

    fn reading(frames: f64) -> CodeReading {
        CodeReading { frames, rate: CodeRate::F25, locate: false }
    }

    /// Feed `count` frames in sequence starting at `first`, one per frame time.
    fn roll(c: &mut Chase, first: f64, count: usize, t0: f64) -> f64 {
        let mut t = t0;
        for i in 0..count {
            c.tick(t);
            c.on_reading(reading(first + i as f64), t);
            t += 1.0 / 25.0;
        }
        t
    }

    #[test]
    fn four_frames_in_sequence_lock() {
        let mut c = Chase::new(R, 8, 1.0);
        assert_eq!(c.state(), ChaseState::NoCode);
        c.on_reading(reading(1000.0), 0.0);
        assert_eq!(c.state(), ChaseState::Syncing);
        assert_eq!(c.code_at(0.0), None);
        c.on_reading(reading(1001.0), 0.04);
        c.on_reading(reading(1002.0), 0.08);
        assert_eq!(c.state(), ChaseState::Syncing);
        c.on_reading(reading(1003.0), 0.12);
        assert_eq!(c.state(), ChaseState::Locked);
        // Between readings the code advances at the nominal rate.
        assert!((c.code_at(0.14).unwrap() - 1003.5).abs() < 1e-9);
    }

    #[test]
    fn short_dropout_freewheels_and_relocks() {
        let mut c = Chase::new(R, 8, 1.0);
        let t = roll(&mut c, 1000.0, 10, 0.0);
        assert_eq!(c.state(), ChaseState::Locked);
        // Five frames with no code.
        let gap = t + 5.0 / 25.0;
        c.tick(gap);
        assert_eq!(c.state(), ChaseState::Freewheel);
        assert!((c.code_at(gap).unwrap() - 1015.0).abs() < 0.01);
        // Code returns in sequence.
        c.on_reading(reading(1015.0), gap);
        assert_eq!(c.state(), ChaseState::Locked);
        assert!(!c.take_jump());
    }

    #[test]
    fn long_dropout_loses_code() {
        let mut c = Chase::new(R, 8, 1.0);
        let t = roll(&mut c, 1000.0, 10, 0.0);
        c.tick(t + 3.0 / 25.0);
        assert_eq!(c.state(), ChaseState::Freewheel);
        c.tick(t + 12.0 / 25.0);
        assert_eq!(c.state(), ChaseState::NoCode);
        assert_eq!(c.code_at(t + 12.0 / 25.0), None);
        // It has to earn trust again.
        c.on_reading(reading(1030.0), t + 1.0);
        assert_eq!(c.state(), ChaseState::Syncing);
    }

    #[test]
    fn a_jump_resyncs_and_is_reported_once() {
        let mut c = Chase::new(R, 8, 1.0);
        let t = roll(&mut c, 1000.0, 10, 0.0);
        c.on_reading(reading(5000.0), t);
        assert_eq!(c.state(), ChaseState::Syncing);
        assert!(c.take_jump());
        assert!(!c.take_jump());
        let t = roll(&mut c, 5001.0, 3, t + 0.04);
        assert_eq!(c.state(), ChaseState::Locked);
        assert!((c.code_at(t - 0.04).unwrap() - 5003.0).abs() < 1e-6);
    }

    #[test]
    fn stopped_code_never_locks() {
        let mut c = Chase::new(R, 8, 1.0);
        let mut t = 0.0;
        for _ in 0..50 {
            c.on_reading(reading(1000.0), t);
            c.tick(t);
            t += 0.04;
        }
        assert_eq!(c.state(), ChaseState::Syncing);
        assert_eq!(c.last_reading().unwrap().0, 1000.0);
    }

    #[test]
    fn rate_mismatch_is_reported_and_never_locks() {
        let mut c = Chase::new(R, 8, 1.0);
        let mut t = 0.0;
        for i in 0..20 {
            c.on_reading(CodeReading { frames: 1000.0 + i as f64, rate: CodeRate::F30, locate: false }, t);
            t += 1.0 / 30.0;
        }
        assert_eq!(c.state(), ChaseState::RateMismatch);
        assert_eq!(c.announced(), Some(CodeRate::F30));
        assert_eq!(c.code_at(t), None);
    }

    #[test]
    fn mtc_interval_of_two_frames_stays_locked() {
        let mut c = Chase::new(R, 8, 2.0);
        let mut t = 0.0;
        for i in 0..10 {
            c.tick(t);
            c.on_reading(reading(1000.0 + 2.0 * i as f64 + 1.75), t);
            t += 2.0 / 25.0;
            c.tick(t - 0.001);
        }
        assert_eq!(c.state(), ChaseState::Locked);
        // A locate while rolling is a jump.
        c.on_reading(CodeReading { frames: 9000.0, rate: CodeRate::F25, locate: true }, t);
        assert_eq!(c.state(), ChaseState::Syncing);
    }

    #[test]
    fn code_that_runs_through_midnight_stays_locked() {
        // Time-of-day code: 23:59:58:20 rolls on to 00:00:00:09.
        let day = R.frames_per_day() as f64;
        let mut c = Chase::new(R, 8, 1.0);
        let mut t = roll(&mut c, day - 30.0, 30, 0.0);
        assert_eq!(c.state(), ChaseState::Locked);
        for i in 0..10 {
            c.tick(t);
            c.on_reading(reading(i as f64), t);
            assert_eq!(c.state(), ChaseState::Locked, "{i} frames after midnight");
            assert!(!c.take_jump(), "midnight is not a relocation");
            t += 1.0 / 25.0;
        }
        // Between readings the code is still frames since midnight.
        assert!((c.code_at(t - 0.02).unwrap() - 9.5).abs() < 1e-6);
        // Freewheeling over midnight carries on from zero as well.
        let mut c = Chase::new(R, 8, 1.0);
        let t = roll(&mut c, day - 10.0, 10, 0.0);
        c.tick(t + 3.0 / 25.0);
        assert_eq!(c.state(), ChaseState::Freewheel);
        assert!((c.code_at(t + 3.0 / 25.0).unwrap() - 3.0).abs() < 1e-6);
        c.on_reading(reading(3.0), t + 3.0 / 25.0);
        assert_eq!(c.state(), ChaseState::Locked);

        // Code that starts just after midnight having stopped just before it is still a jump.
        let mut c = Chase::new(R, 8, 1.0);
        let t = roll(&mut c, day - 500.0, 10, 0.0);
        c.on_reading(reading(20.0), t);
        assert_eq!(c.state(), ChaseState::Syncing);
        assert!(c.take_jump());
    }
}
