//! MIDI timecode and Art-Net timecode parsers and builders, and the rate family
//! every external source announces.

use crate::time::{FrameRate, Timecode};
use serde::{Deserialize, Serialize};

/// Rate family on the wire. 23.976 and 24 look the same, as do 29.97 and 30
/// non-drop, so the project rate decides between them.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum CodeRate {
    F24,
    F25,
    F30Df,
    F30,
}

impl CodeRate {
    pub fn of(rate: FrameRate) -> CodeRate {
        match rate {
            FrameRate::F23976 | FrameRate::F24 => CodeRate::F24,
            FrameRate::F25 => CodeRate::F25,
            FrameRate::F2997Df => CodeRate::F30Df,
            FrameRate::F30 => CodeRate::F30,
        }
    }

    pub fn matches(self, rate: FrameRate) -> bool {
        CodeRate::of(rate) == self
    }

    pub fn label(self) -> &'static str {
        match self {
            CodeRate::F24 => "24",
            CodeRate::F25 => "25",
            CodeRate::F30Df => "29.97 drop",
            CodeRate::F30 => "30",
        }
    }

    /// The two-bit rate code shared by MTC and Art-Net.
    pub fn wire(self) -> u8 {
        match self {
            CodeRate::F24 => 0,
            CodeRate::F25 => 1,
            CodeRate::F30Df => 2,
            CodeRate::F30 => 3,
        }
    }

    pub fn from_wire(v: u8) -> CodeRate {
        match v & 3 {
            0 => CodeRate::F24,
            1 => CodeRate::F25,
            2 => CodeRate::F30Df,
            _ => CodeRate::F30,
        }
    }

    /// A project rate that counts frames the way this family does.
    pub fn counting_rate(self) -> FrameRate {
        match self {
            CodeRate::F24 => FrameRate::F24,
            CodeRate::F25 => FrameRate::F25,
            CodeRate::F30Df => FrameRate::F2997Df,
            CodeRate::F30 => FrameRate::F30,
        }
    }
}

/// One timecode observation from any source.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CodeReading {
    /// Frames since midnight at the source's rate family. Fractional for MTC.
    pub frames: f64,
    pub rate: CodeRate,
    /// A locate (MTC full frame): the source repositioned and is not rolling.
    pub locate: bool,
}

// ---- MIDI timecode ----

/// Assembles quarter-frame messages. Eight pieces carry one timecode across
/// two frames.
#[derive(Default)]
pub struct MtcDecoder {
    pieces: [u8; 8],
    have: u8,
    last_piece: Option<u8>,
}

impl MtcDecoder {
    pub fn new() -> MtcDecoder {
        MtcDecoder::default()
    }

    /// Feed one MIDI message. Returns a reading when a timecode completes.
    pub fn feed(&mut self, msg: &[u8]) -> Option<CodeReading> {
        match msg {
            [0xF1, data, ..] => {
                let piece = (data >> 4) & 7;
                if piece == 0 {
                    self.have = 0;
                } else if self.have == 0 || self.last_piece != Some(piece - 1) {
                    // Joined mid-way or a piece was lost: wait for the next piece 0.
                    self.have = 0;
                    self.last_piece = Some(piece);
                    return None;
                }
                self.pieces[piece as usize] = data & 0x0F;
                self.have |= 1 << piece;
                self.last_piece = Some(piece);
                if piece == 7 && self.have == 0xFF {
                    self.have = 0;
                    let p = &self.pieces;
                    let rate = CodeRate::from_wire(p[7] >> 1);
                    let tc = Timecode {
                        f: p[0] | ((p[1] & 1) << 4),
                        s: p[2] | ((p[3] & 3) << 4),
                        m: p[4] | ((p[5] & 3) << 4),
                        h: p[6] | ((p[7] & 1) << 4),
                        drop_frame: rate == CodeRate::F30Df,
                    };
                    if !tc.is_valid(rate.counting_rate()) {
                        return None;
                    }
                    // The code describes the frame at piece 0. Piece 7 arrives
                    // seven quarter frames later.
                    let frames = tc.to_frames(rate.counting_rate()) as f64 + 1.75;
                    return Some(CodeReading { frames, rate, locate: false });
                }
                None
            }
            [0xF0, 0x7F, _, 0x01, 0x01, hr, mn, sc, fr, 0xF7] => {
                let rate = CodeRate::from_wire(hr >> 5);
                let tc = Timecode { h: hr & 0x1F, m: *mn, s: *sc, f: *fr, drop_frame: rate == CodeRate::F30Df };
                self.have = 0;
                self.last_piece = None;
                tc.is_valid(rate.counting_rate()).then(|| CodeReading {
                    frames: tc.to_frames(rate.counting_rate()) as f64,
                    rate,
                    locate: true,
                })
            }
            _ => None,
        }
    }
}

/// The eight quarter-frame messages for a timecode.
pub fn mtc_quarter_frames(tc: Timecode, rate: CodeRate) -> [[u8; 2]; 8] {
    let nib = [
        tc.f & 0x0F,
        (tc.f >> 4) & 1,
        tc.s & 0x0F,
        (tc.s >> 4) & 3,
        tc.m & 0x0F,
        (tc.m >> 4) & 3,
        tc.h & 0x0F,
        ((tc.h >> 4) & 1) | (rate.wire() << 1),
    ];
    let mut out = [[0u8; 2]; 8];
    for (i, n) in nib.iter().enumerate() {
        out[i] = [0xF1, ((i as u8) << 4) | n];
    }
    out
}

pub fn mtc_full_frame(tc: Timecode, rate: CodeRate) -> [u8; 10] {
    [0xF0, 0x7F, 0x7F, 0x01, 0x01, (rate.wire() << 5) | tc.h, tc.m, tc.s, tc.f, 0xF7]
}

// ---- Art-Net timecode ----

pub const ARTNET_PORT: u16 = 6454;
const ARTNET_ID: &[u8; 8] = b"Art-Net\0";
const OP_TIMECODE: u16 = 0x9700;

/// Parse an `ArtTimeCode` packet. Anything else is ignored.
pub fn artnet_parse(packet: &[u8]) -> Option<CodeReading> {
    if packet.len() < 19 || &packet[0..8] != ARTNET_ID {
        return None;
    }
    if u16::from_le_bytes([packet[8], packet[9]]) != OP_TIMECODE {
        return None;
    }
    let rate = match packet[18] {
        0..=3 => CodeRate::from_wire(packet[18]),
        _ => return None,
    };
    let tc = Timecode { f: packet[14], s: packet[15], m: packet[16], h: packet[17], drop_frame: rate == CodeRate::F30Df };
    tc.is_valid(rate.counting_rate()).then(|| CodeReading {
        frames: tc.to_frames(rate.counting_rate()) as f64,
        rate,
        locate: false,
    })
}

pub fn artnet_build(tc: Timecode, rate: CodeRate) -> [u8; 19] {
    let mut p = [0u8; 19];
    p[0..8].copy_from_slice(ARTNET_ID);
    p[8..10].copy_from_slice(&OP_TIMECODE.to_le_bytes());
    p[10] = 0;
    p[11] = 14;
    p[14] = tc.f;
    p[15] = tc.s;
    p[16] = tc.m;
    p[17] = tc.h;
    p[18] = rate.wire();
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mtc_quarter_frames_decode_with_the_two_frame_correction() {
        for (rate, wire) in [
            (FrameRate::F25, CodeRate::F25),
            (FrameRate::F2997Df, CodeRate::F30Df),
            (FrameRate::F30, CodeRate::F30),
            (FrameRate::F24, CodeRate::F24),
        ] {
            let mut d = MtcDecoder::new();
            let start = Timecode::parse_at("01:00:10:00", rate).unwrap().to_frames(rate);
            // Quarter frames advance the code by two frames per group of eight.
            for group in 0..20 {
                let frame = start + group * 2;
                let tc = Timecode::from_frames(frame, rate);
                let mut last = None;
                for msg in mtc_quarter_frames(tc, wire) {
                    last = d.feed(&msg);
                }
                let r = last.expect("a reading after piece 7");
                assert_eq!(r.rate, wire);
                assert!(!r.locate);
                assert_eq!(r.frames, frame as f64 + 1.75);
            }
        }
    }

    #[test]
    fn mtc_joining_mid_stream_waits_for_piece_zero() {
        let rate = FrameRate::F25;
        let tc = Timecode::parse_at("10:20:30:12", rate).unwrap();
        let msgs = mtc_quarter_frames(tc, CodeRate::F25);
        let mut d = MtcDecoder::new();
        for m in &msgs[3..] {
            assert!(d.feed(m).is_none());
        }
        let mut last = None;
        for m in &msgs {
            last = d.feed(m);
        }
        assert_eq!(last.unwrap().frames, tc.to_frames(rate) as f64 + 1.75);
        // A skipped piece discards the group.
        let mut d = MtcDecoder::new();
        for (i, m) in msgs.iter().enumerate() {
            if i != 4 {
                assert!(d.feed(m).is_none());
            }
        }
    }

    #[test]
    fn mtc_full_frame_is_a_locate() {
        let rate = FrameRate::F30;
        let tc = Timecode::parse_at("02:03:04:05", rate).unwrap();
        let mut d = MtcDecoder::new();
        let r = d.feed(&mtc_full_frame(tc, CodeRate::F30)).unwrap();
        assert!(r.locate);
        assert_eq!(r.frames, tc.to_frames(rate) as f64);
        assert_eq!(r.rate, CodeRate::F30);
    }

    #[test]
    fn artnet_round_trip_and_rejects() {
        for (rate, wire) in [
            (FrameRate::F24, CodeRate::F24),
            (FrameRate::F25, CodeRate::F25),
            (FrameRate::F2997Df, CodeRate::F30Df),
            (FrameRate::F30, CodeRate::F30),
        ] {
            let tc = Timecode::parse_at("01:00:10:07", rate).unwrap();
            let p = artnet_build(tc, wire);
            let r = artnet_parse(&p).unwrap();
            assert_eq!(r.frames, tc.to_frames(rate) as f64);
            assert_eq!(r.rate, wire);
        }
        let tc = Timecode::new(1, 0, 10, 7);
        let good = artnet_build(tc, CodeRate::F25);
        assert!(artnet_parse(&good[..18]).is_none(), "short packet");
        let mut wrong_op = good;
        wrong_op[9] = 0x50;
        assert!(artnet_parse(&wrong_op).is_none(), "wrong opcode");
        let mut wrong_id = good;
        wrong_id[0] = b'X';
        assert!(artnet_parse(&wrong_id).is_none());
        let mut bad_frame = good;
        bad_frame[14] = 29;
        assert!(artnet_parse(&bad_frame).is_none(), "frame 29 at 25 fps");
    }

    #[test]
    fn rate_families() {
        assert!(CodeRate::F24.matches(FrameRate::F23976));
        assert!(CodeRate::F24.matches(FrameRate::F24));
        assert!(!CodeRate::F30.matches(FrameRate::F2997Df));
        assert!(CodeRate::F30Df.matches(FrameRate::F2997Df));
        assert!(!CodeRate::F25.matches(FrameRate::F30));
    }
}
