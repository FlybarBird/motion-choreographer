//! VISCA over IP as the FR7 speaks it: the 8-byte header, the commands the
//! adapter sends and the replies it reads. Bytes only; no sockets here.
//!
//! Source: Sony's "ILME-FR7 VISCA Command List". Each function names the line
//! of the list it encodes.

pub const PORT: u16 = 52381;

pub const TYPE_COMMAND: u16 = 0x0100;
pub const TYPE_INQUIRY: u16 = 0x0110;
pub const TYPE_REPLY: u16 = 0x0111;
pub const TYPE_CONTROL: u16 = 0x0200;
pub const TYPE_CONTROL_REPLY: u16 = 0x0201;

/// Control payload that sets the camera's expected sequence number back to zero.
pub const RESET: [u8; 1] = [0x01];
/// Control reply to a message whose sequence number was not the next one.
pub const BAD_SEQUENCE: [u8; 2] = [0x0F, 0x01];

/// Highest speed step in normal mode.
pub const TOP_STEP: u8 = 0x18;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Packet {
    pub kind: u16,
    pub seq: u32,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn new(kind: u16, seq: u32, payload: &[u8]) -> Packet {
        Packet { kind, seq, payload: payload.to_vec() }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.payload.len());
        out.extend_from_slice(&self.kind.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn parse(bytes: &[u8]) -> Option<Packet> {
        let kind = u16::from_be_bytes(bytes.get(0..2)?.try_into().ok()?);
        let len = u16::from_be_bytes(bytes.get(2..4)?.try_into().ok()?) as usize;
        let seq = u32::from_be_bytes(bytes.get(4..8)?.try_into().ok()?);
        let payload = bytes.get(8..8 + len)?.to_vec();
        (bytes.len() == 8 + len).then_some(Packet { kind, seq, payload })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Turn {
    /// Pan left, or tilt up.
    Plus,
    /// Pan right, or tilt down.
    Minus,
    Hold,
}

impl Turn {
    fn byte(self) -> u8 {
        match self {
            Turn::Plus => 0x01,
            Turn::Minus => 0x02,
            Turn::Hold => 0x03,
        }
    }

    pub fn from_byte(b: u8) -> Option<Turn> {
        match b {
            0x01 => Some(Turn::Plus),
            0x02 => Some(Turn::Minus),
            0x03 => Some(Turn::Hold),
            _ => None,
        }
    }
}

fn nibbles(value: u32, count: usize) -> impl Iterator<Item = u8> {
    (0..count).rev().map(move |i| ((value >> (4 * i)) & 0x0F) as u8)
}

fn from_nibbles(bytes: &[u8]) -> Option<u32> {
    bytes.iter().try_fold(0u32, |acc, b| (*b <= 0x0F).then_some((acc << 4) | *b as u32))
}

/// A signed position in five nibbles, two's complement.
fn signed20(counts: i32) -> u32 {
    (counts as u32) & 0xF_FFFF
}

fn from_signed20(raw: u32) -> i32 {
    if raw & 0x8_0000 != 0 {
        (raw | 0xFFF0_0000) as i32
    } else {
        raw as i32
    }
}

/// Pan and tilt at a speed: `81 01 06 01 vv ww dd dd FF`.
pub fn pan_tilt_drive(pan_step: u8, tilt_step: u8, pan: Turn, tilt: Turn) -> Vec<u8> {
    vec![0x81, 0x01, 0x06, 0x01, pan_step.clamp(1, TOP_STEP), tilt_step.clamp(1, TOP_STEP), pan.byte(), tilt.byte(), 0xFF]
}

/// Pan and tilt stop: the drive command with both directions held.
pub fn pan_tilt_stop() -> Vec<u8> {
    pan_tilt_drive(TOP_STEP, TOP_STEP, Turn::Hold, Turn::Hold)
}

/// Absolute pan and tilt: `81 01 06 02 vv ww 0p 0p 0p 0p 0p 0t 0t 0t 0t 0t FF`.
pub fn pan_tilt_absolute(pan_step: u8, tilt_step: u8, pan: i32, tilt: i32) -> Vec<u8> {
    let mut out = vec![0x81, 0x01, 0x06, 0x02, pan_step.clamp(1, TOP_STEP), tilt_step.clamp(1, TOP_STEP)];
    out.extend(nibbles(signed20(pan), 5));
    out.extend(nibbles(signed20(tilt), 5));
    out.push(0xFF);
    out
}

/// Position query: `81 09 06 12 FF`.
pub fn pan_tilt_inquiry() -> Vec<u8> {
    vec![0x81, 0x09, 0x06, 0x12, 0xFF]
}

/// Zoom position: `81 01 04 47 0z 0z 0z 0z FF`, 0000 wide to 4000 optical tele.
pub fn zoom_direct(counts: u16) -> Vec<u8> {
    let mut out = vec![0x81, 0x01, 0x04, 0x47];
    out.extend(nibbles(counts.min(ZOOM_TELE) as u32, 4));
    out.push(0xFF);
    out
}

pub fn zoom_inquiry() -> Vec<u8> {
    vec![0x81, 0x09, 0x04, 0x47, 0xFF]
}

pub fn zoom_stop() -> Vec<u8> {
    vec![0x81, 0x01, 0x04, 0x07, 0x00, 0xFF]
}

/// Zoom toward tele or wide at a speed step, 0 slow through 7 fast.
/// A power-zoom lens that refuses a direct position still takes this.
pub fn zoom_toward(tele: bool, step: u8) -> Vec<u8> {
    vec![0x81, 0x01, 0x04, 0x07, (if tele { 0x20 } else { 0x30 }) | step.clamp(0, 7), 0xFF]
}

/// Focus position: `81 01 04 48 0p 0p 0p 0p FF`, 0000 far to FFFF near.
pub fn focus_direct(counts: u16) -> Vec<u8> {
    let mut out = vec![0x81, 0x01, 0x04, 0x48];
    out.extend(nibbles(counts as u32, 4));
    out.push(0xFF);
    out
}

pub fn focus_inquiry() -> Vec<u8> {
    vec![0x81, 0x09, 0x04, 0x48, 0xFF]
}

pub fn focus_stop() -> Vec<u8> {
    vec![0x81, 0x01, 0x04, 0x08, 0x00, 0xFF]
}

/// Focus has to be in manual mode before a focus position is accepted.
pub fn focus_manual() -> Vec<u8> {
    vec![0x81, 0x01, 0x04, 0x38, 0x03, 0xFF]
}

pub fn focus_auto() -> Vec<u8> {
    vec![0x81, 0x01, 0x04, 0x38, 0x02, 0xFF]
}

/// Recall preset `number` (1 is the first): `81 01 04 3F 02 pp FF`.
pub fn preset_recall(number: u32) -> Vec<u8> {
    vec![0x81, 0x01, 0x04, 0x3F, 0x02, number.saturating_sub(1).min(0x7F) as u8, 0xFF]
}

pub const ZOOM_TELE: u16 = 0x4000;

pub fn ack(socket: u8) -> Vec<u8> {
    vec![0x90, 0x40 | (socket & 0x0F), 0xFF]
}

pub fn completion(socket: u8) -> Vec<u8> {
    vec![0x90, 0x50 | (socket & 0x0F), 0xFF]
}

pub fn error(socket: u8, code: u8) -> Vec<u8> {
    vec![0x90, 0x60 | (socket & 0x0F), code, 0xFF]
}

pub fn pan_tilt_reply(pan: i32, tilt: i32) -> Vec<u8> {
    let mut out = vec![0x90, 0x50];
    out.extend(nibbles(signed20(pan), 5));
    out.extend(nibbles(signed20(tilt), 5));
    out.push(0xFF);
    out
}

pub fn value_reply(counts: u16) -> Vec<u8> {
    let mut out = vec![0x90, 0x50];
    out.extend(nibbles(counts as u32, 4));
    out.push(0xFF);
    out
}

/// What a reply payload says.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Reply {
    Ack,
    /// Completion of a command, or the answer to an inquiry with its data bytes.
    Done(Vec<u8>),
    Error(u8),
}

pub fn parse_reply(payload: &[u8]) -> Option<Reply> {
    let (&last, body) = payload.split_last()?;
    if last != 0xFF || body.len() < 2 || body[0] != 0x90 {
        return None;
    }
    match body[1] & 0xF0 {
        0x40 if body.len() == 2 => Some(Reply::Ack),
        0x50 => Some(Reply::Done(body[2..].to_vec())),
        0x60 if body.len() == 3 => Some(Reply::Error(body[2])),
        _ => None,
    }
}

pub fn error_text(code: u8) -> &'static str {
    match code {
        0x01 => "the message was too long",
        0x02 => "the camera did not understand the command",
        0x03 => "the camera's command buffer is full",
        0x04 => "the command was cancelled",
        0x05 => "there is no command to cancel",
        0x41 => "the camera cannot run the command in its current mode",
        _ => "the camera reported an error",
    }
}

/// Pan and tilt counts from the answer to a position query.
pub fn parse_pan_tilt(data: &[u8]) -> Option<(i32, i32)> {
    if data.len() != 10 {
        return None;
    }
    Some((from_signed20(from_nibbles(&data[..5])?), from_signed20(from_nibbles(&data[5..])?)))
}

/// Zoom or focus counts from the answer to a position query.
pub fn parse_value(data: &[u8]) -> Option<u16> {
    (data.len() == 4).then(|| from_nibbles(data)).flatten().map(|v| v as u16)
}

/// What a command payload asks for. The protocol fake acts on these.
#[derive(Clone, PartialEq, Debug)]
pub enum Command {
    Drive { pan_step: u8, tilt_step: u8, pan: Turn, tilt: Turn },
    Absolute { pan_step: u8, tilt_step: u8, pan: i32, tilt: i32 },
    PanTiltInquiry,
    ZoomDirect(u16),
    ZoomInquiry,
    ZoomStop,
    FocusDirect(u16),
    FocusInquiry,
    FocusStop,
    FocusManual,
    FocusAuto,
    Recall(u32),
}

pub fn parse_command(p: &[u8]) -> Option<Command> {
    let (&last, body) = p.split_last()?;
    if last != 0xFF || body.first() != Some(&0x81) {
        return None;
    }
    match &body[1..] {
        [0x01, 0x06, 0x01, v, w, a, b] => {
            Some(Command::Drive { pan_step: *v, tilt_step: *w, pan: Turn::from_byte(*a)?, tilt: Turn::from_byte(*b)? })
        }
        [0x01, 0x06, 0x02, v, w, rest @ ..] if rest.len() == 10 => {
            let (pan, tilt) = parse_pan_tilt(rest)?;
            Some(Command::Absolute { pan_step: *v, tilt_step: *w, pan, tilt })
        }
        [0x09, 0x06, 0x12] => Some(Command::PanTiltInquiry),
        [0x01, 0x04, 0x47, rest @ ..] => Some(Command::ZoomDirect(parse_value(rest)?)),
        [0x09, 0x04, 0x47] => Some(Command::ZoomInquiry),
        [0x01, 0x04, 0x07, 0x00] => Some(Command::ZoomStop),
        [0x01, 0x04, 0x48, rest @ ..] => Some(Command::FocusDirect(parse_value(rest)?)),
        [0x09, 0x04, 0x48] => Some(Command::FocusInquiry),
        [0x01, 0x04, 0x08, 0x00] => Some(Command::FocusStop),
        [0x01, 0x04, 0x38, 0x02] => Some(Command::FocusAuto),
        [0x01, 0x04, 0x38, 0x03] => Some(Command::FocusManual),
        [0x01, 0x04, 0x3F, 0x02, n] => Some(Command::Recall(*n as u32 + 1)),
        _ => None,
    }
}

/// Degrees per second of each of the 24 speed steps, slowest first, as measured on an FR7
/// (firmware 4.00, 24-step mode, speed scale 100). Steps 1 to 19 are the measured speeds. Steps
/// 20 to 24 continue the measured curve by its own ratio, because the head needs most of a second
/// to reach them; they end near the 60 degrees per second Sony gives. Bring-up measures a head's own
/// table, which then takes this one's place.
pub fn speed_table() -> Vec<f64> {
    vec![
        0.050, 0.115, 0.221, 0.359, 0.546, 0.777, 1.09, 1.44, 1.91, 2.48, 3.136, 4.00, 5.08, 6.46, 7.97, 10.0, 12.5, 15.5, 19.4,
        24.3, 30.3, 37.9, 47.4, 59.2,
    ]
}

/// The step and direction for a wanted speed in degrees per second. Speeds
/// between two steps alternate between them: `carry` holds the share owed to
/// the faster one. Slower than half the lowest step is a hold.
pub fn step_for(table: &[f64], speed: f64, carry: &mut f64) -> (u8, Turn) {
    let mag = speed.abs();
    if table.is_empty() || mag < table[0] * 0.5 {
        *carry = 0.0;
        return (1, Turn::Hold);
    }
    let turn = if speed > 0.0 { Turn::Plus } else { Turn::Minus };
    let above = table.partition_point(|s| *s <= mag);
    if above == 0 {
        return (1, turn);
    }
    if above == table.len() {
        return (table.len() as u8, turn);
    }
    let (lo, hi) = (table[above - 1], table[above]);
    *carry += (mag - lo) / (hi - lo);
    if *carry >= 1.0 {
        *carry -= 1.0;
        (above as u8 + 1, turn)
    } else {
        (above as u8, turn)
    }
}

/// The slowest step at least as fast as `speed`, for an absolute move that has to arrive in time.
pub fn step_at_least(table: &[f64], speed: f64) -> u8 {
    (table.partition_point(|s| *s < speed.abs()) as u8 + 1).clamp(1, table.len().max(1) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_is_type_length_and_sequence() {
        let p = Packet::new(TYPE_INQUIRY, 0x0102_0304, &pan_tilt_inquiry());
        assert_eq!(p.to_bytes(), [0x01, 0x10, 0x00, 0x05, 0x01, 0x02, 0x03, 0x04, 0x81, 0x09, 0x06, 0x12, 0xFF]);
        assert_eq!(Packet::parse(&p.to_bytes()), Some(p));
        assert_eq!(Packet::new(TYPE_CONTROL, 0, &RESET).to_bytes(), [0x02, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0x01]);
        assert_eq!(Packet::parse(&[0x01, 0x00, 0x00, 0x09, 0, 0, 0, 1, 0x81]), None, "a short payload is not a packet");
    }

    #[test]
    fn every_command_matches_the_published_bytes() {
        assert_eq!(pan_tilt_drive(0x0C, 0x05, Turn::Plus, Turn::Minus), [0x81, 0x01, 0x06, 0x01, 0x0C, 0x05, 0x01, 0x02, 0xFF]);
        assert_eq!(pan_tilt_stop(), [0x81, 0x01, 0x06, 0x01, 0x18, 0x18, 0x03, 0x03, 0xFF]);
        // 30 degrees at 236 counts per degree is 7080, 0x01BA8; -10 degrees is -2360, 0xFF6C8 in five nibbles.
        assert_eq!(
            pan_tilt_absolute(0x18, 0x10, 7080, -2360),
            [0x81, 0x01, 0x06, 0x02, 0x18, 0x10, 0x00, 0x01, 0x0B, 0x0A, 0x08, 0x0F, 0x0F, 0x06, 0x0C, 0x08, 0xFF]
        );
        assert_eq!(pan_tilt_inquiry(), [0x81, 0x09, 0x06, 0x12, 0xFF]);
        assert_eq!(zoom_direct(0x2A5C), [0x81, 0x01, 0x04, 0x47, 0x02, 0x0A, 0x05, 0x0C, 0xFF]);
        assert_eq!(zoom_direct(0xFFFF), zoom_direct(0x4000), "zoom stops at optical tele");
        assert_eq!(zoom_inquiry(), [0x81, 0x09, 0x04, 0x47, 0xFF]);
        assert_eq!(focus_direct(0xF00D), [0x81, 0x01, 0x04, 0x48, 0x0F, 0x00, 0x00, 0x0D, 0xFF]);
        assert_eq!(focus_inquiry(), [0x81, 0x09, 0x04, 0x48, 0xFF]);
        assert_eq!(focus_manual(), [0x81, 0x01, 0x04, 0x38, 0x03, 0xFF]);
        assert_eq!(preset_recall(3), [0x81, 0x01, 0x04, 0x3F, 0x02, 0x02, 0xFF]);
    }

    #[test]
    fn commands_and_replies_read_back() {
        for cmd in [
            pan_tilt_drive(3, 4, Turn::Minus, Turn::Hold),
            pan_tilt_absolute(5, 6, -40120, 46020),
            pan_tilt_inquiry(),
            zoom_direct(0x1234),
            zoom_inquiry(),
            zoom_stop(),
            focus_direct(0xABCD),
            focus_inquiry(),
            focus_stop(),
            focus_manual(),
            preset_recall(12),
        ] {
            assert!(parse_command(&cmd).is_some(), "{cmd:02X?}");
        }
        assert_eq!(
            parse_command(&pan_tilt_absolute(5, 6, -40120, 46020)),
            Some(Command::Absolute { pan_step: 5, tilt_step: 6, pan: -40120, tilt: 46020 })
        );
        assert_eq!(parse_command(&preset_recall(12)), Some(Command::Recall(12)));

        assert_eq!(parse_reply(&ack(1)), Some(Reply::Ack));
        assert_eq!(parse_reply(&completion(1)), Some(Reply::Done(vec![])));
        assert_eq!(parse_reply(&error(2, 0x41)), Some(Reply::Error(0x41)));
        let Some(Reply::Done(data)) = parse_reply(&pan_tilt_reply(-7080, 2360)) else { panic!() };
        assert_eq!(parse_pan_tilt(&data), Some((-7080, 2360)));
        let Some(Reply::Done(data)) = parse_reply(&value_reply(0x4000)) else { panic!() };
        assert_eq!(parse_value(&data), Some(0x4000));
        assert_eq!(parse_reply(&[0x90, 0x50]), None, "no terminator");
    }

    #[test]
    fn speed_steps_run_from_a_twentieth_of_a_degree_to_sixty_and_blend_between() {
        let table = speed_table();
        assert_eq!(table.len(), TOP_STEP as usize);
        assert!((table[0] - 0.05).abs() < 1e-9 && (table[23] - 60.0).abs() < 1.0);
        assert!(table.windows(2).all(|w| w[1] > w[0]));

        let mut carry = 0.0;
        assert_eq!(step_for(&table, 0.005, &mut carry), (1, Turn::Hold), "too slow to move at all");
        assert_eq!(step_for(&table, -100.0, &mut carry), (24, Turn::Minus));
        assert_eq!(step_for(&table, table[9], &mut carry), (10, Turn::Plus));

        // Half way between two steps spends half the commands on each, and averages out.
        let want = (table[14] + table[15]) / 2.0;
        let mut carry = 0.0;
        let picks: Vec<u8> = (0..200).map(|_| step_for(&table, want, &mut carry).0).collect();
        assert!(picks.iter().all(|s| *s == 15 || *s == 16));
        let mean = picks.iter().map(|s| table[*s as usize - 1]).sum::<f64>() / 200.0;
        assert!((mean - want).abs() < want * 0.01, "{mean} against {want}");

        assert_eq!(step_at_least(&table, 60.0), 24);
        assert_eq!(step_at_least(&table, 0.001), 1);
        assert!(table[step_at_least(&table, 10.0) as usize - 1] >= 10.0);
    }
}
