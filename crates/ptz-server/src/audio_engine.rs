//! Server-side audio playback synced to the show clock.
//!
//! Uses cpal for output when available. On headless / missing-device environments
//! falls back to a silent clock-synced engine so transport tests still pass.
//!
//! The cpal `Stream` is owned by a dedicated OS thread so `AudioEngine` stays
//! `Send + Sync` for the Tokio show server.
//!
//! Routing (demo default): program mono → left, LTC generator → right (off).

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use ptz_core::{
    audio::render_output_frame, AudioChannelRoute, AudioRouting, DecodedAudio,
};
use std::sync::Arc;
use std::thread;
use tracing::{info, warn};

struct SharedPlay {
    samples: Vec<f32>, // mono program
    sample_rate: u32,
    /// Playback cursor in program samples (mono frames).
    cursor: f64,
    playing: bool,
    routing: AudioRouting,
    /// LTC tone phase accumulator.
    ltc_phase: f64,
}

pub struct AudioEngine {
    inner: Arc<Mutex<SharedPlay>>,
}

impl AudioEngine {
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(SharedPlay {
            samples: Vec::new(),
            sample_rate: 48000,
            cursor: 0.0,
            playing: false,
            routing: AudioRouting::default(),
            ltc_phase: 0.0,
        }));
        // Skip cpal in CI/VMs without a real sound device, or when forced silent.
        let silent = std::env::var_os("PTZ_SILENT_AUDIO").is_some()
            || !std::path::Path::new("/dev/snd").exists();
        if silent {
            warn!("silent audio engine (no /dev/snd or PTZ_SILENT_AUDIO set)");
        } else {
            let for_thread = inner.clone();
            thread::Builder::new()
                .name("ptz-audio".into())
                .spawn(move || match Self::try_start_cpal(for_thread) {
                    Ok(stream) => {
                        info!("audio output started (cpal default device)");
                        loop {
                            thread::park();
                            let _ = &stream;
                        }
                    }
                    Err(e) => {
                        warn!("audio output unavailable ({e}); running silent engine");
                    }
                })
                .ok();
        }
        Self { inner }
    }

    fn try_start_cpal(inner: Arc<Mutex<SharedPlay>>) -> Result<cpal::Stream, anyhow::Error> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("no default output device"))?;
        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        {
            let mut g = inner.lock();
            g.sample_rate = sample_rate;
        }
        let err_fn = |e| warn!("audio stream error: {e}");
        let stream = match config.sample_format() {
            cpal::SampleFormat::F32 => {
                let stream_config: cpal::StreamConfig = config.into();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [f32], _| {
                        fill_output(data, channels, &inner);
                    },
                    err_fn,
                    None,
                )?
            }
            cpal::SampleFormat::I16 => {
                let stream_config: cpal::StreamConfig = config.into();
                device.build_output_stream(
                    &stream_config,
                    move |data: &mut [i16], _| {
                        let mut tmp = vec![0.0f32; data.len()];
                        fill_output(&mut tmp, channels, &inner);
                        for (o, i) in data.iter_mut().zip(tmp.iter()) {
                            *o = (i.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                        }
                    },
                    err_fn,
                    None,
                )?
            }
            other => return Err(anyhow::anyhow!("unsupported sample format {other:?}")),
        };
        stream.play()?;
        Ok(stream)
    }

    pub fn load_program(&self, audio: &DecodedAudio) {
        let mut g = self.inner.lock();
        let ch = audio.channels.max(1) as usize;
        let frames = audio.samples.len() / ch;
        let mut mono = Vec::with_capacity(frames);
        for i in 0..frames {
            let mut acc = 0.0f32;
            for c in 0..ch {
                acc += audio.samples[i * ch + c];
            }
            mono.push(acc / ch as f32);
        }
        g.samples = mono;
        g.sample_rate = audio.sample_rate;
        g.cursor = 0.0;
    }

    pub fn clear(&self) {
        let mut g = self.inner.lock();
        g.samples.clear();
        g.cursor = 0.0;
        g.playing = false;
    }

    pub fn set_routing(&self, routing: AudioRouting) {
        self.inner.lock().routing = routing;
    }

    pub fn play(&self) {
        self.inner.lock().playing = true;
    }

    pub fn pause(&self) {
        self.inner.lock().playing = false;
    }

    pub fn seek_seconds(&self, seconds: f64) {
        let mut g = self.inner.lock();
        g.cursor = (seconds * g.sample_rate as f64).max(0.0);
    }

    pub fn cue(&self) {
        self.seek_seconds(0.0);
        self.pause();
    }

    pub fn is_playing(&self) -> bool {
        self.inner.lock().playing
    }

    pub fn cursor_seconds(&self) -> f64 {
        let g = self.inner.lock();
        // When silent (no cpal callback), advance cursor wall-clock style if playing.
        // The cpal callback advances cursor; for silent mode we approximate via a
        // separate mechanism in tick by preferring InternalClock. Still expose cursor.
        g.cursor / g.sample_rate.max(1) as f64
    }

    /// Advance silent engine cursor by wall seconds (used when no audio device).
    pub fn silent_advance(&self, dt_secs: f64) {
        let mut g = self.inner.lock();
        if g.playing {
            g.cursor += dt_secs * g.sample_rate as f64;
        }
    }
}

fn fill_output(data: &mut [f32], channels: usize, inner: &Arc<Mutex<SharedPlay>>) {
    let mut g = inner.lock();
    let playing = g.playing;
    let left_prog = matches!(g.routing.left, AudioChannelRoute::Program);
    let right_ltc = matches!(g.routing.right, AudioChannelRoute::LtcGenerator);
    let ltc_on = g.routing.ltc_generator_enabled;
    let sr = g.sample_rate.max(1) as f64;

    for frame in data.chunks_mut(channels) {
        let prog = if playing {
            let idx = g.cursor as usize;
            let sample = if idx < g.samples.len() {
                g.samples[idx]
            } else {
                0.0
            };
            g.cursor += 1.0;
            sample
        } else {
            0.0
        };

        let ltc = if ltc_on && playing {
            let s = (g.ltc_phase * std::f64::consts::TAU).sin() as f32 * 0.2;
            g.ltc_phase = (g.ltc_phase + 1000.0 / sr) % 1.0;
            s
        } else {
            0.0
        };

        let (l, r) = render_output_frame(prog, ltc, left_prog, right_ltc, ltc_on);
        if channels >= 1 {
            frame[0] = l;
        }
        if channels >= 2 {
            frame[1] = r;
        }
        for ch in frame.iter_mut().skip(2) {
            *ch = 0.0;
        }
    }
}
