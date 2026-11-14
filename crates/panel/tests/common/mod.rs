#![allow(dead_code)]

//! A real show server in this process, the stand-in panel, and the controller
//! between them. Tests touch the show only through the panel's controls, and
//! read it back from the server's API and from the panel's displays.

use ptz_core::api::{Status, WorkingSong};
use ptz_core::client::Api;
use ptz_panel::codec::{Color, Lit, Text};
use ptz_panel::fake::FakePanel;
use ptz_panel::{Options, Running};
use ptz_server::engine::Config;
use ptz_server::http;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

// The PTZ Pro's component numbers, by the role the default map gives them.
pub const K1: u32 = 6;
pub const K2: u32 = 7;
pub const K3: u32 = 8;
pub const K4: u32 = 9;
pub const K5: u32 = 10;
pub const K6: u32 = 11;
pub const K7: u32 = 12;
pub const GO: u32 = 13;
pub const ESTOP: u32 = 56;
pub const SHOW: u32 = 27;
pub const TRANSPORT: u32 = 28;
pub const CAMERA: u32 = 29;
pub const PRESETS: u32 = 30;
pub const TIMELINE: u32 = 31;
pub const RECORD: u32 = 32;
pub const MEDIA: u32 = 33;
pub const SHIFT: u32 = 34;
pub const E1: u32 = 35;
pub const E2: u32 = 36;
pub const E3: u32 = 37;
pub const E4: u32 = 38;
pub const E1_TILE: u32 = 23;
pub const E2_TILE: u32 = 24;
pub const E3_TILE: u32 = 25;
pub const E4_TILE: u32 = 26;
pub const CAM1: u32 = 39;
pub const CAM2: u32 = 40;
pub const PAGE_UP: u32 = 49;
pub const PAGE_DOWN: u32 = 50;
pub const PLAYHEAD: u32 = 5;
pub const JOY_PAN: u32 = 1;
pub const JOY_TILT: u32 = 2;
pub const JOY_ZOOM: u32 = 3;
pub const STATUS_TILE: u32 = 15;
pub const MESSAGE_TILE: u32 = 16;

pub const KEYS: [u32; 7] = [K1, K2, K3, K4, K5, K6, K7];
pub const MODES: [u32; 7] = [SHOW, TRANSPORT, CAMERA, PRESETS, TIMELINE, RECORD, MEDIA];

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn temp_dir(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ptz-panel-test-{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

pub fn wait_for(what: &str, limit: Duration, mut done: impl FnMut() -> bool) {
    let start = Instant::now();
    while !done() {
        assert!(start.elapsed() < limit, "timed out waiting for: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub struct TestServer {
    pub dir: PathBuf,
    pub url: String,
    pub port: u16,
    running: Option<http::Running>,
}

fn launch(dir: &std::path::Path, port: u16, seed: bool) -> http::Running {
    let mut cfg = Config::new(dir);
    cfg.audio = ptz_server::engine::AudioChoice::Off;
    cfg.seed = seed;
    let bind = format!("127.0.0.1:{port}");
    let start = Instant::now();
    loop {
        match http::start(&cfg, &bind, None) {
            Ok(r) => return r,
            // A port just let go of may take a moment to come free.
            Err(_) if port != 0 && start.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => panic!("the server does not start on {bind}: {e}"),
        }
    }
}

impl TestServer {
    pub fn start(name: &str) -> TestServer {
        Self::open(temp_dir(name), 0, true)
    }

    /// Open a data folder on a given port; 0 picks a free one. `seed` gives a fresh folder the first-run songs.
    pub fn open(dir: PathBuf, port: u16, seed: bool) -> TestServer {
        let running = launch(&dir, port, seed);
        let url = running.url();
        let port = url.rsplit(':').next().and_then(|p| p.parse().ok()).expect("a port in the url");
        TestServer { dir, url, port, running: Some(running) }
    }

    /// Start a stopped server again: the same data folder on the same port.
    pub fn start_again(&mut self) {
        assert!(self.running.is_none(), "the server is still running");
        self.running = Some(launch(&self.dir, self.port, false));
    }

    pub fn client(&self, id: &str) -> Api {
        Api::new(&self.url, id, None)
    }

    /// Stop the server; its data folder stays.
    pub fn stop(&mut self) {
        self.running.take();
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.running.take();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub struct Bench {
    pub panel: FakePanel,
    pub controller: Option<Running>,
    /// Reads the show; never takes the lease and never acts.
    pub api: Api,
    pub server: TestServer,
    /// Between the controller and the server, when the test asked for it.
    pub proxy: Option<Proxy>,
}

impl Bench {
    /// The first-run show: two songs, presets and media.
    pub fn start(name: &str) -> Bench {
        Bench::with(name, true, |_| {})
    }

    /// A show with nothing in it: one simulated camera, no songs, no presets.
    pub fn empty(name: &str) -> Bench {
        Bench::with(name, false, |_| {})
    }

    pub fn with(name: &str, seed: bool, tweak: impl FnOnce(&mut Options)) -> Bench {
        Bench::build(name, seed, false, tweak)
    }

    /// As `start`, with the controller reaching the server through a proxy that can hold its requests back.
    pub fn proxied(name: &str) -> Bench {
        Bench::build(name, true, true, |_| {})
    }

    fn build(name: &str, seed: bool, proxied: bool, tweak: impl FnOnce(&mut Options)) -> Bench {
        let server = TestServer::open(temp_dir(name), 0, seed);
        let panel = FakePanel::start();
        let proxy = proxied.then(|| Proxy::start(server.port));
        let mut options = Options::new(&proxy.as_ref().map(|p| p.url.clone()).unwrap_or_else(|| server.url.clone()));
        options.panel = Some(panel.addr.to_string());
        options.quiet = true;
        tweak(&mut options);
        let controller = ptz_panel::start(options).expect("the controller starts");
        let api = server.client("observer");
        let bench = Bench { panel, controller: Some(controller), api, server, proxy };
        bench.wait_ready();
        bench
    }

    /// Change the stand-in and make the controller connect to it afresh.
    pub fn controller_restart_with(&self, change: impl FnOnce(&FakePanel)) {
        change(&self.panel);
        self.panel.drop_connections();
        self.wait("the panel to be gone", || self.status().panel.is_some_and(|p| !p.connected));
        self.wait_ready();
    }

    /// The panel shows a live show: E-Stop offered and the status of the server.
    pub fn wait_ready(&self) {
        self.wait("the panel to show the show", || {
            self.line(ESTOP) == "Press" && self.status().panel.is_some_and(|p| p.connected)
        });
    }

    pub fn wait(&self, what: &str, done: impl FnMut() -> bool) {
        wait_for(what, Duration::from_secs(8), done);
    }

    /// Base setup, which is done in the app: give the project `count` cameras.
    pub fn setup_cameras(&self, count: usize) {
        let setup = self.server.client("setup");
        setup.take_lease("Setup", false).expect("the setup station takes the lease");
        let mut project = setup.project().expect("the project");
        for n in project.cameras.len() + 1..=count {
            project.cameras.push(ptz_core::camera::CameraConfig::new(&format!("cam{n}"), &format!("Camera {n}")));
        }
        setup.put_project(&project).expect("the project is saved");
        setup.release_lease().expect("the setup station lets go");
        self.wait("the panel to see the cameras", || self.panel.lit(CAM1 + count as u32 - 1) != Lit::Off);
    }

    // ---- the show, from the server ----

    pub fn status(&self) -> Status {
        self.api.status().expect("status")
    }

    pub fn song(&self) -> WorkingSong {
        let id = self.status().song.expect("a song is loaded").id;
        self.api.working(&id).expect("the working song")
    }

    /// The message on the panel's main display, as the controller reports it to the server.
    pub fn message(&self) -> String {
        self.status().panel.map(|p| p.message).unwrap_or_default()
    }

    // ---- the panel, as the operator sees it ----

    pub fn text(&self, hwc: u32) -> Text {
        self.panel.text(hwc)
    }

    /// The words on a key or tile below its title.
    pub fn line(&self, hwc: u32) -> String {
        let t = self.panel.text(hwc);
        [t.line1, t.line2].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ")
    }

    pub fn title(&self, hwc: u32) -> String {
        self.panel.text(hwc).title
    }

    pub fn light(&self, hwc: u32) -> (Lit, Color) {
        (self.panel.lit(hwc), self.panel.color(hwc).unwrap_or_default())
    }

    /// The soft key that shows `label`, whichever it is now.
    pub fn key(&self, label: &str) -> u32 {
        let mut seen = Vec::new();
        for hwc in KEYS.into_iter().chain([GO]) {
            let line = self.line(hwc);
            if line == label {
                return hwc;
            }
            seen.push(format!("{hwc}: {} / {line}", self.title(hwc)));
        }
        panic!("no key shows \"{label}\". The keys show: {seen:?}");
    }

    pub fn wait_key(&self, label: &str) -> u32 {
        self.wait(&format!("a key that shows {label}"), || KEYS.into_iter().chain([GO]).any(|k| self.line(k) == label));
        self.key(label)
    }

    // ---- the operator's hands ----

    pub fn press(&self, hwc: u32) {
        self.panel.press(hwc);
    }

    pub fn hold(&self, hwc: u32, time: Duration) {
        self.panel.hold(hwc, time);
    }

    pub fn shift(&self, f: impl FnOnce()) {
        self.panel.down(SHIFT);
        std::thread::sleep(Duration::from_millis(60));
        f();
        std::thread::sleep(Duration::from_millis(60));
        self.panel.up(SHIFT);
        std::thread::sleep(Duration::from_millis(60));
    }

    pub fn turn(&self, hwc: u32, steps: i32) {
        self.panel.turn(hwc, steps);
    }

    pub fn mode(&self, hwc: u32) {
        self.press(hwc);
        self.wait("the mode key to light", || self.panel.lit(hwc) == Lit::On);
    }

    /// Press the key that shows `label` and wait for the message it leads to.
    pub fn press_key(&self, label: &str) {
        let key = self.wait_key(label);
        self.press(key);
    }

    /// Wait until the panel says exactly `text`: reported to the server in full,
    /// and on the main display as a part of it.
    pub fn wait_said(&self, text: &str) {
        self.wait_reported(text);
        self.wait(&format!("the main display to show part of \"{text}\""), || {
            let tile = self.text(MESSAGE_TILE);
            ["Note", "Refused", "Hold", "Confirm?"].iter().any(|t| tile.title.starts_with(t))
                && !tile.line1.is_empty()
                && text.contains(&tile.line1)
                && text.contains(&tile.line2)
        });
    }

    /// Wait until the panel has told the server it says exactly `text`. For when the
    /// panel itself cannot be read.
    pub fn wait_reported(&self, text: &str) {
        let start = Instant::now();
        loop {
            let last = self.message();
            if last == text {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(8), "the panel never said \"{text}\". It last said \"{last}\"");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Wait until the main display has shown `text`, over as many pages as it takes, under a title
    /// that starts with `title`. For when the server cannot be asked what the panel said.
    pub fn wait_shown(&self, title: &str, text: &str) {
        self.wait(&format!("the main display to show \"{text}\""), || {
            let pages: Vec<String> = self
                .panel
                .history(MESSAGE_TILE)
                .into_iter()
                .filter(|t| t.title.starts_with(title))
                .flat_map(|t| [t.line1, t.line2])
                .filter(|l| !l.is_empty())
                .collect();
            pages.join(" ").contains(text)
        });
    }

    /// The reason of the last action the server refused the panel, from the run log.
    pub fn last_refusal(&self) -> Option<(String, String, String)> {
        let word = |v: &serde_json::Value| v.as_str().unwrap_or_default().to_string();
        self.log()
            .into_iter()
            .rfind(|r| r.kind == "refused")
            .map(|r| (word(&r.data["action"]), word(&r.data["by"]), word(&r.data["reason"])))
    }

    /// Press a key and keep it down until `done`; says how long that took.
    pub fn hold_until(&self, hwc: u32, what: &str, done: impl FnMut() -> bool) -> Duration {
        let start = Instant::now();
        self.panel.down(hwc);
        self.wait(what, done);
        let took = start.elapsed();
        self.panel.up(hwc);
        took
    }

    /// As `wait_said`, and the display marks it as a refusal.
    pub fn wait_refused(&self, text: &str) {
        self.wait_said(text);
        assert!(self.title(MESSAGE_TILE).starts_with("Refused"), "\"{text}\" is shown as {:?}", self.text(MESSAGE_TILE));
    }

    pub fn wait_said_start(&self, start_of: &str) -> String {
        let start = Instant::now();
        loop {
            let last = self.message();
            if last.starts_with(start_of) {
                return last;
            }
            assert!(start.elapsed() < Duration::from_secs(8), "the panel never said \"{start_of}...\". It last said \"{last}\"");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Drive one joystick axis until the camera has moved, let go, and wait for it to stand still.
    pub fn drive(&self, axis: u32, camera: usize, read: impl Fn(&ptz_core::pose::Pose) -> f64) -> f64 {
        let before = read(&self.status().cameras[camera].pose);
        self.panel.deflect(axis, 400);
        self.wait("the camera to move under the joystick", || (read(&self.status().cameras[camera].pose) - before).abs() > 1e-3);
        std::thread::sleep(Duration::from_millis(300));
        self.panel.deflect(axis, 0);
        let mut last = f64::NAN;
        self.wait("the camera to stand still", || {
            let now = read(&self.status().cameras[camera].pose);
            let still = now == last;
            last = now;
            std::thread::sleep(Duration::from_millis(120));
            still
        });
        last
    }

    /// From the panel: take the lease, start a song, drive camera 1 to a pose, store it,
    /// place a move to it at frame 50, save and commit. Leaves the panel in Show.
    pub fn program(&self) {
        self.take_lease();
        self.new_song();
        self.mode(CAMERA);
        self.drive(JOY_PAN, 0, |p| p.pan);
        self.press_key("Store");
        self.wait_said_start("Stored Preset");
        self.mode(PRESETS);
        self.playhead_to(50);
        let before = self.song().song.motion.len();
        self.press(K2);
        self.wait("the move to be placed", || self.song().song.motion.len() == before + 1);
        self.mode(SHOW);
        self.press_key("Save");
        self.wait_said_start("Saved ");
        self.press_key("Commit");
        self.wait_said_start("Committed ");
    }

    /// Hold E-Stop long enough to reset it, and wait for the reset.
    pub fn reset_estop(&self) {
        self.hold(ESTOP, Duration::from_millis(1700));
        self.wait("E-Stop to be reset", || !self.status().transport.estop);
        self.wait("the E-Stop key to offer a press again", || self.line(ESTOP) == "Press");
    }

    pub fn log(&self) -> Vec<ptz_core::api::LogRecord> {
        self.api.get("/log?since=0").expect("the run log")
    }

    pub fn take_lease(&self) {
        self.mode(SHOW);
        self.press_key("Take");
        self.wait("the panel to hold the lease", || self.status().lease.is_some_and(|l| l.holder == "Panel"));
        self.wait_key("Release");
    }

    pub fn new_song(&self) {
        self.mode(SHOW);
        self.press_key("New");
        self.wait("a song to be loaded", || self.status().song.is_some());
    }

    /// Jog the playhead to a frame with the knob, a step at a time when close.
    pub fn playhead_to(&self, frame: i64) {
        let start = Instant::now();
        loop {
            let at = self.status().frame;
            if at == frame {
                return;
            }
            assert!(start.elapsed() < Duration::from_secs(20), "the playhead did not reach {frame}; it is at {at}");
            let away = frame - at;
            if away.abs() >= 25 {
                // Shift makes a step one second.
                self.panel.down(SHIFT);
                std::thread::sleep(Duration::from_millis(40));
                self.turn(PLAYHEAD, (away / 25).clamp(-20, 20) as i32);
                std::thread::sleep(Duration::from_millis(40));
                self.panel.up(SHIFT);
            } else {
                self.turn(PLAYHEAD, away.signum() as i32);
            }
            let want = if away.abs() >= 25 { at + (away / 25).clamp(-20, 20) * 25 } else { at + away.signum() };
            self.wait("the playhead to move", || self.status().frame == want);
            // Single steps close together would count as a fast turn.
            std::thread::sleep(Duration::from_millis(70));
        }
    }
}

/// The FR7 and AutoPod+ protocol stand-ins as camera 1's rig.
pub struct Rig {
    pub fr7: ptz_server::hw::fake::FakeFr7,
    pub pod: ptz_server::hw::fake::FakeAutoPod,
}

impl Rig {
    /// Base setup, which is done in the app: point camera 1 at the rig and wait until it answers.
    pub fn connect(b: &Bench) -> Rig {
        use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
        let rig = Rig { fr7: FakeFr7::start(), pod: FakeAutoPod::start() };
        let setup = b.server.client("setup");
        setup.take_lease("Setup", false).expect("the setup station takes the lease");
        let mut project = setup.project().expect("the project");
        let cam = &mut project.cameras[0].rig;
        cam.kind = ptz_core::pose::RigKind::Hardware;
        cam.fr7_host = rig.fr7.visca.to_string();
        cam.fr7_http_port = rig.fr7.http.port();
        cam.fr7_user = rig.fr7.user.clone();
        cam.fr7_password = rig.fr7.password.clone();
        cam.autopod_host = rig.pod.addr.to_string();
        cam.autopod_calibration = FakeAutoPod::height_map();
        setup.put_project(&project).expect("the project is saved");
        b.wait("the rig to answer", || {
            let s = b.status();
            let a = &s.first().adapters;
            a.len() == 2 && a.iter().all(|a| a.connected) && a[1].calibrated == Some(true)
        });
        setup.release_lease().expect("the setup station lets go");
        b.wait("the panel to show a real rig", || b.panel.color(CAM1) == Some(Color::Blue) && b.status().telemetry.hardware);
        rig
    }

    pub fn is_moving(&self) -> bool {
        self.fr7.is_moving() || self.pod.is_moving()
    }

    pub fn pose(&self) -> ptz_core::pose::Pose {
        let (pan, tilt, zoom, focus) = self.fr7.pose();
        ptz_core::pose::Pose { pan, tilt, zoom, focus, elevation: self.pod.height_mm() }
    }
}

/// A TCP proxy in front of the server. While frozen it holds back everything the
/// controller sends except an E-Stop, as a server that is slow to answer would.
pub struct Proxy {
    pub url: String,
    frozen: std::sync::Arc<std::sync::atomic::AtomicBool>,
    quit: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Proxy {
    pub fn start(server_port: u16) -> Proxy {
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        let listener = TcpListener::bind("127.0.0.1:0").expect("a free port for the proxy");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let frozen = Arc::new(AtomicBool::new(false));
        let quit = Arc::new(AtomicBool::new(false));
        listener.set_nonblocking(true).unwrap();
        let (f, q) = (frozen.clone(), quit.clone());
        std::thread::spawn(move || {
            while !q.load(Ordering::SeqCst) {
                let Ok((mut client, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let _ = client.set_nonblocking(false);
                let Ok(mut server) = TcpStream::connect(("127.0.0.1", server_port)) else { continue };
                let (_, _) = (client.set_nodelay(true), server.set_nodelay(true));
                let (Ok(mut client_back), Ok(mut server_back)) = (client.try_clone(), server.try_clone()) else { continue };
                std::thread::spawn(move || {
                    let _ = std::io::copy(&mut server_back, &mut client_back);
                    let _ = client_back.shutdown(std::net::Shutdown::Both);
                });
                let (f, q) = (f.clone(), q.clone());
                std::thread::spawn(move || {
                    let mut chunk = [0u8; 16384];
                    loop {
                        let n = match client.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => n,
                        };
                        let estop = chunk[..n].starts_with(b"POST /api/v1/estop ");
                        while f.load(Ordering::SeqCst) && !estop && !q.load(Ordering::SeqCst) {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        if server.write_all(&chunk[..n]).is_err() {
                            break;
                        }
                    }
                    let _ = server.shutdown(std::net::Shutdown::Both);
                });
            }
        });
        Proxy { url, frozen, quit }
    }

    pub fn freeze(&self, on: bool) {
        self.frozen.store(on, Ordering::SeqCst);
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.frozen.store(false, Ordering::SeqCst);
        self.quit.store(true, Ordering::SeqCst);
    }
}

// ---- the camera under the joystick ----

pub fn pan(b: &Bench) -> f64 {
    b.status().first().pose.pan
}

/// The panel takes the lease, a song is loaded, and camera 1 is being panned by the joystick.
pub fn start_panning(b: &Bench) {
    b.take_lease();
    if b.status().song.is_none() {
        b.new_song();
    }
    b.mode(CAMERA);
    let before = pan(b);
    b.panel.deflect(JOY_PAN, 400);
    b.wait("the camera to move under the joystick", || (pan(b) - before).abs() > 0.2);
}

/// How long after `since` the camera last moved. Returns once it has stood still for 300 ms.
pub fn stopped_after(b: &Bench, since: Instant) -> Duration {
    let mut last = pan(b);
    let mut last_moved = Instant::now();
    loop {
        let now = pan(b);
        if now != last {
            last = now;
            last_moved = Instant::now();
        }
        if last_moved.elapsed() > Duration::from_millis(300) {
            return last_moved.saturating_duration_since(since);
        }
        assert!(since.elapsed() < Duration::from_secs(10), "the camera is still moving 10 s later");
        std::thread::sleep(Duration::from_millis(4));
    }
}

pub fn stands_still(b: &Bench, time: Duration) -> bool {
    let at = pan(b);
    let until = Instant::now() + time;
    while Instant::now() < until {
        if pan(b) != at {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    true
}
