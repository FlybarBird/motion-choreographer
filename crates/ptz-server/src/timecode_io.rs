//! Timecode source adapters. Physical I/O is stubbed behind clean interfaces.

use async_trait::async_trait;
use ptz_core::{FrameRate, Timecode};
use parking_lot::Mutex;
use std::time::Instant;

#[async_trait]
pub trait TimecodeSource: Send + Sync {
    fn name(&self) -> &str;
    /// Current absolute timecode after applying signed offset.
    fn read(&self) -> Option<Timecode>;
    fn set_signed_offset(&self, offset: Timecode);
    fn is_physical(&self) -> bool;
}

/// Internal free-running clock driven by the show engine.
pub struct InternalClock {
    rate: Mutex<FrameRate>,
    started: Mutex<Option<Instant>>,
    base_frames: Mutex<i64>,
    offset: Mutex<Timecode>,
    running: Mutex<bool>,
}

impl InternalClock {
    pub fn new(rate: FrameRate) -> Self {
        Self {
            rate: Mutex::new(rate),
            started: Mutex::new(None),
            base_frames: Mutex::new(0),
            offset: Mutex::new(Timecode::zero(rate)),
            running: Mutex::new(false),
        }
    }

    pub fn set_rate(&self, rate: FrameRate) {
        *self.rate.lock() = rate;
        self.offset.lock().rate = rate;
    }

    pub fn seek_frames(&self, frames: i64) {
        *self.base_frames.lock() = frames;
        *self.started.lock() = if *self.running.lock() {
            Some(Instant::now())
        } else {
            None
        };
    }

    pub fn start(&self) {
        *self.running.lock() = true;
        *self.started.lock() = Some(Instant::now());
    }

    pub fn pause(&self) {
        if let Some(tc) = self.read_raw() {
            *self.base_frames.lock() = tc.total_frames;
        }
        *self.running.lock() = false;
        *self.started.lock() = None;
    }

    pub fn stop_and_cue(&self) {
        *self.running.lock() = false;
        *self.started.lock() = None;
        *self.base_frames.lock() = 0;
    }

    fn read_raw(&self) -> Option<Timecode> {
        let rate = *self.rate.lock();
        let frames = self.song_frames();
        Some(Timecode {
            total_frames: frames,
            rate,
        })
    }

    /// Song-relative frame position (ignores SMPTE signed offset).
    pub fn song_frames(&self) -> i64 {
        let rate = *self.rate.lock();
        let base = *self.base_frames.lock();
        if *self.running.lock() {
            let elapsed = self
                .started
                .lock()
                .map(|t| t.elapsed().as_secs_f64())
                .unwrap_or(0.0);
            base + rate.seconds_to_frames(elapsed)
        } else {
            base
        }
    }
}

#[async_trait]
impl TimecodeSource for InternalClock {
    fn name(&self) -> &str {
        "Internal"
    }

    fn read(&self) -> Option<Timecode> {
        let raw = self.read_raw()?;
        let offset = *self.offset.lock();
        Some(raw.add_signed_offset(offset))
    }

    fn set_signed_offset(&self, offset: Timecode) {
        *self.offset.lock() = offset;
    }

    fn is_physical(&self) -> bool {
        false
    }
}

/// LTC audio input parser stub.
///
/// TODO(hardware): decode bi-phase mark LTC from an audio input device via cpal,
/// apply drop-frame rules, and expose through this trait.
pub struct LtcInputStub {
    offset: Mutex<Timecode>,
    rate: FrameRate,
}

impl LtcInputStub {
    pub fn new(rate: FrameRate) -> Self {
        Self {
            offset: Mutex::new(Timecode::zero(rate)),
            rate,
        }
    }
}

#[async_trait]
impl TimecodeSource for LtcInputStub {
    fn name(&self) -> &str {
        "LTC (stub)"
    }

    fn read(&self) -> Option<Timecode> {
        // No physical decoder in VM — return None so show falls back/holds.
        let _ = self.rate;
        let _ = self.offset.lock();
        None
    }

    fn set_signed_offset(&self, offset: Timecode) {
        *self.offset.lock() = offset;
    }

    fn is_physical(&self) -> bool {
        true
    }
}

/// MIDI Timecode quarter-frame parser stub.
///
/// TODO(hardware): ingest MTC quarter-frame messages from a MIDI port and
/// assemble full-frame timecode with signed offset support.
pub struct MtcInputStub {
    offset: Mutex<Timecode>,
}

impl MtcInputStub {
    pub fn new(rate: FrameRate) -> Self {
        Self {
            offset: Mutex::new(Timecode::zero(rate)),
        }
    }
}

#[async_trait]
impl TimecodeSource for MtcInputStub {
    fn name(&self) -> &str {
        "MTC (stub)"
    }

    fn read(&self) -> Option<Timecode> {
        let _ = self.offset.lock();
        None
    }

    fn set_signed_offset(&self, offset: Timecode) {
        *self.offset.lock() = offset;
    }

    fn is_physical(&self) -> bool {
        true
    }
}

/// Art-Net timecode packet parser (data path implemented; UDP bind optional).
///
/// Parses Art-Net OpCode ArtTimecode (0x9700) payloads when provided.
pub struct ArtNetTimecode {
    offset: Mutex<Timecode>,
    last: Mutex<Option<Timecode>>,
    rate: Mutex<FrameRate>,
}

impl ArtNetTimecode {
    pub fn new(rate: FrameRate) -> Self {
        Self {
            offset: Mutex::new(Timecode::zero(rate)),
            last: Mutex::new(None),
            rate: Mutex::new(rate),
        }
    }

    /// Parse a raw Art-Net packet. Returns true if it was ArtTimecode.
    pub fn ingest_packet(&self, packet: &[u8]) -> bool {
        // Art-Net header "Art-Net\0", OpCode little-endian at bytes 8..10
        if packet.len() < 19 || &packet[0..8] != b"Art-Net\0" {
            return false;
        }
        let opcode = u16::from_le_bytes([packet[8], packet[9]]);
        if opcode != 0x9700 {
            return false;
        }
        // ArtTimecode: after Protocol version (10..12), filler + Frames,Seconds,Minutes,Hours,Type
        // Layout per Art-Net 4: bytes 14=Frames 15=Seconds 16=Minutes 17=Hours 18=Type
        if packet.len() < 19 {
            return false;
        }
        let frames = packet[14] as i64;
        let seconds = packet[15] as i64;
        let minutes = packet[16] as i64;
        let hours = packet[17] as i64;
        let ty = packet[18];
        let rate = match ty {
            0 => FrameRate::Fps24,
            1 => FrameRate::Fps25,
            2 => FrameRate::Fps2997Df,
            3 => FrameRate::Fps30,
            _ => *self.rate.lock(),
        };
        *self.rate.lock() = rate;
        if let Ok(tc) = Timecode::from_hmsf(hours, minutes, seconds, frames, rate) {
            *self.last.lock() = Some(tc);
            true
        } else {
            false
        }
    }
}

#[async_trait]
impl TimecodeSource for ArtNetTimecode {
    fn name(&self) -> &str {
        "Art-Net Timecode"
    }

    fn read(&self) -> Option<Timecode> {
        let raw = (*self.last.lock())?;
        Some(raw.add_signed_offset(*self.offset.lock()))
    }

    fn set_signed_offset(&self, offset: Timecode) {
        *self.offset.lock() = offset;
    }

    fn is_physical(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artnet_parse_25fps() {
        let src = ArtNetTimecode::new(FrameRate::Fps25);
        let mut pkt = vec![0u8; 19];
        pkt[0..8].copy_from_slice(b"Art-Net\0");
        pkt[8] = 0x00;
        pkt[9] = 0x97;
        // frames=0, secs=0, mins=0, hours=1, type=1 (25fps)
        pkt[14] = 0;
        pkt[15] = 0;
        pkt[16] = 0;
        pkt[17] = 1;
        pkt[18] = 1;
        assert!(src.ingest_packet(&pkt));
        let tc = src.read().unwrap();
        assert_eq!(tc.display(), "01:00:00:00");
    }

    #[test]
    fn signed_offset_on_artnet() {
        let src = ArtNetTimecode::new(FrameRate::Fps25);
        let mut pkt = vec![0u8; 19];
        pkt[0..8].copy_from_slice(b"Art-Net\0");
        pkt[8] = 0x00;
        pkt[9] = 0x97;
        pkt[17] = 1;
        pkt[18] = 1;
        src.ingest_packet(&pkt);
        src.set_signed_offset(Timecode::parse("-00:00:01:00", FrameRate::Fps25).unwrap());
        assert_eq!(src.read().unwrap().display(), "00:59:59:00");
    }
}
