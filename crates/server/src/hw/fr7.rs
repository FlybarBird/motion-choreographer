//! Sony FR7: pan, tilt, zoom and focus over VISCA/IP, presets over CGI.
//!
//! `Link` is one conversation with the camera, used directly by the bring-up
//! tool. `Fr7` runs a `Link` on its own thread twenty times a second so the show
//! never waits on the network.

use super::cgi::{self, Cgi};
use super::visca::{self, Packet, Reply, Turn};
use super::{with_port, Loop, PERIOD};
use crate::rig::{DevicePreset, Drive, Ride};
use ptz_core::api::AdapterStatus;
use ptz_core::pose::{AxisTable, Pose, Preset, RigConfig};
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Pan, tilt, zoom and focus as the camera reports them, in pose units.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Head {
    pub pan: f64,
    pub tilt: f64,
    pub zoom: f64,
    pub focus: f64,
}

/// When each part of a `Head` was read.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ReadTimes {
    pub pan_tilt: Instant,
    pub zoom: Instant,
    pub focus: Instant,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Fr7Config {
    /// `host:port` of the VISCA service.
    pub visca: String,
    /// `host:port` of the web interface.
    pub http: String,
    pub user: String,
    pub password: String,
    pub counts_per_degree: f64,
    pub pan_sign: f64,
    pub tilt_sign: f64,
    pub absolute_moves: bool,
    /// Degrees per second of each speed step; empty uses `visca::speed_table`.
    pub speed_steps: Vec<f64>,
}

impl Fr7Config {
    pub fn of(rig: &RigConfig) -> Fr7Config {
        let host = rig.fr7_host.trim();
        let name = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
        Fr7Config {
            visca: with_port(host, visca::PORT),
            http: format!("{name}:{}", rig.fr7_http_port),
            user: rig.fr7_user.clone(),
            password: rig.fr7_password.clone(),
            counts_per_degree: rig.fr7_counts_per_degree.max(1.0),
            pan_sign: if rig.fr7_invert_pan { -1.0 } else { 1.0 },
            tilt_sign: if rig.fr7_invert_tilt { -1.0 } else { 1.0 },
            absolute_moves: rig.fr7_absolute_moves,
            speed_steps: rig.fr7_speed_steps.clone(),
        }
    }
}

/// One VISCA/IP conversation: sequence numbers, timeouts and retries.
pub struct Link {
    socket: UdpSocket,
    seq: u32,
    pub cfg: Fr7Config,
    pub timeout: Duration,
    pub retries: u32,
    /// Round trip of the last exchange.
    pub last_round_trip: Duration,
    table: Vec<f64>,
    /// The first message after a reset carries sequence number zero, not one.
    /// Sony's list leaves this open; the link settles on whichever the camera takes.
    zero_first: bool,
}

impl Link {
    pub fn connect(cfg: &Fr7Config) -> Result<Link, String> {
        let socket = UdpSocket::bind("0.0.0.0:0").map_err(|e| format!("could not open a UDP socket: {e}"))?;
        socket.connect(&cfg.visca).map_err(|e| format!("{} is not an address the FR7 can be reached at: {e}", cfg.visca))?;
        let mut link = Link {
            socket,
            seq: 0,
            cfg: cfg.clone(),
            timeout: Duration::from_millis(120),
            retries: 2,
            last_round_trip: Duration::ZERO,
            table: visca::speed_table(),
            zero_first: false,
        };
        link.set_speed_table(cfg.speed_steps.clone());
        link.reset()?;
        Ok(link)
    }

    pub fn speed_table(&self) -> &[f64] {
        &self.table
    }

    pub fn set_speed_table(&mut self, table: Vec<f64>) {
        if !table.is_empty() {
            self.table = table;
        }
    }

    fn receive(&self, deadline: Instant) -> Option<Packet> {
        let mut buf = [0u8; 64];
        loop {
            let left = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero())?;
            self.socket.set_read_timeout(Some(left)).ok()?;
            match self.socket.recv(&mut buf) {
                Ok(n) => {
                    if let Some(p) = Packet::parse(&buf[..n]) {
                        return Some(p);
                    }
                }
                Err(_) => return None,
            }
        }
    }

    /// Set the camera's sequence counter, and ours, back to zero.
    pub fn reset(&mut self) -> Result<(), String> {
        let packet = Packet::new(visca::TYPE_CONTROL, 0, &visca::RESET).to_bytes();
        for _ in 0..=self.retries {
            self.socket.send(&packet).map_err(|e| format!("could not send to {}: {e}", self.cfg.visca))?;
            let deadline = Instant::now() + self.timeout;
            while let Some(p) = self.receive(deadline) {
                if p.kind == visca::TYPE_CONTROL_REPLY && p.payload == visca::RESET {
                    self.seq = if self.zero_first { u32::MAX } else { 0 };
                    return Ok(());
                }
            }
        }
        Err(format!("the FR7 is not answering at {}", self.cfg.visca))
    }

    /// Send one message and wait for what answers it. A lost packet is sent again
    /// under the same sequence number, as Sony's list asks.
    fn exchange(&mut self, kind: u16, payload: &[u8]) -> Result<Vec<u8>, String> {
        let mut resyncs = 0;
        'again: loop {
            self.seq = self.seq.wrapping_add(1);
            let seq = self.seq;
            let packet = Packet::new(kind, seq, payload).to_bytes();
            let began = Instant::now();
            for _ in 0..=self.retries {
                self.socket.send(&packet).map_err(|e| format!("could not send to {}: {e}", self.cfg.visca))?;
                let deadline = Instant::now() + self.timeout;
                while let Some(p) = self.receive(deadline) {
                    if p.kind == visca::TYPE_CONTROL_REPLY && p.payload == visca::BAD_SEQUENCE {
                        // Another controller, or a restart, moved the camera's counter.
                        resyncs += 1;
                        if resyncs > 2 {
                            return Err("the FR7 keeps refusing the sequence number; is another controller driving it?".into());
                        }
                        if resyncs == 2 {
                            self.zero_first = !self.zero_first;
                        }
                        self.reset()?;
                        continue 'again;
                    }
                    if p.kind != visca::TYPE_REPLY || p.seq != seq {
                        continue;
                    }
                    match visca::parse_reply(&p.payload) {
                        // An inquiry is answered by its data; a command by its acknowledgement.
                        Some(Reply::Ack) if kind == visca::TYPE_COMMAND => {
                            self.last_round_trip = began.elapsed();
                            return Ok(Vec::new());
                        }
                        Some(Reply::Done(data)) => {
                            self.last_round_trip = began.elapsed();
                            return Ok(data);
                        }
                        Some(Reply::Error(code)) => {
                            return Err(format!("the FR7 refused a command: {}", visca::error_text(code)))
                        }
                        _ => {}
                    }
                }
            }
            return Err(format!("the FR7 stopped answering at {}", self.cfg.visca));
        }
    }

    pub fn command(&mut self, payload: &[u8]) -> Result<(), String> {
        self.exchange(visca::TYPE_COMMAND, payload).map(|_| ())
    }

    pub fn inquire(&mut self, payload: &[u8]) -> Result<Vec<u8>, String> {
        self.exchange(visca::TYPE_INQUIRY, payload)
    }

    fn counts(&self, degrees: f64, sign: f64) -> i32 {
        (degrees * sign * self.cfg.counts_per_degree).round() as i32
    }

    /// Raw pan and tilt counts, as the camera gives them.
    pub fn read_counts(&mut self) -> Result<(i32, i32), String> {
        let data = self.inquire(&visca::pan_tilt_inquiry())?;
        visca::parse_pan_tilt(&data).ok_or_else(|| "the FR7's position answer was not understood".to_string())
    }

    /// Pan and tilt in degrees.
    pub fn read_pan_tilt(&mut self) -> Result<(f64, f64), String> {
        let (pan, tilt) = self.read_counts()?;
        let cpd = self.cfg.counts_per_degree;
        Ok((pan as f64 / cpd * self.cfg.pan_sign, tilt as f64 / cpd * self.cfg.tilt_sign))
    }

    pub fn read_zoom(&mut self) -> Result<f64, String> {
        let data = self.inquire(&visca::zoom_inquiry())?;
        let counts = visca::parse_value(&data).ok_or_else(|| "the FR7's zoom answer was not understood".to_string())?;
        Ok(counts as f64 / visca::ZOOM_TELE as f64)
    }

    pub fn read_focus(&mut self) -> Result<f64, String> {
        let data = self.inquire(&visca::focus_inquiry())?;
        let counts = visca::parse_value(&data).ok_or_else(|| "the FR7's focus answer was not understood".to_string())?;
        Ok(counts as f64 / 65535.0)
    }

    pub fn read_head(&mut self) -> Result<Head, String> {
        let (pan, tilt) = self.read_pan_tilt()?;
        Ok(Head { pan, tilt, zoom: self.read_zoom()?, focus: self.read_focus()? })
    }

    /// Pan and tilt at these speed steps and directions.
    pub fn drive_steps(&mut self, pan: (u8, Turn), tilt: (u8, Turn)) -> Result<(), String> {
        self.command(&visca::pan_tilt_drive(pan.0, tilt.0, pan.1, tilt.1))
    }

    /// Go to a pan and tilt, each axis at the slowest step that covers its distance in `seconds`.
    pub fn go_to(&mut self, from: (f64, f64), to: (f64, f64), seconds: f64) -> Result<(), String> {
        let seconds = seconds.max(0.05);
        let pan_step = visca::step_at_least(&self.table, (to.0 - from.0).abs() / seconds);
        let tilt_step = visca::step_at_least(&self.table, (to.1 - from.1).abs() / seconds);
        let (pan, tilt) = (self.counts(to.0, self.cfg.pan_sign), self.counts(to.1, self.cfg.tilt_sign));
        self.command(&visca::pan_tilt_absolute(pan_step, tilt_step, pan, tilt))
    }

    pub fn zoom_to(&mut self, zoom: f64) -> Result<(), String> {
        self.command(&visca::zoom_direct(zoom_counts(zoom)))
    }

    pub fn focus_to(&mut self, focus: f64) -> Result<(), String> {
        self.command(&visca::focus_direct(focus_counts(focus)))
    }

    /// Stop pan, tilt, zoom and focus. Every stop is sent even if an earlier one fails.
    pub fn stop_all(&mut self) -> Result<(), String> {
        let results =
            [self.command(&visca::pan_tilt_stop()), self.command(&visca::zoom_stop()), self.command(&visca::focus_stop())];
        results.into_iter().collect()
    }

    pub fn recall(&mut self, number: u32) -> Result<(), String> {
        self.command(&visca::preset_recall(number))
    }
}

pub fn zoom_counts(zoom: f64) -> u16 {
    (zoom.clamp(0.0, 1.0) * visca::ZOOM_TELE as f64).round() as u16
}

pub fn focus_counts(focus: f64) -> u16 {
    (focus.clamp(0.0, 1.0) * 65535.0).round() as u16
}

/// Preset numbers and names from `command/inquiry.cgi?inq=presetposition`.
pub fn preset_names(cgi: &mut Cgi) -> Result<Vec<(u32, String)>, String> {
    let text = cgi.get_text("/command/inquiry.cgi?inq=presetposition")?;
    let fields = cgi::fields(&text);
    let Some(list) = cgi::field(&fields, "PresetName") else {
        return Err("the FR7's preset list was not understood".into());
    };
    let parts: Vec<&str> = list.split(',').collect();
    Ok(parts
        .chunks(2)
        .filter_map(|pair| match pair {
            [number, name] => Some((number.trim().parse::<u32>().ok()?, name.trim().to_string())),
            _ => None,
        })
        .collect())
}

pub fn preset_picture(cgi: &mut Cgi, number: u32) -> Option<Vec<u8>> {
    cgi.get(&format!("/preset/presetimg{number}.jpg")).ok().filter(|b| b.starts_with(&[0xFF, 0xD8]))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode {
    Idle,
    Drive(Drive),
    Stop,
    Sync,
}

struct State {
    head: Head,
    read_at: ReadTimes,
    connected: bool,
    detail: String,
    round_trip_ms: f64,
    moving: bool,
    mode: Mode,
    told_at: Instant,
    /// The engine stopped sending targets while live, so the worker stopped the head.
    starved: bool,
    axes: AxisTable,
    sync_progress: Option<String>,
    sync_result: Option<Result<Vec<DevicePreset>, String>>,
    quit: bool,
}

pub struct Fr7 {
    cfg: Fr7Config,
    state: Arc<Mutex<State>>,
}

/// How long a live command stays good. The engine repeats it sixty times a second.
const STARVED_AFTER: Duration = Duration::from_millis(300);
/// Share of the position error closed per second.
const GAIN: f64 = 4.0;
/// A stopped head has to hold still for this many readings before it counts as stopped.
const STILL_READINGS: u32 = 2;

impl Fr7 {
    pub fn start(cfg: Fr7Config, axes: AxisTable) -> Fr7 {
        let now = Instant::now();
        let state = Arc::new(Mutex::new(State {
            head: Head::default(),
            read_at: ReadTimes { pan_tilt: now, zoom: now, focus: now },
            connected: false,
            detail: format!("Connecting to {}", cfg.visca),
            round_trip_ms: 0.0,
            moving: false,
            mode: Mode::Idle,
            told_at: now,
            starved: false,
            axes,
            sync_progress: None,
            sync_result: None,
            quit: false,
        }));
        let (shared, worker_cfg) = (state.clone(), cfg.clone());
        std::thread::Builder::new().name("fr7".into()).spawn(move || Worker::new(worker_cfg, shared).run()).expect("fr7 thread");
        Fr7 { cfg, state }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn status(&self) -> AdapterStatus {
        let s = self.lock();
        AdapterStatus {
            name: "FR7".into(),
            connected: s.connected,
            detail: s.detail.clone(),
            calibrated: None,
            round_trip_ms: s.connected.then_some(s.round_trip_ms),
            raw_position: None,
        }
    }

    pub fn problem(&self) -> Option<String> {
        let s = self.lock();
        if !s.connected {
            Some(format!("The FR7 is not answering at {}", self.cfg.visca))
        } else if s.starved {
            Some("The FR7 was stopped because the show stopped sending it targets".into())
        } else {
            None
        }
    }

    pub fn head(&self) -> (Head, ReadTimes) {
        let s = self.lock();
        (s.head, s.read_at)
    }

    pub fn round_trip_s(&self) -> f64 {
        self.lock().round_trip_ms / 1000.0
    }

    pub fn is_moving(&self) -> bool {
        let s = self.lock();
        s.moving || s.mode == Mode::Stop
    }

    pub fn set_axes(&self, axes: &AxisTable) {
        self.lock().axes = *axes;
    }

    pub fn drive(&self, cmd: &Drive) {
        let mut s = self.lock();
        // A preset sync owns the head until it ends or is stopped.
        if s.mode != Mode::Sync {
            s.mode = Mode::Drive(*cmd);
            s.told_at = Instant::now();
            s.starved = false;
        }
    }

    pub fn stop(&self) {
        let mut s = self.lock();
        if s.mode == Mode::Sync {
            s.sync_result = Some(Err("The preset sync was stopped".into()));
            s.sync_progress = None;
        }
        s.mode = Mode::Stop;
    }

    pub fn begin_sync(&self) -> Result<(), String> {
        let mut s = self.lock();
        if !s.connected {
            return Err(format!("The FR7 is not answering at {}", self.cfg.visca));
        }
        if s.mode == Mode::Sync {
            return Err("A preset sync is already running".into());
        }
        s.mode = Mode::Sync;
        s.sync_result = None;
        s.sync_progress = Some("Reading the preset list".into());
        Ok(())
    }

    pub fn take_sync(&self) -> Option<Result<Vec<DevicePreset>, String>> {
        let mut s = self.lock();
        let result = s.sync_result.take()?;
        s.sync_progress = None;
        Some(result)
    }

    pub fn sync_progress(&self) -> Option<String> {
        self.lock().sync_progress.clone()
    }
}

impl Drop for Fr7 {
    fn drop(&mut self) {
        self.lock().quit = true;
    }
}

/// Recalling presets one at a time to learn their poses.
struct SyncJob {
    list: Vec<(u32, String)>,
    index: usize,
    recalled_at: Option<Instant>,
    still: u32,
    out: Vec<DevicePreset>,
}

struct Worker {
    cfg: Fr7Config,
    state: Arc<Mutex<State>>,
    link: Option<Link>,
    cgi: Cgi,
    carry: [f64; 2],
    cycle: u64,
    last_sent: Option<Vec<u8>>,
    sent_at_cycle: u64,
    zoom_sent: Option<u16>,
    focus_sent: Option<u16>,
    focus_is_manual: bool,
    ride: Option<Ride>,
    stops_sent: u32,
    still: u32,
    sync: Option<SyncJob>,
    was: Mode,
}

impl Worker {
    fn new(cfg: Fr7Config, state: Arc<Mutex<State>>) -> Worker {
        let cgi = Cgi::new(&cfg.http, &cfg.user, &cfg.password, Duration::from_secs(3));
        Worker {
            cfg,
            state,
            link: None,
            cgi,
            carry: [0.0; 2],
            cycle: 0,
            last_sent: None,
            sent_at_cycle: 0,
            zoom_sent: None,
            focus_sent: None,
            focus_is_manual: false,
            ride: None,
            stops_sent: 0,
            still: 0,
            sync: None,
            was: Mode::Idle,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn run(mut self) {
        let mut pace = Loop::new(PERIOD);
        loop {
            if self.lock().quit {
                if let Some(link) = &mut self.link {
                    let _ = link.stop_all();
                }
                return;
            }
            if let Err(why) = self.cycle() {
                self.link = None;
                let mut s = self.lock();
                s.connected = false;
                s.moving = false;
                s.detail = why;
                if s.mode == Mode::Sync {
                    s.sync_result = Some(Err("The FR7 stopped answering during the preset sync".into()));
                    s.sync_progress = None;
                    s.mode = Mode::Idle;
                }
                drop(s);
                self.sync = None;
                // Do not hammer an address nothing answers at.
                std::thread::sleep(Duration::from_millis(250));
            }
            pace.wait();
        }
    }

    fn cycle(&mut self) -> Result<(), String> {
        let fresh = self.link.is_none();
        if fresh {
            self.link = Some(Link::connect(&self.cfg)?);
            self.last_sent = None;
            self.zoom_sent = None;
            self.focus_sent = None;
            self.focus_is_manual = false;
        }
        self.cycle += 1;
        let cycle = self.cycle;
        let link = self.link.as_mut().expect("connected above");
        if fresh {
            // Nothing is reported as connected until every axis has been read once.
            let head = link.read_head()?;
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            s.head = head;
            let now = Instant::now();
            s.read_at = ReadTimes { pan_tilt: now, zoom: now, focus: now };
            // A server that died while live leaves the head on its last speed command.
            if s.mode == Mode::Idle {
                s.mode = Mode::Stop;
            }
        }
        let before = self.state.lock().unwrap_or_else(|e| e.into_inner()).head;

        let (pan, tilt) = link.read_pan_tilt()?;
        let mut read_at = self.state.lock().unwrap_or_else(|e| e.into_inner()).read_at;
        read_at.pan_tilt = Instant::now() - link.last_round_trip / 2;
        let round_trip_ms = link.last_round_trip.as_secs_f64() * 1000.0;
        // Zoom and focus change slowly; they share the cycles between them.
        let mut head = Head { pan, tilt, ..before };
        if cycle.is_multiple_of(2) {
            head.zoom = link.read_zoom()?;
            read_at.zoom = Instant::now() - link.last_round_trip / 2;
        } else {
            head.focus = link.read_focus()?;
            read_at.focus = Instant::now() - link.last_round_trip / 2;
        }
        let cpd = self.cfg.counts_per_degree;
        let moved = (head.pan - before.pan).abs() > 1.5 / cpd
            || (head.tilt - before.tilt).abs() > 1.5 / cpd
            || (head.zoom - before.zoom).abs() > 2.0 / visca::ZOOM_TELE as f64
            || (head.focus - before.focus).abs() > 8.0 / 65535.0;

        let (mode, axes) = {
            let mut s = self.lock();
            s.head = head;
            s.read_at = read_at;
            s.connected = true;
            s.round_trip_ms = round_trip_ms;
            s.moving = moved;
            s.detail = format!("{} answers in {:.1} ms", self.cfg.visca, round_trip_ms);
            if matches!(s.mode, Mode::Drive(_)) && s.told_at.elapsed() > STARVED_AFTER {
                s.mode = Mode::Stop;
                s.starved = true;
            }
            (s.mode, s.axes)
        };
        if mode != self.was {
            if mode == Mode::Stop {
                self.stops_sent = 0;
                self.still = 0;
            }
            if !matches!(mode, Mode::Drive(_)) {
                self.ride = None;
                self.last_sent = None;
            }
            self.was = mode;
        }

        match mode {
            Mode::Idle => {}
            Mode::Drive(cmd) => self.follow(&cmd, &head, &axes)?,
            Mode::Stop => {
                // Stop is repeated until the head reads the same twice running.
                self.link.as_mut().expect("connected").stop_all()?;
                self.stops_sent += 1;
                self.still = if moved { 0 } else { self.still + 1 };
                self.zoom_sent = None;
                self.focus_sent = None;
                if self.stops_sent >= 2 && self.still >= STILL_READINGS {
                    let mut s = self.lock();
                    if s.mode == Mode::Stop {
                        s.mode = Mode::Idle;
                    }
                }
                self.sync = None;
            }
            Mode::Sync => self.sync_step(&head, moved)?,
        }
        Ok(())
    }

    /// Steer toward the commanded pose: its own speed plus a share of the error.
    fn follow(&mut self, cmd: &Drive, head: &Head, axes: &AxisTable) -> Result<(), String> {
        let link = self.link.as_mut().expect("connected");
        if !self.focus_is_manual {
            link.command(&visca::focus_manual())?;
            self.focus_is_manual = true;
        }
        let ride = cmd.ride.filter(|_| self.cfg.absolute_moves);
        if let Some(ride) = ride {
            // One absolute command per move; the head runs its own ramp to the end pose.
            if self.ride.is_none_or(|r| r.pan != ride.pan || r.tilt != ride.tilt) {
                link.go_to((head.pan, head.tilt), (ride.pan, ride.tilt), ride.seconds)?;
                self.ride = Some(ride);
                self.last_sent = None;
            }
        } else {
            self.ride = None;
            let scale = cmd.scale.clamp(0.0, 1.0);
            let table = link.speed_table().to_vec();
            let want = |target: f64, feed: f64, at: f64, top: f64, carry: &mut f64| {
                let speed = (feed + GAIN * (target - at)).clamp(-top * scale, top * scale);
                visca::step_for(&table, speed, carry)
            };
            let [pan_carry, tilt_carry] = &mut self.carry;
            let pan = want(cmd.target.pan, cmd.velocity.pan, head.pan, axes.pan.max_speed, pan_carry);
            let tilt = want(cmd.target.tilt, cmd.velocity.tilt, head.tilt, axes.tilt.max_speed, tilt_carry);
            let bytes = visca::pan_tilt_drive(pan.0, tilt.0, pan.1, tilt.1);
            // The same command is not repeated every cycle, but it is refreshed twice a second.
            if self.last_sent.as_ref() != Some(&bytes) || self.cycle - self.sent_at_cycle >= 10 {
                link.command(&bytes)?;
                self.last_sent = Some(bytes);
                self.sent_at_cycle = self.cycle;
            }
        }
        let zoom = zoom_counts(cmd.target.zoom);
        if self.zoom_sent != Some(zoom) {
            link.command(&visca::zoom_direct(zoom))?;
            self.zoom_sent = Some(zoom);
        }
        let focus = focus_counts(cmd.target.focus);
        if self.focus_sent != Some(focus) {
            link.command(&visca::focus_direct(focus))?;
            self.focus_sent = Some(focus);
        }
        Ok(())
    }

    fn sync_step(&mut self, head: &Head, moved: bool) -> Result<(), String> {
        if self.sync.is_none() {
            let list = match preset_names(&mut self.cgi) {
                Ok(list) => list,
                Err(why) => {
                    self.finish_sync(Err(why));
                    return Ok(());
                }
            };
            self.sync = Some(SyncJob { list, index: 0, recalled_at: None, still: 0, out: Vec::new() });
        }
        let link = self.link.as_mut().expect("connected");
        let job = self.sync.as_mut().expect("set above");
        let Some((number, name)) = job.list.get(job.index).cloned() else {
            let out = std::mem::take(&mut job.out);
            self.finish_sync(Ok(out));
            return Ok(());
        };
        match job.recalled_at {
            None => {
                link.recall(number)?;
                job.recalled_at = Some(Instant::now());
                job.still = 0;
                let progress = format!("Recalling preset {} of {}: {name}", job.index + 1, job.list.len());
                self.lock().sync_progress = Some(progress);
            }
            Some(since) => {
                job.still = if moved { 0 } else { job.still + 1 };
                let waited = since.elapsed();
                if waited > Duration::from_secs(40) {
                    self.finish_sync(Err(format!("Preset {number} ({name}) never came to rest")));
                    return Ok(());
                }
                // The head takes a moment to start; then it has to stand still.
                if waited > Duration::from_millis(600) && job.still >= 6 {
                    let settled = Head { zoom: link.read_zoom()?, focus: link.read_focus()?, ..*head };
                    let thumbnail = preset_picture(&mut self.cgi, number);
                    let job = self.sync.as_mut().expect("still set");
                    job.out.push(DevicePreset {
                        preset: Preset {
                            id: String::new(),
                            camera: ptz_core::camera::first_camera(),
                            name,
                            pose: Pose {
                                pan: settled.pan,
                                tilt: settled.tilt,
                                zoom: settled.zoom,
                                focus: settled.focus,
                                elevation: 0.0,
                            },
                            camera_number: Some(number),
                            has_thumbnail: thumbnail.is_some(),
                        },
                        thumbnail,
                    });
                    job.index += 1;
                    job.recalled_at = None;
                }
            }
        }
        Ok(())
    }

    fn finish_sync(&mut self, result: Result<Vec<DevicePreset>, String>) {
        self.sync = None;
        let mut s = self.lock();
        s.sync_result = Some(result);
        // The progress line stays until the show has taken the result.
        s.sync_progress = Some("Storing the presets".into());
        if s.mode == Mode::Sync {
            s.mode = Mode::Idle;
        }
    }
}
