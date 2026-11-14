//! Server-side audio playback synced to the show clock.
//!
//! Uses cpal for output when available. On headless / missing-device environments
//! falls back to a silent clock-synced engine so transport tests still pass.
//!
//! The cpal `Stream` is owned by a dedicated OS thread so `AudioEngine` stays
//! `Send + Sync` for the Tokio show server.
//!
//! Routing (demo default): program mono → left, LTC generator → right (off).
//!
//! macOS: always attempt the default cpal output (Core Audio). The Linux-only
//! `/dev/snd` heuristic must never silence the Mac build.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use parking_lot::Mutex;
use ptz_core::{AudioChannelRoute, AudioRouting, DecodedAudio};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tracing::{info, warn};

struct SharedPlay {
    /// Mono program PCM at `program_sample_rate`.
    samples: Vec<f32>,
    program_sample_rate: u32,
    /// Device callback rate (cpal). Used to advance the program cursor.
    output_sample_rate: u32,
    /// Playback cursor in program samples.
    cursor: f64,
    playing: bool,
    routing: AudioRouting,
    ltc_phase: f64,
    /// Output device name, or "silent".
    device_name: String,
    /// Why we're silent / how we opened, for status + logs.
    silent: bool,
    reason: Option<String>,
}

pub struct AudioEngine {
    inner: Arc<Mutex<SharedPlay>>,
}

impl AudioEngine {
    pub fn new() -> Self {
        let inner = Arc::new(Mutex::new(SharedPlay {
            samples: Vec::new(),
            program_sample_rate: 48000,
            output_sample_rate: 48000,
            cursor: 0.0,
            playing: false,
            routing: AudioRouting::default(),
            ltc_phase: 0.0,
            device_name: "silent".into(),
            silent: true,
            reason: Some("initializing".into()),
        }));

        if std::env::var_os("PTZ_SILENT_AUDIO").is_some() {
            let reason = "PTZ_SILENT_AUDIO set".to_string();
            warn!("silent audio engine ({reason})");
            {
                let mut g = inner.lock();
                g.silent = true;
                g.device_name = "silent".into();
                g.reason = Some(reason);
            }
            return Self { inner };
        }

        // Linux CI/VMs without ALSA: skip probing. Never apply this on macOS.
        #[cfg(target_os = "linux")]
        {
            if !std::path::Path::new("/dev/snd").exists() {
                let reason = "no /dev/snd (Linux headless)".to_string();
                warn!("silent audio engine ({reason})");
                {
                    let mut g = inner.lock();
                    g.silent = true;
                    g.device_name = "silent".into();
                    g.reason = Some(reason);
                }
                return Self { inner };
            }
        }

        let for_thread = inner.clone();
        let (tx, rx) = mpsc::channel();
        if thread::Builder::new()
            .name("ptz-audio".into())
            .spawn(move || match Self::try_start_cpal(for_thread.clone()) {
                Ok((stream, name, out_sr)) => {
                    {
                        let mut g = for_thread.lock();
                        g.silent = false;
                        g.device_name = name.clone();
                        g.output_sample_rate = out_sr;
                        g.reason = None;
                    }
                    let _ = tx.send(Ok((name, out_sr)));
                    info!("audio output started via cpal");
                    loop {
                        thread::park();
                        let _ = &stream;
                    }
                }
                Err(e) => {
                    let reason = e.to_string();
                    {
                        let mut g = for_thread.lock();
                        g.silent = true;
                        g.device_name = "silent".into();
                        g.reason = Some(reason.clone());
                    }
                    warn!("audio output unavailable ({reason}); running silent engine");
                    let _ = tx.send(Err(reason));
                }
            })
            .is_err()
        {
            let mut g = inner.lock();
            g.silent = true;
            g.device_name = "silent".into();
            g.reason = Some("failed to spawn audio thread".into());
        } else {
            // Wait briefly so status reflects the real device after construction.
            match rx.recv_timeout(Duration::from_secs(3)) {
                Ok(Ok((name, sr))) => {
                    info!("cpal default output: {name} @ {sr} Hz");
                }
                Ok(Err(_)) => {}
                Err(_) => {
                    let mut g = inner.lock();
                    if g.reason.as_deref() == Some("initializing") {
                        g.silent = true;
                        g.device_name = "silent".into();
                        g.reason = Some("cpal open timed out".into());
                    }
                }
            }
        }

        Self { inner }
    }

    fn try_start_cpal(
        inner: Arc<Mutex<SharedPlay>>,
    ) -> Result<(cpal::Stream, String, u32), anyhow::Error> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| anyhow::anyhow!("no default output device"))?;
        let name = device
            .name()
            .unwrap_or_else(|_| "default output".to_string());
        let config = device.default_output_config()?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        {
            let mut g = inner.lock();
            g.output_sample_rate = sample_rate;
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
        Ok((stream, name, sample_rate))
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
        g.program_sample_rate = audio.sample_rate.max(1);
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
        let sr = g.program_sample_rate.max(1) as f64;
        g.cursor = (seconds * sr).max(0.0);
        if !g.samples.is_empty() {
            g.cursor = g.cursor.min((g.samples.len() - 1) as f64);
        }
    }

    pub fn cue(&self) {
        self.seek_seconds(0.0);
        self.pause();
    }

    pub fn is_playing(&self) -> bool {
        self.inner.lock().playing
    }

    pub fn is_silent(&self) -> bool {
        self.inner.lock().silent
    }

    pub fn cursor_seconds(&self) -> f64 {
        let g = self.inner.lock();
        g.cursor / g.program_sample_rate.max(1) as f64
    }

    pub fn program_sample_rate(&self) -> u32 {
        self.inner.lock().program_sample_rate
    }

    pub fn duration_secs(&self) -> Option<f64> {
        let g = self.inner.lock();
        if g.samples.is_empty() {
            None
        } else {
            Some(g.samples.len() as f64 / g.program_sample_rate.max(1) as f64)
        }
    }

    pub fn device_name(&self) -> String {
        self.inner.lock().device_name.clone()
    }

    pub fn silent_reason(&self) -> Option<String> {
        self.inner.lock().reason.clone()
    }

    /// Follow the show clock when there is no device callback advancing the cursor.
    pub fn follow_clock_seconds(&self, seconds: f64) {
        if self.is_silent() && self.is_playing() {
            self.seek_seconds(seconds);
        }
    }

    /// Soft-resync to the show clock when drift exceeds `max_drift_secs`.
    pub fn resync_if_drift(&self, clock_seconds: f64, max_drift_secs: f64) -> f64 {
        let cursor = self.cursor_seconds();
        let drift = cursor - clock_seconds;
        if drift.abs() > max_drift_secs {
            self.seek_seconds(clock_seconds);
            0.0
        } else {
            drift
        }
    }
}

fn route_sample(route: AudioChannelRoute, program: f32, ltc: f32, ltc_on: bool) -> f32 {
    match route {
        AudioChannelRoute::Silent => 0.0,
        AudioChannelRoute::Program => program,
        AudioChannelRoute::LtcGenerator => {
            if ltc_on {
                ltc
            } else {
                0.0
            }
        }
    }
}

fn fill_output(data: &mut [f32], channels: usize, inner: &Arc<Mutex<SharedPlay>>) {
    let mut g = inner.lock();
    let playing = g.playing;
    let routing = g.routing.clone();
    let ltc_on = routing.ltc_generator_enabled;
    let out_sr = g.output_sample_rate.max(1) as f64;
    let prog_sr = g.program_sample_rate.max(1) as f64;
    // Advance program cursor by the ratio of program rate to device rate.
    let step = prog_sr / out_sr;

    for frame in data.chunks_mut(channels) {
        let prog = if playing {
            let idx = g.cursor as usize;
            let sample = if idx < g.samples.len() {
                g.samples[idx]
            } else {
                0.0
            };
            g.cursor += step;
            sample
        } else {
            0.0
        };

        let ltc = if ltc_on && playing {
            let s = (g.ltc_phase * std::f64::consts::TAU).sin() as f32 * 0.2;
            g.ltc_phase = (g.ltc_phase + 1000.0 / out_sr) % 1.0;
            s
        } else {
            0.0
        };

        let l = route_sample(routing.left, prog, ltc, ltc_on);
        let r = route_sample(routing.right, prog, ltc, ltc_on);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_program_left_ltc_right_off() {
        let l = route_sample(AudioChannelRoute::Program, 0.5, 0.9, false);
        let r = route_sample(AudioChannelRoute::LtcGenerator, 0.5, 0.9, false);
        assert!((l - 0.5).abs() < f32::EPSILON);
        assert!((r - 0.0).abs() < f32::EPSILON);
    }

    #[test]
    fn routing_ltc_on_right() {
        let r = route_sample(AudioChannelRoute::LtcGenerator, 0.5, 0.9, true);
        assert!((r - 0.9).abs() < f32::EPSILON);
    }
}
