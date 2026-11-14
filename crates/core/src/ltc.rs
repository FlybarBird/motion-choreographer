//! Linear timecode: 80 bits per frame, biphase-mark coded, ending in a fixed
//! 16-bit sync word. Encoder and decoder.

use crate::time::{FrameRate, Timecode};

/// Bits 64..79 in transmission order.
const SYNC: [u8; 16] = [0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1];

/// The 80 bits of one frame, with prefix sums so any instant inside the frame
/// can be turned into a signal level without state.
#[derive(Clone, Debug)]
pub struct LtcFrame {
    bits: [u8; 80],
    ones_before: [u8; 81],
}

impl LtcFrame {
    pub fn new(tc: Timecode, rate: FrameRate) -> LtcFrame {
        let mut bits = [0u8; 80];
        let mut put = |at: usize, len: usize, value: u8| {
            for i in 0..len {
                bits[at + i] = (value >> i) & 1;
            }
        };
        put(0, 4, tc.f % 10);
        put(8, 2, tc.f / 10);
        put(16, 4, tc.s % 10);
        put(24, 3, tc.s / 10);
        put(32, 4, tc.m % 10);
        put(40, 3, tc.m / 10);
        put(48, 4, tc.h % 10);
        put(56, 2, tc.h / 10);
        if rate.drop_frame() {
            bits[10] = 1;
        }
        bits[64..80].copy_from_slice(&SYNC);
        // Polarity correction keeps the number of ones even, so every frame
        // starts on the same signal polarity.
        let correction = if rate.nominal() == 25 { 59 } else { 27 };
        let ones: u32 = bits.iter().map(|b| *b as u32).sum();
        if ones % 2 == 1 {
            bits[correction] = 1;
        }
        let mut ones_before = [0u8; 81];
        for i in 0..80 {
            ones_before[i + 1] = ones_before[i] + bits[i];
        }
        LtcFrame { bits, ones_before }
    }

    pub fn bits(&self) -> &[u8; 80] {
        &self.bits
    }

    /// Signal level, +1 or -1, at `phase` in 0..1 through the frame.
    pub fn level_at(&self, phase: f64) -> f32 {
        let pos = (phase.clamp(0.0, 0.999_999_999) * 160.0) as usize;
        let bit = pos / 2;
        let second_half = pos % 2 == 1;
        // One transition at the start of every bit, one more mid-bit for a one.
        let mut transitions = bit as u32 + 1 + self.ones_before[bit] as u32;
        if second_half && self.bits[bit] == 1 {
            transitions += 1;
        }
        if transitions.is_multiple_of(2) {
            1.0
        } else {
            -1.0
        }
    }
}

/// Render consecutive frames starting at `first_frame` (frames since midnight).
/// `speed` scales the frame rate, for decoder tests.
pub fn encode(first_frame: i64, count: i64, rate: FrameRate, sample_rate: u32, level: f32, speed: f64) -> Vec<f32> {
    let samples_per_frame = sample_rate as f64 * rate.den() as f64 / (rate.num() as f64 * speed);
    let total = (count as f64 * samples_per_frame).floor() as usize;
    let mut out = Vec::with_capacity(total);
    let mut current = -1i64;
    let mut frame = LtcFrame::new(Timecode::from_frames(first_frame, rate), rate);
    for n in 0..total {
        let pos = n as f64 / samples_per_frame;
        let index = pos.floor() as i64;
        if index != current {
            current = index;
            frame = LtcFrame::new(Timecode::from_frames(first_frame + index, rate), rate);
        }
        out.push(frame.level_at(pos - index as f64) * level);
    }
    out
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LtcReading {
    pub tc: Timecode,
    /// Sample index just after the frame's last bit. At this sample the code is `tc` plus one frame.
    pub end_sample: u64,
    /// Measured frame length, from which the source rate follows.
    pub samples_per_frame: f64,
}

/// Streaming decoder. Measures the bit period from the signal, so it follows
/// level and speed changes.
pub struct LtcDecoder {
    sample_index: u64,
    envelope: f32,
    positive: bool,
    last_edge: u64,
    period: f64,
    prev_interval: f64,
    half_pending: bool,
    /// Length of the first half of a one bit, while its second half is awaited.
    first_half: f64,
    bits: u128,
    bit_count: u32,
    last_sync_end: Option<u64>,
}

impl Default for LtcDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl LtcDecoder {
    pub fn new() -> LtcDecoder {
        LtcDecoder {
            sample_index: 0,
            envelope: 0.0,
            positive: false,
            last_edge: 0,
            period: 0.0,
            prev_interval: 0.0,
            half_pending: false,
            first_half: 0.0,
            bits: 0,
            bit_count: 0,
            last_sync_end: None,
        }
    }

    pub fn samples_seen(&self) -> u64 {
        self.sample_index
    }

    fn push_bit(&mut self, bit: u8, out: &mut Vec<LtcReading>) {
        self.bits = (self.bits >> 1) | ((bit as u128) << 79);
        self.bit_count = (self.bit_count + 1).min(80);
        if self.bit_count < 80 {
            return;
        }
        let bit_at = |i: usize| ((self.bits >> i) & 1) as u8;
        if (0..16).any(|i| bit_at(64 + i) != SYNC[i]) {
            return;
        }
        let field = |at: usize, len: usize| (0..len).fold(0u8, |v, i| v | (bit_at(at + i) << i));
        let tc = Timecode {
            f: field(0, 4) + 10 * field(8, 2),
            s: field(16, 4) + 10 * field(24, 3),
            m: field(32, 4) + 10 * field(40, 3),
            h: field(48, 4) + 10 * field(56, 2),
            drop_frame: bit_at(10) == 1,
        };
        let end = self.sample_index;
        // The distance between sync words is the frame length, unless frames were missed in between.
        let by_bits = self.period * 80.0;
        let spf = match self.last_sync_end {
            Some(prev) if end > prev && ((end - prev) as f64 - by_bits).abs() < by_bits * 0.25 => (end - prev) as f64,
            _ => by_bits,
        };
        self.last_sync_end = Some(end);
        if tc.h < 24 && tc.m < 60 && tc.s < 60 && tc.f < 30 {
            out.push(LtcReading { tc, end_sample: end, samples_per_frame: spf });
        }
    }

    /// Feed samples; returns every frame completed inside them.
    pub fn feed(&mut self, samples: &[f32]) -> Vec<LtcReading> {
        let mut out = Vec::new();
        for &x in samples {
            // An infinite sample would pin the envelope at infinity and nothing after it would be heard.
            let x = if x.is_finite() { x } else { 0.0 };
            self.sample_index += 1;
            self.envelope = (self.envelope * 0.9995).max(x.abs());
            let threshold = self.envelope * 0.2;
            if self.envelope < 1e-4 {
                continue;
            }
            let flipped = if self.positive { x < -threshold } else { x > threshold };
            if !flipped {
                continue;
            }
            self.positive = !self.positive;
            let d = (self.sample_index - self.last_edge) as f64;
            self.last_edge = self.sample_index;
            if self.period == 0.0 {
                // Biphase intervals are one bit or half a bit long. The first
                // pair that differs by a factor of two gives the bit period.
                let ratio = if self.prev_interval > 0.0 { d / self.prev_interval } else { 0.0 };
                if (1.5..2.6).contains(&ratio) {
                    self.period = d;
                } else if (0.38..0.67).contains(&ratio) {
                    self.period = self.prev_interval;
                }
                self.prev_interval = d;
                self.half_pending = false;
                self.bit_count = 0;
                continue;
            }
            if d > self.period * 1.6 || d < self.period * 0.3 {
                // A gap or a glitch: measure again.
                self.period = 0.0;
                self.prev_interval = 0.0;
                self.half_pending = false;
                self.bit_count = 0;
                continue;
            }
            if d < self.period * 0.75 {
                if self.half_pending {
                    self.half_pending = false;
                    // Both halves together: doubling one of them would carry its rounding into the period.
                    self.period = self.period * 0.9 + (self.first_half + d) * 0.1;
                    self.push_bit(1, &mut out);
                } else {
                    self.half_pending = true;
                    self.first_half = d;
                }
            } else {
                if self.half_pending {
                    // A lone half bit: we were out of phase. Resynchronise.
                    self.half_pending = false;
                    self.bit_count = 0;
                }
                self.period = self.period * 0.9 + d * 0.1;
                self.push_bit(0, &mut out);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(samples: &[f32]) -> Vec<LtcReading> {
        let mut d = LtcDecoder::new();
        let mut out = Vec::new();
        for chunk in samples.chunks(480) {
            out.extend(d.feed(chunk));
        }
        out
    }

    #[test]
    fn every_frame_of_an_hour_round_trips_at_each_rate() {
        for rate in FrameRate::ALL {
            let hour = rate.nominal() * 3600;
            // Bit-level check of the whole hour, cheap and exhaustive.
            for n in 0..hour {
                let tc = Timecode::from_frames(n, rate);
                let frame = LtcFrame::new(tc, rate);
                let bits = frame.bits();
                assert_eq!(bits.iter().map(|b| *b as u32).sum::<u32>() % 2, 0);
                let field = |at: usize, len: usize| (0..len).fold(0u8, |v, i| v | (bits[at + i] << i));
                assert_eq!(field(0, 4) + 10 * field(8, 2), tc.f);
                assert_eq!(field(16, 4) + 10 * field(24, 3), tc.s);
                assert_eq!(field(32, 4) + 10 * field(40, 3), tc.m);
                assert_eq!(field(48, 4) + 10 * field(56, 2), tc.h);
                assert_eq!(frame.level_at(0.0), frame.level_at(0.0));
            }
            // Signal-level check across a minute boundary and an hour boundary.
            for first in [0, hour - 2 * rate.nominal() - 5] {
                let count = 3 * rate.nominal() + 10;
                let audio = encode(first, count, rate, 48000, 0.5, 1.0);
                let got = decode_all(&audio);
                assert!(got.len() as i64 >= count - 2, "{} decoded {} of {count}", rate.label(), got.len());
                for (i, r) in got.iter().enumerate() {
                    let n = r.tc.to_frames(rate);
                    assert_eq!(n, got[0].tc.to_frames(rate) + i as i64, "sequence broke at {}", r.tc);
                    assert_eq!(r.tc.drop_frame, rate.drop_frame());
                    // The frame ends where the next one starts.
                    let spf = 48000.0 * rate.den() as f64 / rate.num() as f64;
                    let expect_end = (n - first + 1) as f64 * spf;
                    assert!(
                        (r.end_sample as f64 - expect_end).abs() < 3.0,
                        "{} ended at {} not {expect_end}",
                        r.tc,
                        r.end_sample
                    );
                }
                assert!(got[0].tc.to_frames(rate) <= first + 1);
            }
        }
    }

    #[test]
    fn decodes_off_speed_and_low_level() {
        let rate = FrameRate::F25;
        for (speed, level) in [(0.9, 0.5), (1.1, 0.5), (1.0, 0.004), (1.0, 1.0)] {
            let audio = encode(90_000, 60, rate, 48000, level, speed);
            let got = decode_all(&audio);
            assert!(got.len() >= 57, "speed {speed} level {level}: {} frames", got.len());
            assert_eq!(got[10].tc.to_frames(rate), got[0].tc.to_frames(rate) + 10);
            let fps = 48000.0 / got[20].samples_per_frame;
            assert!((fps - 25.0 * speed).abs() < 0.2, "measured {fps} fps at speed {speed}");
        }
    }

    #[test]
    fn silence_and_noise_decode_nothing() {
        assert!(decode_all(&vec![0.0; 48000]).is_empty());
        let mut seed = 1u32;
        let noise: Vec<f32> = (0..48000)
            .map(|_| {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                (seed >> 8) as f32 / (1 << 24) as f32 - 0.5
            })
            .collect();
        assert!(decode_all(&noise).is_empty());
    }

    #[test]
    fn recovers_after_a_dropout() {
        let rate = FrameRate::F30;
        let mut audio = encode(1000, 30, rate, 48000, 0.5, 1.0);
        audio.extend(vec![0.0; 9000]);
        audio.extend(encode(5000, 30, rate, 48000, 0.5, 1.0));
        let got = decode_all(&audio);
        assert!(got.iter().any(|r| r.tc.to_frames(rate) == 1020));
        assert!(got.iter().any(|r| r.tc.to_frames(rate) == 5020));
    }

    #[test]
    fn a_sample_that_is_not_a_number_does_not_deafen_the_decoder() {
        // A float file or input device can carry an infinite or not-a-number sample.
        let rate = FrameRate::F30;
        for bad in [f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
            let mut audio = encode(1000, 30, rate, 48000, 0.5, 1.0);
            audio.push(bad);
            audio.extend(encode(5000, 30, rate, 48000, 0.5, 1.0));
            let got = decode_all(&audio);
            assert!(got.iter().any(|r| r.tc.to_frames(rate) == 1020), "{bad}");
            assert!(got.iter().any(|r| r.tc.to_frames(rate) == 5020), "code after a {bad} sample is still read");
        }
    }
}
