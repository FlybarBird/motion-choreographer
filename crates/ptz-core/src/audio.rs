//! Audio decode (symphonia), duration, and waveform peak extraction.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecodedAudio {
    pub sample_rate: u32,
    pub channels: u16,
    /// Interleaved f32 samples in −1..1.
    pub samples: Vec<f32>,
    pub duration_secs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Waveform {
    /// Absolute peak per bucket, 0..1.
    pub peaks: Vec<f32>,
    pub duration_secs: f64,
}

pub fn decode_audio(path: &Path) -> Result<DecodedAudio> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .context("probe audio")?;
    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| anyhow!("no default audio track"))?
        .clone();
    let track_id = track.id;
    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| anyhow!("missing sample rate"))?;
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count() as u16)
        .unwrap_or(1);

    let mut samples: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymError::ResetRequired) => continue,
            Err(SymError::IoError(_)) => break,
            Err(e) => return Err(e.into()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio_buf) => {
                let mut sample_buf =
                    SampleBuffer::<f32>::new(audio_buf.capacity() as u64, *audio_buf.spec());
                sample_buf.copy_interleaved_ref(audio_buf);
                samples.extend_from_slice(sample_buf.samples());
            }
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }

    let frames = samples.len() / channels.max(1) as usize;
    let duration_secs = frames as f64 / sample_rate as f64;
    Ok(DecodedAudio {
        sample_rate,
        channels,
        samples,
        duration_secs,
    })
}

/// Downsample absolute peaks for timeline drawing.
pub fn waveform_peaks(audio: &DecodedAudio, buckets: usize) -> Waveform {
    let buckets = buckets.max(1);
    let ch = audio.channels.max(1) as usize;
    let frames = audio.samples.len() / ch;
    let mut peaks = vec![0.0f32; buckets];
    if frames == 0 {
        return Waveform {
            peaks,
            duration_secs: audio.duration_secs,
        };
    }
    for (i, frame_idx) in (0..frames).enumerate() {
        let mut peak = 0.0f32;
        for c in 0..ch {
            let s = audio.samples[frame_idx * ch + c].abs();
            if s > peak {
                peak = s;
            }
        }
        let bucket = (i * buckets) / frames;
        let bucket = bucket.min(buckets - 1);
        if peak > peaks[bucket] {
            peaks[bucket] = peak;
        }
    }
    Waveform {
        peaks,
        duration_secs: audio.duration_secs,
    }
}

/// Mix mono program + optional LTC tone into stereo according to routing.
pub fn render_output_frame(
    program_mono: f32,
    ltc_sample: f32,
    left_is_program: bool,
    right_is_ltc: bool,
    ltc_enabled: bool,
) -> (f32, f32) {
    let l = if left_is_program { program_mono } else { 0.0 };
    let r = if right_is_ltc && ltc_enabled {
        ltc_sample
    } else if right_is_ltc {
        0.0
    } else {
        0.0
    };
    (l, r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hound::{SampleFormat, WavSpec, WavWriter};
    use std::f32::consts::PI;
    use std::path::PathBuf;

    fn write_test_wav(path: &Path, duration_secs: f64, click_at: f64) {
        let sr = 48000u32;
        let n = (sr as f64 * duration_secs).round() as usize;
        let click = (click_at * sr as f64).round() as usize;
        let spec = WavSpec {
            channels: 1,
            sample_rate: sr,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        };
        let mut w = WavWriter::create(path, spec).unwrap();
        for i in 0..n {
            let t = i as f32 / sr as f32;
            let mut s = 0.02 * (2.0 * PI * 220.0 * t).sin();
            if (i as isize - click as isize).unsigned_abs() < (0.01 * sr as f32) as usize {
                let env = 1.0
                    - (i as f32 - click as f32).abs() / (0.01 * sr as f32);
                s = 0.95 * (2.0 * PI * 2000.0 * t).sin() * env;
            }
            w.write_sample((s * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn decode_duration_and_waveform_on_12_4s_wav() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures");
        std::fs::create_dir_all(&dir).unwrap();
        // Prefer repo fixture; regenerate locally if missing.
        let path = dir.join("test-click-12_4s.wav");
        if !path.exists() {
            write_test_wav(&path, 12.4, 2.5);
        }

        let audio = decode_audio(&path).expect("decode");
        assert!(
            (audio.duration_secs - 12.4).abs() < 0.05,
            "duration {}",
            audio.duration_secs
        );
        assert_ne!(
            (audio.duration_secs * 10.0).round() as i64,
            300,
            "must not be 30s"
        );

        let wave = waveform_peaks(&audio, 1240); // ~10ms buckets
        // Click at 2.5s → bucket ~250
        let click_bucket = ((2.5 / audio.duration_secs) * wave.peaks.len() as f64) as usize;
        let window = &wave.peaks[click_bucket.saturating_sub(3)..=(click_bucket + 3).min(wave.peaks.len() - 1)];
        let click_peak = window.iter().cloned().fold(0.0f32, f32::max);
        let early = &wave.peaks[0..50];
        let early_peak = early.iter().cloned().fold(0.0f32, f32::max);
        assert!(
            click_peak > early_peak * 5.0,
            "transient click should dominate bed: click={click_peak} early={early_peak}"
        );
    }
}
