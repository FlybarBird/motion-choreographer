//! The master clock for free-run playback. Until an audio device is open this
//! is the monotonic clock; the audio engine replaces it with the device's
//! sample counter.

use ptz_core::time::FrameRate;
use std::time::Instant;

pub struct Clock {
    playing: Option<(Instant, f64)>,
    parked: f64,
}

impl Default for Clock {
    fn default() -> Self {
        Clock::new()
    }
}

impl Clock {
    pub fn new() -> Clock {
        Clock { playing: None, parked: 0.0 }
    }

    pub fn play(&mut self, frame: f64, _rate: FrameRate) {
        self.playing = Some((Instant::now(), frame));
    }

    pub fn stop(&mut self) {
        self.playing = None;
    }

    pub fn seek(&mut self, frame: f64, _rate: FrameRate) {
        self.parked = frame;
        if self.playing.is_some() {
            self.playing = Some((Instant::now(), frame));
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// Position in song frames.
    pub fn position(&self, rate: FrameRate) -> f64 {
        match self.playing {
            Some((t0, f0)) => f0 + rate.seconds_to_frames(t0.elapsed().as_secs_f64()),
            None => self.parked,
        }
    }
}
