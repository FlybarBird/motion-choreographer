//! The media library. Each imported file gets a folder under `media/<id>/`
//! holding the original, the decoded audio, a peak pyramid for waveforms and
//! a small record. Durations come from the decoded sample count, never from
//! the file's header.

use crate::storage::{read_json, write_atomic, write_json};
use ptz_core::api::{MediaInfo, MediaKind, PeakLevel};
use ptz_core::time::FrameRate;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Samples per bucket at the finest peak level.
pub const PEAK_BASE: u32 = 256;

const AUDIO_MAGIC: &[u8; 4] = b"PTZA";
const PEAKS_MAGIC: &[u8; 4] = b"PTZP";

pub const VIDEO_EXTENSIONS: [&str; 5] = ["mov", "mp4", "m4v", "mxf", "mkv"];
pub const AUDIO_EXTENSIONS: [&str; 5] = ["wav", "wave", "aif", "aiff", "mp3"];

pub fn extension(name: &str) -> String {
    Path::new(name).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase()
}

/// Decoded audio: interleaved f32 at the file's own rate.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub channels: u32,
    pub rate: u32,
    pub samples: Vec<f32>,
}

impl Decoded {
    pub fn frames(&self) -> u64 {
        (self.samples.len() / self.channels.max(1) as usize) as u64
    }

    pub fn seconds(&self) -> f64 {
        self.frames() as f64 / self.rate.max(1) as f64
    }

    pub fn channel(&self, index: u32) -> Vec<f32> {
        self.samples.iter().skip(index as usize).step_by(self.channels.max(1) as usize).copied().collect()
    }

    /// The channels averaged into one, leaving out `exclude` (a channel that carries timecode).
    pub fn mono(&self, exclude: Option<u32>) -> Vec<f32> {
        let ch = self.channels.max(1) as usize;
        let used: Vec<usize> = (0..ch).filter(|c| Some(*c as u32) != exclude).collect();
        if used.is_empty() {
            return vec![0.0; self.frames() as usize];
        }
        let scale = 1.0 / used.len() as f32;
        self.samples.chunks_exact(ch).map(|f| used.iter().map(|c| f[*c]).sum::<f32>() * scale).collect()
    }
}

/// Decode a WAV, AIFF or MP3 file completely.
pub fn decode_file(path: &Path, hint_ext: &str) -> Result<Decoded, String> {
    use symphonia::core::audio::sample::Sample;
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let file = fs::File::open(path).map_err(|e| format!("could not open the file: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if !hint_ext.is_empty() {
        hint.with_extension(hint_ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("not a WAV, AIFF or MP3 file ({e})"))?;
    let track = format.default_track(TrackType::Audio).ok_or("the file has no audio track")?;
    let track_id = track.id;
    let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("the file has no audio track")?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| format!("this audio encoding is not supported ({e})"))?;

    let mut out = Decoded { channels: 0, rate: 0, samples: Vec::new() };
    let mut scratch: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            // A truncated file ends here; what was decoded so far is kept.
            Err(Error::IoError(_)) => break,
            Err(e) => return Err(format!("the file is damaged ({e})")),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                let (rate, channels) = (buf.spec().rate(), buf.spec().channels().count() as u32);
                if out.channels == 0 {
                    out.channels = channels;
                    out.rate = rate;
                } else if out.channels != channels || out.rate != rate {
                    return Err("the audio format changes part way through the file".into());
                }
                scratch.resize(buf.samples_interleaved(), f32::MID);
                buf.copy_to_slice_interleaved(&mut scratch);
                out.samples.extend_from_slice(&scratch);
            }
            Err(Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("the file is damaged ({e})")),
        }
    }
    if out.channels == 0 || out.rate == 0 || out.samples.is_empty() {
        return Err("the file holds no audio".into());
    }
    Ok(out)
}

// ---- decoded audio on disk ----

pub fn write_audio(path: &Path, d: &Decoded) -> std::io::Result<()> {
    let mut bytes = Vec::with_capacity(24 + d.samples.len() * 4);
    bytes.extend_from_slice(AUDIO_MAGIC);
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&d.channels.to_le_bytes());
    bytes.extend_from_slice(&d.rate.to_le_bytes());
    bytes.extend_from_slice(&d.frames().to_le_bytes());
    for s in &d.samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    write_atomic(path, &bytes)
}

pub fn read_audio(path: &Path) -> Result<Decoded, String> {
    let mut bytes = Vec::new();
    fs::File::open(path).and_then(|mut f| f.read_to_end(&mut bytes)).map_err(|e| format!("could not read decoded audio: {e}"))?;
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if bytes.get(..4) != Some(AUDIO_MAGIC.as_slice()) || word(4) != Some(1) {
        return Err("decoded audio is not in the expected format".into());
    }
    let (channels, rate) = (word(8).unwrap_or(0), word(12).unwrap_or(0));
    let samples: Vec<f32> = bytes.get(24..).unwrap_or(&[]).as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
    if channels == 0 || rate == 0 {
        return Err("decoded audio is not in the expected format".into());
    }
    Ok(Decoded { channels, rate, samples })
}

// ---- peaks ----

/// One level: for each bucket, for each channel, the lowest and highest sample.
#[derive(Clone, Debug, PartialEq)]
pub struct PeakTable {
    pub samples_per_bucket: u32,
    pub channels: u32,
    /// `[min, max]` per channel per bucket.
    pub data: Vec<f32>,
}

impl PeakTable {
    pub fn buckets(&self) -> usize {
        self.data.len() / (2 * self.channels.max(1) as usize)
    }

    pub fn range(&self, bucket: usize, channel: u32) -> (f32, f32) {
        let at = (bucket * self.channels as usize + channel as usize) * 2;
        (self.data[at], self.data[at + 1])
    }

    /// Fold the channels together and scale for the wire.
    pub fn level(&self, sample_rate: u32) -> PeakLevel {
        let ch = self.channels.max(1) as usize;
        let pairs = self
            .data
            .chunks_exact(2 * ch)
            .map(|b| {
                let lo = b.iter().step_by(2).copied().fold(0.0f32, f32::min);
                let hi = b.iter().skip(1).step_by(2).copied().fold(0.0f32, f32::max);
                // Round outward so a quiet signal still shows.
                ((lo.clamp(-1.0, 0.0) * 127.0).floor() as i8, (hi.clamp(0.0, 1.0) * 127.0).ceil() as i8)
            })
            .collect();
        PeakLevel { samples_per_bucket: self.samples_per_bucket, sample_rate, pairs }
    }
}

/// The pyramid: 256 samples per bucket, then each level half as long as the one before.
pub fn build_peaks(d: &Decoded) -> Vec<PeakTable> {
    let ch = d.channels.max(1) as usize;
    let mut first = PeakTable { samples_per_bucket: PEAK_BASE, channels: d.channels, data: Vec::new() };
    for block in d.samples.chunks(PEAK_BASE as usize * ch) {
        for c in 0..ch {
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for s in block.iter().skip(c).step_by(ch) {
                lo = lo.min(*s);
                hi = hi.max(*s);
            }
            first.data.push(lo);
            first.data.push(hi);
        }
    }
    let mut levels = vec![first];
    while levels.last().is_some_and(|l| l.buckets() > 64) {
        let prev = levels.last().expect("non-empty");
        let mut next = PeakTable { samples_per_bucket: prev.samples_per_bucket * 2, channels: prev.channels, data: Vec::new() };
        for pair in prev.data.chunks(4 * ch) {
            for c in 0..ch {
                let a = (pair[c * 2], pair[c * 2 + 1]);
                let b = if pair.len() > 2 * ch { (pair[2 * ch + c * 2], pair[2 * ch + c * 2 + 1]) } else { a };
                next.data.push(a.0.min(b.0));
                next.data.push(a.1.max(b.1));
            }
        }
        levels.push(next);
    }
    levels
}

pub fn write_peaks(path: &Path, levels: &[PeakTable]) -> std::io::Result<()> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(PEAKS_MAGIC);
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&(levels.len() as u32).to_le_bytes());
    for l in levels {
        bytes.extend_from_slice(&l.samples_per_bucket.to_le_bytes());
        bytes.extend_from_slice(&l.channels.to_le_bytes());
        bytes.extend_from_slice(&(l.data.len() as u32).to_le_bytes());
        for v in &l.data {
            bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    write_atomic(path, &bytes)
}

pub fn read_peaks(path: &Path) -> Option<Vec<PeakTable>> {
    let bytes = fs::read(path).ok()?;
    let word = |at: usize| Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?));
    if bytes.get(..4)? != PEAKS_MAGIC || word(4)? != 1 {
        return None;
    }
    let count = word(8)?;
    let mut at = 12;
    let mut levels = Vec::new();
    for _ in 0..count {
        let (samples_per_bucket, channels, len) = (word(at)?, word(at + 4)?, word(at + 8)? as usize);
        at += 12;
        let body = bytes.get(at..at + len * 4)?;
        at += len * 4;
        let data = body.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        levels.push(PeakTable { samples_per_bucket, channels, data });
    }
    Some(levels)
}

/// The coarsest level that still has at least one bucket per `samples_per_pixel`.
pub fn pick_level(levels: &[PeakTable], samples_per_pixel: u32) -> Option<&PeakTable> {
    levels.iter().rev().find(|l| l.samples_per_bucket <= samples_per_pixel).or(levels.first())
}

// ---- WAV writing, for the seeded click and for tests ----

pub mod wav {
    use super::*;

    /// 16-bit PCM. `time_reference` adds a broadcast `bext` chunk carrying the
    /// start time in samples since midnight.
    pub fn bytes(rate: u32, channels: u16, samples: &[f32], time_reference: Option<u64>) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut bext = Vec::new();
        if let Some(t) = time_reference {
            // Description 256, originator 32, reference 32, date 10, time 8,
            // then the time reference (low, high), version, UMID 64, reserved 190.
            let mut body = vec![0u8; 256 + 32 + 32 + 10 + 8];
            body.extend_from_slice(&(t as u32).to_le_bytes());
            body.extend_from_slice(&((t >> 32) as u32).to_le_bytes());
            body.extend_from_slice(&1u16.to_le_bytes());
            body.extend_from_slice(&[0u8; 64 + 190]);
            bext.extend_from_slice(b"bext");
            bext.extend_from_slice(&(body.len() as u32).to_le_bytes());
            bext.extend_from_slice(&body);
        }
        let mut out = Vec::with_capacity(44 + bext.len() + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + bext.len() as u32 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(&bext);
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&channels.to_le_bytes());
        out.extend_from_slice(&rate.to_le_bytes());
        out.extend_from_slice(&(rate * channels as u32 * 2).to_le_bytes());
        out.extend_from_slice(&(channels * 2).to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
        }
        out
    }

    pub fn write(path: &Path, rate: u32, channels: u16, samples: &[f32], time_reference: Option<u64>) -> std::io::Result<()> {
        let mut f = fs::File::create(path)?;
        f.write_all(&bytes(rate, channels, samples, time_reference))
    }
}

/// The click fixture: `seconds` of silence with one short click at `click_at`.
pub fn click_track(rate: u32, seconds: f64, click_at: f64) -> Vec<f32> {
    let mut samples = vec![0.0f32; (seconds * rate as f64).round() as usize];
    let start = (click_at * rate as f64).round() as usize;
    let len = (0.004 * rate as f64) as usize;
    for i in 0..len {
        if let Some(s) = samples.get_mut(start + i) {
            let t = i as f32 / rate as f32;
            let fade = 1.0 - i as f32 / len as f32;
            *s = (t * 2.0 * std::f32::consts::PI * 1500.0).cos() * 0.9 * fade;
        }
    }
    samples
}

/// The seeded bed: a quiet chord that swells and falls, with a tick on every
/// second so the waveform shows where the seconds are.
pub fn bed_track(rate: u32, seconds: f64) -> Vec<f32> {
    let n = (seconds * rate as f64).round() as usize;
    let tau = 2.0 * std::f32::consts::PI;
    (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let swell = 0.55 + 0.45 * (t * tau / 8.0 - tau / 4.0).sin();
            let chord = [220.0f32, 277.18, 329.63].iter().map(|f| (t * tau * f).sin()).sum::<f32>() / 3.0;
            let into_second = t.fract();
            let tick = if into_second < 0.004 { (into_second * tau * 1500.0).cos() * (1.0 - into_second / 0.004) } else { 0.0 };
            let fade = (t / 0.5).min(1.0).min((seconds as f32 - t) / 0.5).max(0.0);
            (chord * 0.28 * swell + tick * 0.5) * fade
        })
        .collect()
}

// ---- the library ----

pub fn info_for(id: &str, name: &str, kind: MediaKind, d: &Decoded, rate: FrameRate) -> MediaInfo {
    let seconds = d.seconds();
    MediaInfo {
        id: id.to_string(),
        name: name.to_string(),
        kind,
        channels: d.channels,
        sample_rate: d.rate,
        samples: d.frames(),
        duration_s: seconds,
        duration_frames: duration_frames(d.frames(), d.rate, rate),
        start_tc: None,
        start_tc_source: None,
        proxy_frames: 0,
        ltc_channel: None,
        start_samples: None,
        proxy_rate: None,
    }
}

// ---- start timecode ----

pub const STAMP_BEXT: &str = "bext";
pub const STAMP_QUICKTIME: &str = "quicktime";
pub const STAMP_TYPED: &str = "typed";

/// A broadcast WAV's time reference: the file's start as samples since midnight.
/// Zero is what software writes when it has no time to give, so it counts as none.
pub fn bext_time_reference(path: &Path) -> Option<u64> {
    use std::io::{Seek, SeekFrom};
    let mut f = fs::File::open(path).ok()?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if !matches!(&head[..4], b"RIFF" | b"RF64" | b"BW64") || &head[8..] != b"WAVE" {
        return None;
    }
    loop {
        let mut chunk = [0u8; 8];
        f.read_exact(&mut chunk).ok()?;
        let len = u32::from_le_bytes(chunk[4..].try_into().ok()?) as u64;
        if &chunk[..4] == b"bext" {
            // Description 256, originator 32, originator reference 32, date 10, time 8, then the reference.
            let mut body = [0u8; 346];
            if len < body.len() as u64 {
                return None;
            }
            f.read_exact(&mut body).ok()?;
            let low = u32::from_le_bytes(body[338..342].try_into().ok()?) as u64;
            let high = u32::from_le_bytes(body[342..346].try_into().ok()?) as u64;
            return Some((high << 32) | low).filter(|t| *t > 0);
        }
        f.seek(SeekFrom::Current((len + (len & 1)) as i64)).ok()?;
    }
}

/// Restate what depends on the project rate: the length in frames, and the
/// start timecode of a broadcast WAV.
pub fn restate(info: &mut MediaInfo, rate: FrameRate) {
    info.duration_frames = duration_frames(info.samples, info.sample_rate, rate);
    if let (Some(samples), Some(STAMP_BEXT)) = (info.start_samples, info.start_tc_source.as_deref()) {
        info.start_tc = Some(ptz_core::time::timecode_of_samples(samples, info.sample_rate, rate).to_string());
    }
}

/// The start timecode of a file that carries LTC on one of its channels: the
/// first decoded frame, backed up to the file's first sample.
pub fn ltc_start(d: &Decoded, channel: u32, rate: FrameRate) -> Result<ptz_core::Timecode, String> {
    use ptz_core::ltc::LtcDecoder;
    let shown = channel + 1;
    if channel >= d.channels {
        return Err(format!(
            "the file has {} channel{}, so there is no channel {shown}",
            d.channels,
            if d.channels == 1 { "" } else { "s" }
        ));
    }
    let samples = d.channel(channel);
    let mut decoder = LtcDecoder::new();
    let mut readings = Vec::new();
    for block in samples.chunks(48_000) {
        readings.extend(decoder.feed(block));
        if readings.len() >= 3 {
            break;
        }
    }
    let Some(first) = readings.first() else {
        return Err(format!("no LTC was found on channel {shown}"));
    };
    // The frame length is measured best between two sync words.
    let measured = readings.get(1).unwrap_or(first).samples_per_frame.max(1.0);
    let family = crate::lock::ltc_family(first.tc.drop_frame, d.rate as f64 / measured);
    if !family.matches(rate) {
        return Err(format!("channel {shown} carries {} fps LTC and the project is {} fps", family.label(), rate.label()));
    }
    if !first.tc.is_valid(rate) {
        return Err(format!("the LTC on channel {shown} does not read as timecode at {} fps", rate.label()));
    }
    // When the frame's last bit ended the code was one frame on; the file began `end_sample` earlier.
    let before = first.end_sample as f64 * rate.fps() / d.rate as f64;
    let start = first.tc.to_frames(rate) as f64 + 1.0 - before;
    Ok(ptz_core::Timecode::from_frames(start.round() as i64, rate))
}

pub fn ltc_start_of_file(audio: &Path, channel: u32, rate: FrameRate) -> Result<ptz_core::Timecode, String> {
    ltc_start(&read_audio(audio)?, channel, rate)
}

/// Whole timeline frames the media covers at the project rate, rounded up so the last samples are reachable.
pub fn duration_frames(samples: u64, sample_rate: u32, rate: FrameRate) -> i64 {
    let n = samples as i128 * rate.num() as i128;
    let d = sample_rate.max(1) as i128 * rate.den() as i128;
    ((n + d - 1) / d) as i64
}

/// What an import produced, before it is added to the show.
pub struct Ingested {
    pub info: MediaInfo,
    pub decoded: Decoded,
}

pub fn original_path(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.file_stem().and_then(|s| s.to_str()) == Some("original"))
}

/// Turn an uploaded file into a media folder. On failure the folder is
/// removed and the decoder's message is returned.
pub fn ingest(source: &Path, dir: &Path, id: &str, name: &str, rate: FrameRate) -> Result<Ingested, String> {
    let result = ingest_inner(source, dir, id, name, rate);
    if result.is_err() {
        let _ = fs::remove_dir_all(dir);
    }
    let _ = fs::remove_file(source);
    result
}

fn ingest_inner(source: &Path, dir: &Path, id: &str, name: &str, rate: FrameRate) -> Result<Ingested, String> {
    let ext = extension(name);
    let io = |e: std::io::Error| format!("could not write to the media folder: {e}");
    fs::create_dir_all(dir).map_err(io)?;
    let original = dir.join(if ext.is_empty() { "original".to_string() } else { format!("original.{ext}") });
    if fs::rename(source, &original).is_err() {
        fs::copy(source, &original).map_err(io)?;
    }
    let is_video = VIDEO_EXTENSIONS.contains(&ext.as_str());
    let (decoded, kind) = if is_video {
        (crate::video::extract_audio(&original, dir)?, MediaKind::Video)
    } else {
        (decode_file(&original, &ext)?, MediaKind::Audio)
    };
    write_audio(&dir.join("audio.f32"), &decoded).map_err(io)?;
    write_peaks(&dir.join("peaks.bin"), &build_peaks(&decoded)).map_err(io)?;
    let mut info = info_for(id, name, kind, &decoded, rate);
    crate::video::after_ingest(&original, dir, rate, &mut info);
    write_json(&dir.join("info.json"), &info).map_err(io)?;
    Ok(Ingested { info, decoded })
}

/// Every media record on disk. Durations are restated at the current project rate.
pub fn load_all(root: &Path, rate: FrameRate) -> Vec<MediaInfo> {
    let mut out: Vec<MediaInfo> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| read_json::<MediaInfo>(&e.path().join("info.json")))
        .map(|mut m| {
            restate(&mut m, rate);
            m
        })
        .collect();
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.id.cmp(&b.id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ptz-media-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn tone(rate: u32, seconds: f64, channels: u16) -> Vec<f32> {
        let n = (seconds * rate as f64).round() as usize;
        (0..n)
            .flat_map(|i| {
                let v = (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / rate as f32).sin() * 0.5;
                (0..channels).map(move |c| if c == 0 { v } else { -v * 0.5 })
            })
            .collect()
    }

    /// AIFF: big-endian 16-bit PCM, rate as an 80-bit float.
    fn aiff_bytes(rate: u32, channels: u16, samples: &[f32]) -> Vec<u8> {
        let frames = (samples.len() / channels as usize) as u32;
        let mut ext = [0u8; 10];
        let exp = 31 - rate.leading_zeros();
        ext[..2].copy_from_slice(&((16383 + exp) as u16).to_be_bytes());
        ext[2..10].copy_from_slice(&((rate as u64) << (63 - exp)).to_be_bytes());
        let mut comm = Vec::new();
        comm.extend_from_slice(&channels.to_be_bytes());
        comm.extend_from_slice(&frames.to_be_bytes());
        comm.extend_from_slice(&16u16.to_be_bytes());
        comm.extend_from_slice(&ext);
        let mut ssnd = vec![0u8; 8];
        for s in samples {
            ssnd.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_be_bytes());
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"FORM");
        out.extend_from_slice(&((4 + 8 + comm.len() + 8 + ssnd.len()) as u32).to_be_bytes());
        out.extend_from_slice(b"AIFF");
        out.extend_from_slice(b"COMM");
        out.extend_from_slice(&(comm.len() as u32).to_be_bytes());
        out.extend_from_slice(&comm);
        out.extend_from_slice(b"SSND");
        out.extend_from_slice(&(ssnd.len() as u32).to_be_bytes());
        out.extend_from_slice(&ssnd);
        out
    }

    #[test]
    fn wav_decodes_to_the_exact_sample_count() {
        let dir = temp("wav");
        for (rate, channels, seconds) in [(48_000u32, 1u16, 1.0), (44_100, 2, 2.5), (96_000, 2, 0.25)] {
            let samples = tone(rate, seconds, channels);
            let path = dir.join("t.wav");
            wav::write(&path, rate, channels, &samples, None).unwrap();
            let d = decode_file(&path, "wav").unwrap();
            assert_eq!((d.rate, d.channels), (rate, channels as u32));
            assert_eq!(d.samples.len(), samples.len());
            let worst = d.samples.iter().zip(&samples).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1.0 / 16_000.0, "{worst}");
        }
        // A broadcast chunk before the format chunk does not disturb decoding.
        let samples = tone(48_000, 0.5, 2);
        let path = dir.join("b.wav");
        wav::write(&path, 48_000, 2, &samples, Some(172_800_000)).unwrap();
        assert_eq!(decode_file(&path, "wav").unwrap().samples.len(), samples.len());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn aiff_decodes_to_the_exact_sample_count() {
        let dir = temp("aiff");
        for (rate, channels) in [(48_000u32, 2u16), (44_100, 1)] {
            let samples = tone(rate, 1.5, channels);
            let path = dir.join("t.aiff");
            fs::write(&path, aiff_bytes(rate, channels, &samples)).unwrap();
            let d = decode_file(&path, "aiff").unwrap();
            assert_eq!((d.rate, d.channels), (rate, channels as u32));
            assert_eq!(d.samples.len(), samples.len());
            let worst = d.samples.iter().zip(&samples).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(worst < 1.0 / 16_000.0, "{worst}");
        }
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mp3_decodes_to_the_expected_length() {
        let dir = temp("mp3");
        let wav_path = dir.join("t.wav");
        wav::write(&wav_path, 44_100, 2, &tone(44_100, 3.0, 2), None).unwrap();
        let mp3 = dir.join("t.mp3");
        let made = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-i"])
            .arg(&wav_path)
            .args(["-codec:a", "libmp3lame", "-b:a", "128k"])
            .arg(&mp3)
            .status();
        if !matches!(made, Ok(s) if s.success()) {
            eprintln!("skipped: ffmpeg with libmp3lame is needed to make the MP3 fixture");
            let _ = fs::remove_dir_all(dir);
            return;
        }
        let d = decode_file(&mp3, "mp3").unwrap();
        assert_eq!((d.rate, d.channels), (44_100, 2));
        // The encoder's delay and padding are trimmed, so the length matches the source.
        assert_eq!(d.frames(), 3 * 44_100, "decoded {} frames", d.frames());
        let loud = d.channel(0).iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!((loud - 0.5).abs() < 0.1, "{loud}");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_that_is_not_audio_is_refused_and_leaves_nothing_behind() {
        let dir = temp("bad");
        let src = dir.join("upload.tmp");
        fs::write(&src, b"this is a text file, not audio").unwrap();
        let media = dir.join("media").join("notes");
        let err = ingest(&src, &media, "notes", "notes.wav", FrameRate::F25).err().unwrap();
        assert!(err.contains("not a WAV, AIFF or MP3 file"), "{err}");
        assert!(!media.exists());
        assert!(!src.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_click_fixture_is_12_4_seconds_and_the_click_is_in_the_right_bucket_at_every_level() {
        let dir = temp("click");
        let src = dir.join("upload.tmp");
        wav::write(&src, 48_000, 1, &click_track(48_000, 12.4, 2.5), None).unwrap();
        let media = dir.join("media").join("click");
        let got = ingest(&src, &media, "click", "Click 12.4s.wav", FrameRate::F25).unwrap();
        assert_eq!(got.info.samples, 595_200);
        assert!((got.info.duration_s - 12.4).abs() < 1e-9);
        assert_eq!(got.info.duration_frames, 310);
        assert_eq!((got.info.channels, got.info.sample_rate, got.info.kind), (1, 48_000, MediaKind::Audio));
        assert_eq!(load_all(&dir.join("media"), FrameRate::F30)[0].duration_frames, 372);

        let levels = read_peaks(&media.join("peaks.bin")).unwrap();
        assert_eq!(levels, build_peaks(&got.decoded));
        assert!(levels.len() >= 5, "{} levels", levels.len());
        for (i, level) in levels.iter().enumerate() {
            assert_eq!(level.samples_per_bucket, PEAK_BASE << i);
            let want = (2.5 * 48_000.0) as usize / level.samples_per_bucket as usize;
            let loudest = (0..level.buckets())
                .max_by(|a, b| {
                    let size = |n: usize| level.range(n, 0).1 - level.range(n, 0).0;
                    size(*a).total_cmp(&size(*b))
                })
                .unwrap();
            assert_eq!(loudest, want, "level {i}");
            // The click is 4 ms long, so at the finest level it runs on into the next bucket.
            let want_end = ((2.5 + 0.004) * 48_000.0) as usize / level.samples_per_bucket as usize;
            let stray =
                (0..level.buckets()).filter(|b| !(want..=want_end).contains(b) && level.range(*b, 0) != (0.0, 0.0)).count();
            assert_eq!(stray, 0, "level {i}: only the click's buckets have signal");
            // The wire form keeps the click in the same place.
            let wire = PeakLevel::from_bytes(&level.level(48_000).to_bytes()).unwrap();
            assert_eq!(wire.pairs.len(), level.buckets());
            assert!(wire.pairs[want].1 > 100 && wire.pairs[want].0 < -50, "{:?}", wire.pairs[want]);
        }
        // Zoomed out asks for a coarse level, zoomed in for the finest.
        assert_eq!(pick_level(&levels, 5000).unwrap().samples_per_bucket, 4096);
        assert_eq!(pick_level(&levels, 256).unwrap().samples_per_bucket, 256);
        assert_eq!(pick_level(&levels, 10).unwrap().samples_per_bucket, 256);

        let back = read_audio(&media.join("audio.f32")).unwrap();
        assert_eq!(back, got.decoded);
        assert!(original_path(&media).unwrap().ends_with("original.wav"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn mono_mix_averages_and_can_leave_a_channel_out() {
        let d = Decoded { channels: 2, rate: 48_000, samples: vec![1.0, 0.0, 0.5, 0.5, -1.0, 1.0] };
        assert_eq!(d.mono(None), vec![0.5, 0.5, 0.0]);
        assert_eq!(d.mono(Some(1)), vec![1.0, 0.5, -1.0]);
        assert_eq!(d.channel(1), vec![0.0, 0.5, 1.0]);
        assert_eq!(duration_frames(48_000, 48_000, FrameRate::F25), 25);
        assert_eq!(duration_frames(48_001, 48_000, FrameRate::F25), 26);
    }
}
