//! The audio engine. One `Renderer` mixes the timeline's clips into the output
//! channels and counts the samples it has played; that count is the master
//! clock for free-run playback. The same renderer is driven by the device
//! callback, by a paced thread when there is no device, and by the tests.

use ptz_core::ltc::LtcFrame;
use ptz_core::pose::{ChannelRoute, Routing};
use ptz_core::time::{frame_to_sample, sample_to_frame_f, FrameRate, Timecode};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Sample rate used when no device is open.
pub const NULL_RATE: u32 = 48_000;
/// Fade applied on Play, Stop and Seek so they do not click.
pub const FADE_SECONDS: f64 = 0.010;

/// A stretch of mono audio at the device rate, placed on the song timeline.
#[derive(Clone)]
pub struct PlayClip {
    /// Song position of the first sample that plays, in device samples.
    pub start: i64,
    /// Index into `data` of that first sample.
    pub offset: i64,
    pub len: i64,
    pub data: Arc<Vec<f32>>,
}

/// LTC generator settings: which code the song's first frame carries.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LtcOut {
    pub rate: FrameRate,
    /// Code frame number at song frame 0.
    pub start_code: i64,
    pub level: f32,
}

#[derive(Clone, Default)]
pub struct Mix {
    pub clips: Vec<PlayClip>,
    pub routing: Routing,
    pub ltc: Option<LtcOut>,
}

enum Command {
    Play(i64),
    Stop,
    Seek(i64),
    Mix(Arc<Mix>),
}

#[derive(Clone, Copy)]
struct Report {
    pos: i64,
    playing: bool,
    at: Instant,
}

/// State shared between the engine and whatever drives the renderer.
pub struct Shared {
    commands: Mutex<Vec<(u64, Command)>>,
    report: Mutex<Report>,
    applied: AtomicU64,
    callbacks: AtomicU64,
    peak_left: AtomicU32,
    peak_right: AtomicU32,
}

impl Shared {
    fn new() -> Shared {
        Shared {
            commands: Mutex::new(Vec::new()),
            report: Mutex::new(Report { pos: 0, playing: false, at: Instant::now() }),
            applied: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            peak_left: AtomicU32::new(0),
            peak_right: AtomicU32::new(0),
        }
    }
}

struct Tail {
    pos: i64,
    gain: f32,
}

pub struct Renderer {
    shared: Arc<Shared>,
    rate: u32,
    mix: Arc<Mix>,
    playing: bool,
    pos: i64,
    /// Where the current play started; the reported position never goes below it.
    origin: i64,
    gain: f32,
    tail: Option<Tail>,
    fade_step: f32,
    peak: [f32; 2],
    ltc_frame: Option<(i64, LtcFrame)>,
}

impl Renderer {
    fn new(shared: Arc<Shared>, rate: u32) -> Renderer {
        Renderer {
            shared,
            rate,
            mix: Arc::new(Mix::default()),
            playing: false,
            pos: 0,
            origin: 0,
            gain: 0.0,
            tail: None,
            fade_step: (1.0 / (FADE_SECONDS * rate as f64)) as f32,
            peak: [0.0; 2],
            ltc_frame: None,
        }
    }

    fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Play(p) => {
                if self.playing {
                    self.tail = Some(Tail { pos: self.pos, gain: self.gain });
                }
                self.pos = p;
                self.origin = p;
                self.playing = true;
                self.gain = 0.0;
            }
            Command::Stop => {
                if self.playing {
                    self.tail = Some(Tail { pos: self.pos, gain: self.gain });
                    self.playing = false;
                }
            }
            Command::Seek(p) => {
                if self.playing {
                    self.tail = Some(Tail { pos: self.pos, gain: self.gain });
                    self.gain = 0.0;
                }
                self.pos = p;
                self.origin = p;
            }
            Command::Mix(m) => self.mix = m,
        }
    }

    fn program_at(mix: &Mix, pos: i64) -> f32 {
        let mut v = 0.0;
        for c in &mix.clips {
            let i = pos - c.start;
            if i >= 0 && i < c.len {
                if let Some(s) = c.data.get((c.offset + i) as usize) {
                    v += *s;
                }
            }
        }
        v
    }

    fn ltc_at(&mut self, cfg: LtcOut, pos: i64) -> f32 {
        let code = cfg.start_code as f64 + sample_to_frame_f(pos, self.rate, cfg.rate);
        let n = code.floor() as i64;
        if self.ltc_frame.as_ref().map(|(k, _)| *k) != Some(n) {
            let day = cfg.rate.frames_per_day();
            self.ltc_frame = Some((n, LtcFrame::new(Timecode::from_frames(n.rem_euclid(day), cfg.rate), cfg.rate)));
        }
        let (_, frame) = self.ltc_frame.as_ref().expect("set above");
        frame.level_at(code - n as f64) * cfg.level
    }

    /// Fill `out` (interleaved, `channels` wide). `latency` is how many samples
    /// sit between this buffer and the loudspeaker, so the reported position is
    /// what is being heard.
    pub fn render(&mut self, out: &mut [f32], channels: usize, latency: i64) {
        let pending = match self.shared.commands.try_lock() {
            Ok(mut q) if !q.is_empty() => std::mem::take(&mut *q),
            _ => Vec::new(),
        };
        for (gen, cmd) in pending {
            self.apply(cmd);
            self.shared.applied.store(gen, Ordering::Release);
        }
        let mix = self.mix.clone();
        let channels = channels.max(1);
        let mut peak = [0.0f32; 2];
        for frame in out.chunks_mut(channels) {
            let mut program = 0.0;
            if self.playing {
                program += Self::program_at(&mix, self.pos) * self.gain;
                self.gain = (self.gain + self.fade_step).min(1.0);
            }
            if let Some(t) = &mut self.tail {
                program += Self::program_at(&mix, t.pos) * t.gain;
                t.pos += 1;
                t.gain -= self.fade_step;
                if t.gain <= 0.0 {
                    self.tail = None;
                }
            }
            let ltc = match (self.playing, mix.ltc) {
                (true, Some(cfg)) if mix.routing.left == ChannelRoute::Ltc || mix.routing.right == ChannelRoute::Ltc => {
                    self.ltc_at(cfg, self.pos)
                }
                _ => 0.0,
            };
            let route = |r: ChannelRoute| match r {
                ChannelRoute::Program => program,
                ChannelRoute::Ltc => ltc,
                ChannelRoute::Silent => 0.0,
            };
            let (l, r) = (route(mix.routing.left), route(mix.routing.right));
            for (i, s) in frame.iter_mut().enumerate() {
                *s = match (channels, i) {
                    // A mono device gets the left choice.
                    (1, _) | (_, 0) => l,
                    (_, 1) => r,
                    _ => 0.0,
                };
            }
            peak[0] = peak[0].max(l.abs());
            peak[1] = peak[1].max(r.abs());
            if self.playing {
                self.pos += 1;
            }
        }

        // Meters fall by half every 150 ms.
        let frames = (out.len() / channels) as f32;
        let decay = 0.5f32.powf(frames / (0.15 * self.rate as f32));
        for (held, new) in self.peak.iter_mut().zip(peak) {
            *held = (*held * decay).max(new);
        }
        self.shared.peak_left.store(self.peak[0].to_bits(), Ordering::Relaxed);
        self.shared.peak_right.store(self.peak[1].to_bits(), Ordering::Relaxed);
        self.shared.callbacks.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut r) = self.shared.report.try_lock() {
            let heard = if self.playing { (self.pos - latency).max(self.origin) } else { self.pos };
            *r = Report { pos: heard, playing: self.playing, at: Instant::now() };
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct DeviceInfo {
    /// Output device name, or `None` when running silent.
    pub name: Option<String>,
    pub silent_reason: Option<String>,
}

/// The engine's handle on the audio thread.
pub struct AudioEngine {
    shared: Arc<Shared>,
    rate: u32,
    info: Arc<Mutex<DeviceInfo>>,
    stop: Arc<AtomicBool>,
    next_gen: u64,
    /// Generation of the last Play, Stop or Seek. The renderer's report is stale until it has applied it.
    transport_gen: u64,
    want_playing: bool,
    want_pos: i64,
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl AudioEngine {
    /// Open the output device on its own thread. Falls back to a silent, paced
    /// renderer when there is no device, and says why in `info()`.
    pub fn start(choice: crate::engine::AudioChoice) -> AudioEngine {
        let shared = Arc::new(Shared::new());
        let info = Arc::new(Mutex::new(DeviceInfo::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel::<u32>();
        {
            let (shared, info, stop) = (shared.clone(), info.clone(), stop.clone());
            std::thread::Builder::new()
                .name("audio".into())
                .spawn(move || device::run(shared, info, stop, choice, tx))
                .expect("audio thread");
        }
        let rate = rx.recv_timeout(Duration::from_secs(8)).unwrap_or(NULL_RATE);
        AudioEngine { shared, rate, info, stop, next_gen: 0, transport_gen: 0, want_playing: false, want_pos: 0 }
    }

    /// An engine with nothing driving it. The caller renders by hand.
    pub fn offline(rate: u32) -> (AudioEngine, Renderer) {
        let shared = Arc::new(Shared::new());
        let renderer = Renderer::new(shared.clone(), rate);
        let info = DeviceInfo { name: None, silent_reason: Some("Offline renderer".into()) };
        let engine = AudioEngine {
            shared,
            rate,
            info: Arc::new(Mutex::new(info)),
            stop: Arc::new(AtomicBool::new(false)),
            next_gen: 0,
            transport_gen: 0,
            want_playing: false,
            want_pos: 0,
        };
        (engine, renderer)
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    pub fn info(&self) -> DeviceInfo {
        self.info.lock().map(|i| i.clone()).unwrap_or_default()
    }

    pub fn callbacks(&self) -> u64 {
        self.shared.callbacks.load(Ordering::Relaxed)
    }

    pub fn peaks(&self) -> (f32, f32) {
        (
            f32::from_bits(self.shared.peak_left.load(Ordering::Relaxed)),
            f32::from_bits(self.shared.peak_right.load(Ordering::Relaxed)),
        )
    }

    fn push(&mut self, cmd: Command) {
        self.next_gen += 1;
        if !matches!(cmd, Command::Mix(_)) {
            self.transport_gen = self.next_gen;
        }
        if let Ok(mut q) = self.shared.commands.lock() {
            q.push((self.next_gen, cmd));
        }
    }

    pub fn set_mix(&mut self, mix: Mix) {
        self.push(Command::Mix(Arc::new(mix)));
    }

    fn sample_of(&self, frame: f64, fps: FrameRate) -> i64 {
        let whole = frame.floor();
        let base = frame_to_sample(whole as i64, self.rate, fps);
        let next = frame_to_sample(whole as i64 + 1, self.rate, fps);
        base + ((next - base) as f64 * (frame - whole)).round() as i64
    }

    pub fn play(&mut self, frame: f64, fps: FrameRate) {
        let p = self.sample_of(frame, fps);
        self.want_playing = true;
        self.want_pos = p;
        self.push(Command::Play(p));
    }

    pub fn stop(&mut self) {
        if self.want_playing {
            self.want_pos = self.position_samples();
            self.want_playing = false;
            self.push(Command::Stop);
        }
    }

    pub fn seek(&mut self, frame: f64, fps: FrameRate) {
        let p = self.sample_of(frame, fps);
        self.want_pos = p;
        self.push(Command::Seek(p));
    }

    pub fn is_playing(&self) -> bool {
        self.want_playing
    }

    fn position_samples(&self) -> i64 {
        if !self.want_playing || self.shared.applied.load(Ordering::Acquire) < self.transport_gen {
            return self.want_pos;
        }
        let Ok(r) = self.shared.report.lock().map(|r| *r) else { return self.want_pos };
        if !r.playing {
            return self.want_pos;
        }
        // Between callbacks the position keeps moving at the device rate.
        let ahead = r.at.elapsed().as_secs_f64().min(0.25);
        r.pos + (ahead * self.rate as f64) as i64
    }

    /// Position in song frames. This is the master clock while playing.
    pub fn position(&self, fps: FrameRate) -> f64 {
        sample_to_frame_f(self.position_samples(), self.rate, fps)
    }

    /// Where the renderer itself says it is, without extrapolation.
    pub fn rendered_position(&self, fps: FrameRate) -> f64 {
        let pos = self.shared.report.lock().map(|r| r.pos).unwrap_or(self.want_pos);
        sample_to_frame_f(pos, self.rate, fps)
    }
}

/// Resample mono audio. Done once when a clip is loaded, never in the callback.
pub fn resample(mono: &[f32], from: u32, to: u32) -> Result<Vec<f32>, String> {
    use audioadapter_buffers::direct::InterleavedSlice;
    use rubato::{Fft, FixedSync, Resampler};
    if from == to || mono.is_empty() {
        return Ok(mono.to_vec());
    }
    let mut r = Fft::<f32>::new(from as usize, to as usize, 1024, 1, FixedSync::Input).map_err(|e| format!("resampler: {e}"))?;
    let input = InterleavedSlice::new(mono, 1, mono.len()).map_err(|e| format!("resampler: {e}"))?;
    let out = r.process_all(&input, mono.len(), None).map_err(|e| format!("resampler: {e}"))?;
    let mut data = out.take_data();
    let want = (mono.len() as u64 * to as u64).div_ceil(from as u64) as usize;
    data.resize(want, 0.0);
    Ok(data)
}

/// Play one second of tone on the left, then one on the right, through the
/// same path a show uses. Returns the report to print, or why nothing played.
pub fn selftest(choice: crate::engine::AudioChoice) -> Result<String, String> {
    let mut engine = AudioEngine::start(choice);
    let info = engine.info();
    let Some(name) = info.name else {
        return Err(info.silent_reason.unwrap_or_else(|| "No audio output".into()));
    };
    let rate = engine.rate();
    let tone: Arc<Vec<f32>> =
        Arc::new((0..rate).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 440.0 / rate as f32).sin() * 0.3).collect());
    let fps = FrameRate::F25;
    let mut report = format!("Output device: {name} at {rate} Hz\n");
    for (label, routing) in [
        ("Left", Routing { left: ChannelRoute::Program, right: ChannelRoute::Silent }),
        ("Right", Routing { left: ChannelRoute::Silent, right: ChannelRoute::Program }),
    ] {
        let before = engine.callbacks();
        engine.set_mix(Mix {
            clips: vec![PlayClip { start: 0, offset: 0, len: tone.len() as i64, data: tone.clone() }],
            routing,
            ltc: None,
        });
        engine.play(0.0, fps);
        std::thread::sleep(Duration::from_millis(1100));
        let position = engine.position(fps);
        engine.stop();
        std::thread::sleep(Duration::from_millis(150));
        let callbacks = engine.callbacks() - before;
        report.push_str(&format!(
            "{label}: 1 s of 440 Hz, {callbacks} callbacks, clock advanced {:.2} s\n",
            fps.frames_to_seconds(position)
        ));
        if callbacks == 0 {
            return Err(format!("{report}{name} opened but never asked for audio"));
        }
    }
    report.push_str(&format!("Callbacks in total: {}\n", engine.callbacks()));
    Ok(report)
}

pub mod device {
    //! The thread that owns the output stream. The stream type is not `Send`,
    //! so it is created, held and dropped here.

    use super::*;
    use crate::engine::AudioChoice;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, SampleFormat, SizedSample};
    use std::sync::mpsc::Sender;

    /// Why the default host cannot be used, when that can be known without opening it.
    #[cfg(target_os = "linux")]
    pub(crate) fn host_problem() -> Option<String> {
        match std::fs::read_to_string("/proc/asound/cards") {
            Ok(cards) if cards.contains("no soundcards") || cards.trim().is_empty() => {
                Some("This machine has no sound card (ALSA lists none)".into())
            }
            Ok(_) => None,
            Err(_) => Some("This machine has no sound card (ALSA is not loaded)".into()),
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn host_problem() -> Option<String> {
        None
    }

    pub fn device_name(device: &cpal::Device) -> String {
        device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "Unnamed device".into())
    }

    /// Output and input device names, for `--list-audio`.
    pub fn list() -> Result<(Vec<String>, Vec<String>), String> {
        if let Some(p) = host_problem() {
            return Err(p);
        }
        let host = cpal::default_host();
        let outs = host.output_devices().map_err(|e| e.to_string())?.map(|d| device_name(&d)).collect();
        let ins = host.input_devices().map_err(|e| e.to_string())?.map(|d| device_name(&d)).collect();
        Ok((outs, ins))
    }

    fn pick(choice: &AudioChoice) -> Result<cpal::Device, String> {
        if *choice == AudioChoice::Off {
            return Err("Audio output is turned off (--no-audio)".into());
        }
        if let Some(p) = host_problem() {
            return Err(p);
        }
        let host = cpal::default_host();
        match choice {
            AudioChoice::Named(want) => {
                let want_l = want.to_lowercase();
                let mut devices = host.output_devices().map_err(|e| format!("Could not list audio devices: {e}"))?;
                devices
                    .find(|d| device_name(d).to_lowercase().contains(&want_l))
                    .ok_or_else(|| format!("No output device is named \"{want}\". See ptz-server --list-audio"))
            }
            _ => host.default_output_device().ok_or_else(|| "There is no default audio output device".to_string()),
        }
    }

    struct Open {
        stream: cpal::Stream,
        name: String,
    }

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        renderer: Arc<Mutex<Renderer>>,
        failed: Arc<Mutex<Option<String>>>,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample + FromSample<f32>,
    {
        let channels = config.channels as usize;
        let rate = config.sample_rate as f64;
        let mut scratch: Vec<f32> = Vec::new();
        device.build_output_stream(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                scratch.resize(data.len(), 0.0);
                let ts = info.timestamp();
                let latency = (ts.playback.duration_since(ts.callback).as_secs_f64() * rate) as i64;
                match renderer.lock() {
                    Ok(mut r) => r.render(&mut scratch, channels, latency.clamp(0, rate as i64)),
                    Err(_) => scratch.fill(0.0),
                }
                for (o, s) in data.iter_mut().zip(&scratch) {
                    *o = T::from_sample(*s);
                }
            },
            move |err: cpal::Error| {
                use cpal::ErrorKind::*;
                // Glitches are not failures; anything else ends the stream.
                if !matches!(err.kind(), Xrun | RealtimeDenied) {
                    if let Ok(mut f) = failed.lock() {
                        *f = Some(err.to_string());
                    }
                }
            },
            None,
        )
    }

    fn open(
        choice: &AudioChoice,
        want_rate: Option<u32>,
        renderer: &Arc<Mutex<Renderer>>,
        failed: &Arc<Mutex<Option<String>>>,
        make_renderer: &mut dyn FnMut(u32),
    ) -> Result<Open, String> {
        let device = pick(choice)?;
        let name = device_name(&device);
        let supported = device.default_output_config().map_err(|e| format!("{name} could not be opened: {e}"))?;
        let rate = supported.sample_rate();
        if let Some(want) = want_rate {
            if want != rate {
                return Err(format!(
                    "{name} runs at {rate} Hz but the server started at {want} Hz. Restart the server to use it"
                ));
            }
        }
        make_renderer(rate);
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        let (r, f) = (renderer.clone(), failed.clone());
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, r, f),
            SampleFormat::I16 => build::<i16>(&device, config, r, f),
            SampleFormat::U16 => build::<u16>(&device, config, r, f),
            SampleFormat::I32 => build::<i32>(&device, config, r, f),
            SampleFormat::I8 => build::<i8>(&device, config, r, f),
            SampleFormat::U8 => build::<u8>(&device, config, r, f),
            SampleFormat::F64 => build::<f64>(&device, config, r, f),
            other => return Err(format!("{name} uses the sample format {other}, which is not supported")),
        }
        .map_err(|e| format!("{name} could not be opened: {e}"))?;
        stream.play().map_err(|e| format!("{name} would not start: {e}"))?;
        Ok(Open { stream, name })
    }

    pub fn run(
        shared: Arc<Shared>,
        info: Arc<Mutex<DeviceInfo>>,
        stop: Arc<AtomicBool>,
        choice: AudioChoice,
        ready: Sender<u32>,
    ) {
        let failed: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        // The renderer is created once the rate is known and then never replaced.
        let renderer = Arc::new(Mutex::new(Renderer::new(shared.clone(), NULL_RATE)));
        let made: std::cell::Cell<Option<u32>> = std::cell::Cell::new(None);
        let mut make = |rate: u32| {
            if made.get().is_none() {
                made.set(Some(rate));
                if let Ok(mut r) = renderer.lock() {
                    *r = Renderer::new(shared.clone(), rate);
                }
            }
        };
        let set_info = |name: Option<String>, reason: Option<String>| {
            if let Ok(mut i) = info.lock() {
                *i = DeviceInfo { name, silent_reason: reason };
            }
        };

        let mut current = match open(&choice, None, &renderer, &failed, &mut make) {
            Ok(o) => {
                set_info(Some(o.name.clone()), None);
                Some(o)
            }
            Err(reason) => {
                make(NULL_RATE);
                set_info(None, Some(format!("{reason}. Running silent")));
                None
            }
        };
        let rate = made.get().unwrap_or(NULL_RATE);
        let _ = ready.send(rate);

        let started = Instant::now();
        let mut silent_rendered: u64 = (started.elapsed().as_secs_f64() * rate as f64) as u64;
        let mut silent_buf = vec![0.0f32; 2 * 256];
        let mut last_retry = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            match &current {
                Some(open) => {
                    let _keep_alive = &open.stream;
                    std::thread::sleep(Duration::from_millis(50));
                    let failure = failed.lock().ok().and_then(|mut f| f.take());
                    if let Some(err) = failure {
                        let name = open.name.clone();
                        current = None;
                        set_info(None, Some(format!("{name} stopped: {err}. Running silent")));
                        silent_rendered = (started.elapsed().as_secs_f64() * rate as f64) as u64;
                        last_retry = Instant::now();
                    }
                }
                None => {
                    // No device: render at the pace of the monotonic clock so the show still runs.
                    let due = (started.elapsed().as_secs_f64() * rate as f64) as u64;
                    if due.saturating_sub(silent_rendered) > rate as u64 {
                        silent_rendered = due - 256;
                    }
                    while silent_rendered + 256 <= due {
                        silent_rendered += 256;
                        // The block is rendered when this thread wakes, a little after it fell due.
                        // Reporting how far past due it is keeps the position on the clock, not on the wake-ups.
                        let late = (started.elapsed().as_secs_f64() * rate as f64) as i64 - silent_rendered as i64;
                        if let Ok(mut r) = renderer.lock() {
                            r.render(&mut silent_buf, 2, -late.max(0));
                        }
                    }
                    std::thread::sleep(Duration::from_millis(2));
                    if choice != AudioChoice::Off && last_retry.elapsed() > Duration::from_secs(3) && host_problem().is_none() {
                        last_retry = Instant::now();
                        if let Ok(o) = open(&choice, Some(rate), &renderer, &failed, &mut make) {
                            set_info(Some(o.name.clone()), None);
                            current = Some(o);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn clip(start: i64, offset: i64, len: i64, data: Vec<f32>) -> PlayClip {
        PlayClip { start, offset, len, data: Arc::new(data) }
    }

    fn render(r: &mut Renderer, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames * 2];
        r.render(&mut out, 2, 0);
        out
    }

    fn left(out: &[f32]) -> Vec<f32> {
        out.chunks(2).map(|f| f[0]).collect()
    }

    fn right(out: &[f32]) -> Vec<f32> {
        out.chunks(2).map(|f| f[1]).collect()
    }

    #[test]
    fn program_left_silent_right_puts_signal_only_on_the_left() {
        let (mut e, mut r) = AudioEngine::offline(RATE);
        let data: Vec<f32> = (0..RATE).map(|i| ((i as f32) * 0.05).sin() * 0.5).collect();
        e.set_mix(Mix { clips: vec![clip(0, 0, RATE as i64, data)], routing: Routing::default(), ltc: None });
        e.play(0.0, FrameRate::F25);
        let out = render(&mut r, 4800);
        assert!(left(&out).iter().any(|s| s.abs() > 0.3));
        assert!(right(&out).iter().all(|s| *s == 0.0));

        // Swap the routing: now only the right.
        let data: Vec<f32> = vec![0.5; RATE as usize];
        e.set_mix(Mix {
            clips: vec![clip(0, 0, RATE as i64, data)],
            routing: Routing { left: ChannelRoute::Silent, right: ChannelRoute::Program },
            ltc: None,
        });
        let out = render(&mut r, 4800);
        assert!(left(&out).iter().all(|s| *s == 0.0));
        assert!(right(&out).iter().any(|s| *s > 0.4));
        let (pl, pr) = e.peaks();
        assert!(pl < 0.4 && pr > 0.4, "meters follow the routing: {pl} {pr}");
    }

    #[test]
    fn clip_offset_and_trim_land_on_the_exact_sample() {
        let (mut e, mut r) = AudioEngine::offline(RATE);
        // The source is a ramp, so every output sample names the source sample it came from.
        let data: Vec<f32> = (0..10_000).map(|i| i as f32).collect();
        // Play source samples 2000..5000 starting at song sample 1500.
        e.set_mix(Mix { clips: vec![clip(1500, 2000, 3000, data)], routing: Routing::default(), ltc: None });
        e.play(0.0, FrameRate::F25);
        let out = left(&render(&mut r, 6000));
        // Skip the 10 ms fade-in (480 samples); the clip starts after it.
        assert_eq!(out[1499], 0.0);
        assert_eq!(out[1500], 2000.0);
        assert_eq!(out[1501], 2001.0);
        assert_eq!(out[4499], 4999.0);
        assert_eq!(out[4500], 0.0, "the trimmed tail is not played");
    }

    #[test]
    fn play_stop_and_seek_fade_instead_of_clicking() {
        let (mut e, mut r) = AudioEngine::offline(RATE);
        e.set_mix(Mix {
            clips: vec![clip(0, 0, 10 * RATE as i64, vec![1.0; 10 * RATE as usize])],
            routing: Routing::default(),
            ltc: None,
        });
        let step = 1.0 / (FADE_SECONDS as f32 * RATE as f32);
        let max_jump = |s: &[f32]| s.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);

        e.play(0.0, FrameRate::F25);
        let a = left(&render(&mut r, 2000));
        assert_eq!(a[0], 0.0);
        assert!((a[1999] - 1.0).abs() < 1e-6);
        assert!(max_jump(&a) <= step * 1.01, "fade in");

        // Seek while playing: the old position fades out as the new one fades in.
        e.seek(100.0, FrameRate::F25);
        let b = left(&render(&mut r, 2000));
        let joined: Vec<f32> = a.iter().chain(&b).copied().collect();
        assert!(max_jump(&joined) <= step * 1.01, "seek");
        assert!(b.iter().all(|s| *s > 0.98), "a crossfade between equal levels stays level");

        e.stop();
        let c = left(&render(&mut r, 2000));
        let joined: Vec<f32> = b.iter().chain(&c).copied().collect();
        assert!(max_jump(&joined) <= step * 1.01, "stop");
        assert_eq!(c[1999], 0.0);
        assert_eq!(c[480 + 5], 0.0, "silent 10 ms after Stop");
    }

    #[test]
    fn played_samples_are_the_clock_at_every_rate() {
        for fps in FrameRate::ALL {
            let (mut e, mut r) = AudioEngine::offline(RATE);
            e.play(0.0, fps);
            // Ten seconds of audio in device-sized buffers.
            for _ in 0..(10 * RATE as usize / 480) {
                render(&mut r, 480);
            }
            let expect = 10.0 * fps.num() as f64 / fps.den() as f64;
            assert!((e.rendered_position(fps) - expect).abs() < 1e-9, "{fps:?}");

            // Starting from a frame lands on that frame's first sample.
            e.play(1234.0, fps);
            render(&mut r, 1);
            let back = e.rendered_position(fps);
            assert!(back >= 1234.0 && back - 1234.0 < 2.0 * fps.fps() / RATE as f64, "{fps:?}: {back}");

            // Stopped, the engine holds the position it stopped at.
            e.stop();
            let held = e.position(fps);
            render(&mut r, 4800);
            assert_eq!(e.position(fps), held);
            assert!(!e.is_playing());
        }
    }

    #[test]
    fn reported_position_allows_for_output_latency() {
        let (mut e, mut r) = AudioEngine::offline(RATE);
        e.play(0.0, FrameRate::F25);
        let mut out = vec![0.0; 960 * 2];
        r.render(&mut out, 2, 480);
        assert!((e.rendered_position(FrameRate::F25) - 0.25).abs() < 1e-9, "960 rendered, 480 not yet heard");
        // It never reports a position before the play point.
        e.play(100.0, FrameRate::F25);
        let mut out = vec![0.0; 100 * 2];
        r.render(&mut out, 2, 480);
        assert_eq!(e.rendered_position(FrameRate::F25), 100.0);
    }

    #[test]
    fn ltc_generator_output_decodes_back_to_the_song_timecode() {
        let (mut e, mut r) = AudioEngine::offline(RATE);
        let fps = FrameRate::F25;
        let start = Timecode::parse("01:00:00:00").unwrap().to_frames(fps);
        e.set_mix(Mix {
            clips: Vec::new(),
            routing: Routing { left: ChannelRoute::Program, right: ChannelRoute::Ltc },
            ltc: Some(LtcOut { rate: fps, start_code: start, level: 0.5 }),
        });
        e.play(50.0, fps);
        let out = right(&render(&mut r, RATE as usize * 2));
        let mut dec = ptz_core::ltc::LtcDecoder::new();
        let readings = dec.feed(&out);
        assert!(readings.len() > 40, "{} frames decoded", readings.len());
        let first = &readings[0];
        let code = first.tc.to_frames(fps);
        assert!((start + 50..start + 54).contains(&code), "first decoded code {}", first.tc);
        let last = readings.last().unwrap().tc.to_frames(fps);
        assert!((last - (start + 99)).abs() <= 1, "two seconds later the code is 50 frames on: {last}");
    }

    #[test]
    fn resampling_keeps_length_and_pitch() {
        let from = 44_100;
        let tone: Vec<f32> = (0..from).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 1000.0 / from as f32).sin()).collect();
        let out = resample(&tone, from, RATE).unwrap();
        assert_eq!(out.len(), RATE as usize);
        // Count zero crossings in the middle: a 1 kHz tone crosses 2000 times a second.
        let mid = &out[4800..43200];
        let crossings = mid.windows(2).filter(|w| (w[0] <= 0.0) != (w[1] <= 0.0)).count();
        assert!((crossings as i64 - 1600).abs() <= 2, "{crossings}");
        let peak = mid.iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!((peak - 1.0).abs() < 0.05, "{peak}");
        assert_eq!(resample(&tone, RATE, RATE).unwrap().len(), tone.len());
    }
}
