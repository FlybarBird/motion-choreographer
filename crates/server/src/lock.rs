//! Timecode inputs: LTC on an audio input, MIDI timecode, and Art-Net
//! timecode. Each one runs on its own thread, decodes what arrives and hands
//! over readings stamped with the time they were valid. The show polls them
//! from its tick and feeds the chase engine.
//!
//! A dropped file is the fourth source. The server rolls that one itself, so
//! its code is the audio clock and it needs no decoding and no chase; the
//! show handles it directly.

use ptz_core::codes::{artnet_parse, CodeRate, CodeReading, MtcDecoder};
use ptz_core::ltc::LtcDecoder;
use ptz_core::pose::InputConfig;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One timecode observation and the moment it was true.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Stamped {
    pub reading: CodeReading,
    pub at: Instant,
}

/// Anything that yields timecode readings.
pub trait CodeSource: Send {
    /// Readings that arrived since the last call, oldest first.
    fn poll(&mut self) -> Vec<Stamped>;
    /// Frames between readings while the code rolls: 1 for LTC and Art-Net, 2 for MTC.
    fn interval_frames(&self) -> f64;
    /// Why the input stopped delivering, when it has failed since it opened.
    fn problem(&self) -> Option<String>;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Ltc,
    Mtc,
    ArtNet,
}

impl Kind {
    pub fn parse(id: &str) -> Option<Kind> {
        match id {
            "ltc" => Some(Kind::Ltc),
            "mtc" => Some(Kind::Mtc),
            "artnet" => Some(Kind::ArtNet),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Kind::Ltc => "ltc",
            Kind::Mtc => "mtc",
            Kind::ArtNet => "artnet",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Ltc => "LTC input",
            Kind::Mtc => "MTC input",
            Kind::ArtNet => "Art-Net timecode",
        }
    }

    pub fn interval_frames(self) -> f64 {
        match self {
            Kind::Mtc => 2.0,
            Kind::Ltc | Kind::ArtNet => 1.0,
        }
    }
}

/// The rate family an LTC frame belongs to: the drop-frame flag, or the nearest
/// of 24, 25 and 30 to the frame length measured from the signal.
pub fn ltc_family(drop_frame: bool, fps: f64) -> CodeRate {
    if drop_frame {
        return CodeRate::F30Df;
    }
    let near = |target: f64| (fps / target).ln().abs();
    let mut best = (CodeRate::F24, near(24.0));
    for (rate, target) in [(CodeRate::F25, 25.0), (CodeRate::F30, 30.0)] {
        if near(target) < best.1 {
            best = (rate, near(target));
        }
    }
    best.0
}

/// Turns one channel of audio into stamped readings.
pub struct LtcReader {
    decoder: LtcDecoder,
    sample_rate: u32,
}

impl LtcReader {
    pub fn new(sample_rate: u32) -> LtcReader {
        LtcReader { decoder: LtcDecoder::new(), sample_rate: sample_rate.max(1) }
    }

    /// `samples` is the next stretch of the channel; `end` is when its last sample was captured.
    pub fn feed(&mut self, samples: &[f32], end: Instant) -> Vec<Stamped> {
        let frames = self.decoder.feed(samples);
        let seen = self.decoder.samples_seen();
        let rate = self.sample_rate as f64;
        frames
            .into_iter()
            .filter_map(|r| {
                let family = ltc_family(r.tc.drop_frame, rate / r.samples_per_frame.max(1.0));
                let counting = family.counting_rate();
                if !r.tc.is_valid(counting) {
                    return None;
                }
                // The frame's last bit ended `age` ago, and at that moment the code was one frame on.
                let age = seen.saturating_sub(r.end_sample) as f64 / rate;
                Some(Stamped {
                    reading: CodeReading { frames: r.tc.to_frames(counting) as f64 + 1.0, rate: family, locate: false },
                    at: end.checked_sub(Duration::from_secs_f64(age)).unwrap_or(end),
                })
            })
            .collect()
    }
}

/// Turns MIDI messages into stamped readings.
#[derive(Default)]
pub struct MtcReader {
    decoder: MtcDecoder,
}

impl MtcReader {
    pub fn new() -> MtcReader {
        MtcReader::default()
    }

    pub fn feed(&mut self, message: &[u8], at: Instant) -> Option<Stamped> {
        self.decoder.feed(message).map(|reading| Stamped { reading, at })
    }
}

enum Item {
    Reading(Stamped),
    Failed(String),
}

/// Stops the input's thread when the source is dropped.
struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// A source whose readings arrive from another thread.
pub struct Feed {
    interval: f64,
    rx: Receiver<Item>,
    problem: Option<String>,
    _stop: Option<StopOnDrop>,
}

/// The sending side of a `Feed`.
#[derive(Clone)]
pub struct Feeder {
    tx: Sender<Item>,
}

impl Feeder {
    /// False once the source has been dropped.
    pub fn send(&self, stamped: Stamped) -> bool {
        self.tx.send(Item::Reading(stamped)).is_ok()
    }

    pub fn fail(&self, why: impl Into<String>) {
        let _ = self.tx.send(Item::Failed(why.into()));
    }
}

impl Feed {
    /// A source fed by the caller. The tests script timecode through this, past
    /// the same readers the devices use.
    pub fn scripted(kind: Kind) -> (Feeder, Feed) {
        let (tx, rx) = channel();
        (Feeder { tx }, Feed { interval: kind.interval_frames(), rx, problem: None, _stop: None })
    }

    fn live(kind: Kind, stop: Arc<AtomicBool>) -> (Feeder, Feed) {
        let (feeder, mut feed) = Feed::scripted(kind);
        feed._stop = Some(StopOnDrop(stop));
        (feeder, feed)
    }
}

impl CodeSource for Feed {
    fn poll(&mut self) -> Vec<Stamped> {
        let mut out = Vec::new();
        while let Ok(item) = self.rx.try_recv() {
            match item {
                Item::Reading(s) => out.push(s),
                Item::Failed(why) => self.problem = Some(why),
            }
        }
        out
    }

    fn interval_frames(&self) -> f64 {
        self.interval
    }

    fn problem(&self) -> Option<String> {
        self.problem.clone()
    }
}

/// The audio inputs and MIDI ports on this machine, for the input settings.
pub fn choices() -> ptz_core::api::InputChoices {
    let mut out = ptz_core::api::InputChoices::default();
    match crate::audio::device::list() {
        Ok((_, inputs)) => out.audio_inputs = inputs,
        Err(why) => out.audio_problem = Some(why),
    }
    match midir::MidiInput::new("PTZ Programmer") {
        Ok(input) => out.midi_inputs = input.ports().iter().filter_map(|p| input.port_name(p).ok()).collect(),
        Err(e) => out.midi_problem = Some(format!("MIDI is not available on this machine: {e}")),
    }
    out
}

/// Open the input for `kind` as the project configures it. An input that
/// cannot be opened says why, in words the operator can act on.
pub fn open(kind: Kind, cfg: &InputConfig) -> Result<Feed, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let (feeder, feed) = Feed::live(kind, stop.clone());
    match kind {
        Kind::ArtNet => artnet::open(&cfg.artnet_bind, feeder, stop)?,
        Kind::Mtc => on_thread("mtc-in", stop, {
            let port = cfg.mtc_port.clone();
            move |stop, ready| mtc::run(&port, feeder, stop, ready)
        })?,
        Kind::Ltc => on_thread("ltc-in", stop, {
            let (device, channel) = (cfg.ltc_device.clone(), cfg.ltc_channel);
            move |stop, ready| ltc_in::run(&device, channel, feeder, stop, ready)
        })?,
    }
    Ok(feed)
}

/// Run a device on its own thread, since its handle may not move between
/// threads, and wait for it to say whether it opened.
fn on_thread(
    name: &str,
    stop: Arc<AtomicBool>,
    body: impl FnOnce(Arc<AtomicBool>, Sender<Result<(), String>>) + Send + 'static,
) -> Result<(), String> {
    let (ready_tx, ready_rx) = channel();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || body(stop, ready_tx))
        .map_err(|e| format!("The input thread could not start: {e}"))?;
    match ready_rx.recv_timeout(Duration::from_secs(8)) {
        Ok(result) => result,
        Err(_) => Err("The input did not open within 8 seconds".into()),
    }
}

fn hold_until(stop: &AtomicBool) {
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

mod artnet {
    use super::*;
    use socket2::{Domain, Protocol, Socket, Type};
    use std::net::{SocketAddr, UdpSocket};

    /// A UDP socket that shares its port, so other Art-Net software on this machine keeps working.
    fn bind(addr: SocketAddr) -> std::io::Result<UdpSocket> {
        let socket = Socket::new(Domain::for_address(addr), Type::DGRAM, Some(Protocol::UDP))?;
        socket.set_reuse_address(true)?;
        #[cfg(unix)]
        socket.set_reuse_port(true)?;
        socket.bind(&addr.into())?;
        socket.set_read_timeout(Some(Duration::from_millis(100)))?;
        Ok(socket.into())
    }

    pub fn open(bind_to: &str, feeder: Feeder, stop: Arc<AtomicBool>) -> Result<(), String> {
        let addr: SocketAddr = bind_to.parse().map_err(|_| {
            format!("The Art-Net address \"{bind_to}\" is not an address like 0.0.0.0:6454. Change it in the project's inputs")
        })?;
        let socket = bind(addr).map_err(|e| format!("Art-Net timecode could not listen on {addr}: {e}"))?;
        std::thread::Builder::new()
            .name("artnet-in".into())
            .spawn(move || {
                let mut packet = [0u8; 1024];
                while !stop.load(Ordering::Relaxed) {
                    match socket.recv_from(&mut packet) {
                        Ok((len, _)) => {
                            let at = Instant::now();
                            if let Some(reading) = artnet_parse(&packet[..len]) {
                                if !feeder.send(Stamped { reading, at }) {
                                    break;
                                }
                            }
                        }
                        Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                        Err(e) => {
                            feeder.fail(format!("Art-Net timecode stopped: {e}"));
                            break;
                        }
                    }
                }
            })
            .map_err(|e| format!("The Art-Net thread could not start: {e}"))?;
        Ok(())
    }
}

mod mtc {
    use super::*;
    use midir::MidiInput;

    pub fn run(want: &str, feeder: Feeder, stop: Arc<AtomicBool>, ready: Sender<Result<(), String>>) {
        let opened = (|| {
            let input = MidiInput::new("PTZ Programmer").map_err(|e| format!("MIDI is not available on this machine: {e}"))?;
            let ports = input.ports();
            let names: Vec<String> = ports.iter().map(|p| input.port_name(p).unwrap_or_default()).collect();
            let want_l = want.to_lowercase();
            let index = if want.is_empty() {
                (!ports.is_empty()).then_some(0)
            } else {
                names.iter().position(|n| n.to_lowercase().contains(&want_l))
            };
            let Some(index) = index else {
                return Err(if ports.is_empty() {
                    "No MIDI input was found. Connect the interface that carries MIDI timecode".to_string()
                } else {
                    format!("No MIDI input is named \"{want}\". Inputs here: {}", names.join(", "))
                });
            };
            let mut reader = MtcReader::new();
            let tx = feeder.clone();
            input
                .connect(
                    &ports[index],
                    "ptz-mtc-in",
                    move |_, message, _| {
                        if let Some(s) = reader.feed(message, Instant::now()) {
                            tx.send(s);
                        }
                    },
                    (),
                )
                .map_err(|e| format!("{} could not be opened: {e}", names[index]))
        })();
        match opened {
            Ok(connection) => {
                let _ = ready.send(Ok(()));
                hold_until(&stop);
                drop(connection);
            }
            Err(why) => {
                let _ = ready.send(Err(why));
            }
        }
    }
}

mod ltc_in {
    use super::*;
    use crate::audio::device::{device_name, host_problem};
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{FromSample, Sample, SampleFormat, SizedSample};

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        channel: usize,
        feeder: Feeder,
    ) -> Result<cpal::Stream, cpal::Error>
    where
        T: SizedSample,
        f32: FromSample<T>,
    {
        let channels = config.channels as usize;
        let rate = config.sample_rate;
        let mut reader = LtcReader::new(rate);
        let mut one: Vec<f32> = Vec::new();
        let failed = feeder.clone();
        device.build_input_stream(
            config,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                one.clear();
                one.extend(data.iter().skip(channel).step_by(channels.max(1)).map(|s| f32::from_sample(*s)));
                // The buffer's first sample was captured `lag` ago; its last one a buffer length later.
                let ts = info.timestamp();
                let lag = ts.callback.duration_since(ts.capture).as_secs_f64();
                let age = (lag - one.len() as f64 / rate as f64).clamp(0.0, 0.5);
                let now = Instant::now();
                let end = now.checked_sub(Duration::from_secs_f64(age)).unwrap_or(now);
                for s in reader.feed(&one, end) {
                    feeder.send(s);
                }
            },
            move |err: cpal::Error| {
                use cpal::ErrorKind::*;
                if !matches!(err.kind(), Xrun | RealtimeDenied) {
                    failed.fail(format!("The LTC input stopped: {err}"));
                }
            },
            None,
        )
    }

    fn open(want: &str, channel: u32, feeder: Feeder) -> Result<cpal::Stream, String> {
        if let Some(p) = host_problem() {
            return Err(format!("LTC needs an audio input. {p}"));
        }
        let host = cpal::default_host();
        let device = if want.is_empty() {
            host.default_input_device().ok_or_else(|| {
                "There is no audio input for LTC. Connect one, or allow microphone access for the server".to_string()
            })?
        } else {
            let want_l = want.to_lowercase();
            host.input_devices()
                .map_err(|e| format!("Could not list audio inputs: {e}"))?
                .find(|d| device_name(d).to_lowercase().contains(&want_l))
                .ok_or_else(|| format!("No audio input is named \"{want}\". See ptz-server --list-audio"))?
        };
        let name = device_name(&device);
        let supported = device.default_input_config().map_err(|e| format!("{name} could not be opened for LTC: {e}"))?;
        let channels = supported.channels() as u32;
        if channel >= channels {
            return Err(format!(
                "{name} has {channels} input channel{}, so LTC cannot be read from channel {}",
                if channels == 1 { "" } else { "s" },
                channel + 1
            ));
        }
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        let ch = channel as usize;
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, ch, feeder),
            SampleFormat::I16 => build::<i16>(&device, config, ch, feeder),
            SampleFormat::U16 => build::<u16>(&device, config, ch, feeder),
            SampleFormat::I32 => build::<i32>(&device, config, ch, feeder),
            SampleFormat::I8 => build::<i8>(&device, config, ch, feeder),
            SampleFormat::U8 => build::<u8>(&device, config, ch, feeder),
            SampleFormat::F64 => build::<f64>(&device, config, ch, feeder),
            other => return Err(format!("{name} uses the sample format {other}, which is not supported")),
        }
        .map_err(|e| format!("{name} could not be opened for LTC: {e}"))?;
        stream.play().map_err(|e| format!("{name} would not start: {e}"))?;
        Ok(stream)
    }

    pub fn run(want: &str, channel: u32, feeder: Feeder, stop: Arc<AtomicBool>, ready: Sender<Result<(), String>>) {
        match open(want, channel, feeder) {
            Ok(stream) => {
                let _ = ready.send(Ok(()));
                hold_until(&stop);
                drop(stream);
            }
            Err(why) => {
                let _ = ready.send(Err(why));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ptz_core::chase::{Chase, ChaseState};
    use ptz_core::codes::{artnet_build, mtc_full_frame, mtc_quarter_frames};
    use ptz_core::time::{FrameRate, Timecode};

    fn secs(from: Instant, at: Instant) -> f64 {
        at.duration_since(from).as_secs_f64()
    }

    #[test]
    fn ltc_audio_becomes_readings_stamped_where_each_frame_ended() {
        for (rate, family) in [
            (FrameRate::F24, CodeRate::F24),
            (FrameRate::F25, CodeRate::F25),
            (FrameRate::F2997Df, CodeRate::F30Df),
            (FrameRate::F2997, CodeRate::F30),
            (FrameRate::F30, CodeRate::F30),
        ] {
            let sr = 48_000u32;
            let first = Timecode::parse_at("01:00:10:00", rate).unwrap().to_frames(rate);
            let audio = ptz_core::ltc::encode(first, 50, rate, sr, 0.5, 1.0);
            let t0 = Instant::now();
            let mut reader = LtcReader::new(sr);
            let mut chase = Chase::new(rate, 8, 1.0);
            let mut fed = 0usize;
            let mut count = 0;
            // 10 ms buffers, each handed over at the moment its last sample would have been captured.
            for chunk in audio.chunks(480) {
                fed += chunk.len();
                let end = t0 + Duration::from_secs_f64(fed as f64 / sr as f64);
                for s in reader.feed(chunk, end) {
                    assert_eq!(s.reading.rate, family, "{}", rate.label());
                    // The code at any moment is the first frame plus the time since the audio began.
                    let want = first as f64 + secs(t0, s.at) * rate.fps();
                    assert!((s.reading.frames - want).abs() < 0.02, "{}: {} against {want}", rate.label(), s.reading.frames);
                    chase.on_reading(s.reading, secs(t0, s.at));
                    count += 1;
                }
            }
            assert!(count >= 48, "{}: {count} frames decoded", rate.label());
            assert_eq!(chase.state(), ChaseState::Locked, "{}", rate.label());
        }
    }

    #[test]
    fn the_rate_family_comes_from_the_drop_flag_or_the_measured_frame_length() {
        assert_eq!(ltc_family(true, 29.97), CodeRate::F30Df);
        assert_eq!(ltc_family(false, 29.97), CodeRate::F30);
        assert_eq!(ltc_family(false, 23.976), CodeRate::F24);
        assert_eq!(ltc_family(false, 24.4), CodeRate::F24);
        assert_eq!(ltc_family(false, 24.6), CodeRate::F25);
        assert_eq!(ltc_family(false, 25.0), CodeRate::F25);
    }

    #[test]
    fn mtc_messages_become_readings_and_a_full_frame_is_a_locate() {
        let rate = FrameRate::F25;
        let t0 = Instant::now();
        let mut reader = MtcReader::new();
        let tc = Timecode::parse_at("01:00:10:00", rate).unwrap();
        let mut got = None;
        for (i, piece) in mtc_quarter_frames(tc, CodeRate::F25).iter().enumerate() {
            got = reader.feed(piece, t0 + Duration::from_millis(10 * i as u64));
        }
        let s = got.expect("the eighth piece completes a timecode");
        assert_eq!(s.reading.frames, tc.to_frames(rate) as f64 + 1.75);
        assert_eq!(s.at, t0 + Duration::from_millis(70));
        let locate = reader.feed(&mtc_full_frame(tc, CodeRate::F25), t0).unwrap();
        assert!(locate.reading.locate);
        assert!(reader.feed(&[0x90, 60, 100], t0).is_none(), "a note is not timecode");
    }

    #[test]
    fn art_net_packets_arrive_through_a_shared_port() {
        let rate = FrameRate::F25;
        // Two listeners on one port: address reuse lets other Art-Net software run beside the server.
        let probe = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = probe.local_addr().unwrap();
        drop(probe);
        let cfg = InputConfig { artnet_bind: addr.to_string(), ..InputConfig::default() };
        let mut first = open(Kind::ArtNet, &cfg).expect("first listener");
        let mut second = open(Kind::ArtNet, &cfg).expect("a second listener shares the port");

        let tx = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        let tc = Timecode::parse_at("01:00:10:00", rate).unwrap();
        tx.send_to(&artnet_build(tc, CodeRate::F25), addr).unwrap();
        tx.send_to(b"not art-net at all", addr).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut got = Vec::new();
        // A packet sent to one machine goes to one of the listeners; a broadcast reaches them all.
        while got.is_empty() && Instant::now() < deadline {
            got.extend(first.poll());
            got.extend(second.poll());
            std::thread::sleep(Duration::from_millis(5));
        }
        std::thread::sleep(Duration::from_millis(50));
        got.extend(first.poll());
        got.extend(second.poll());
        assert_eq!(got.len(), 1, "one timecode packet, the stray one ignored");
        assert_eq!(got[0].reading, CodeReading { frames: tc.to_frames(rate) as f64, rate: CodeRate::F25, locate: false });
        assert!(first.problem().is_none());

        let bad = InputConfig { artnet_bind: "six thousand".into(), ..InputConfig::default() };
        assert!(open(Kind::ArtNet, &bad).err().unwrap().contains("not an address like 0.0.0.0:6454"));
    }
}
