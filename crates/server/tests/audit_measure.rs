//! Audit: measurements. These print numbers; they only fail when a stated bound is broken.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::camera::{CameraConfig, MAX_CAMERAS};
use ptz_core::client::Api;
use ptz_core::edit;
use ptz_core::pose::{Pose, Preset, Project, RigKind};
use ptz_core::transport::{Action, Output};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CAM: &str = ptz_core::FIRST_CAMERA;

/// Between the adapter and the stand-in camera: stamps every packet the camera is sent,
/// and can lose the answers (`mute`) or everything (`cut`).
struct ViscaProxy {
    addr: SocketAddr,
    mute: Arc<AtomicBool>,
    cut: Arc<AtomicBool>,
    heard: Arc<Mutex<Vec<(Instant, Vec<u8>)>>>,
    quit: Arc<AtomicBool>,
}

impl ViscaProxy {
    fn start(real: SocketAddr) -> ViscaProxy {
        let front = UdpSocket::bind("127.0.0.1:0").unwrap();
        let back = UdpSocket::bind("127.0.0.1:0").unwrap();
        back.connect(real).unwrap();
        let addr = front.local_addr().unwrap();
        for s in [&front, &back] {
            s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        }
        let flag = || Arc::new(AtomicBool::new(false));
        let (mute, cut, quit) = (flag(), flag(), flag());
        let heard = Arc::new(Mutex::new(Vec::new()));
        let client: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
        let (f, b, c, q, h, x) =
            (front.try_clone().unwrap(), back.try_clone().unwrap(), client.clone(), quit.clone(), heard.clone(), cut.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            while !q.load(Ordering::Relaxed) {
                if let Ok((n, from)) = f.recv_from(&mut buf) {
                    if x.load(Ordering::Relaxed) {
                        continue;
                    }
                    *c.lock().unwrap() = Some(from);
                    h.lock().unwrap().push((Instant::now(), buf[..n].to_vec()));
                    let _ = b.send(&buf[..n]);
                }
            }
        });
        let (m, q) = (mute.clone(), quit.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            while !q.load(Ordering::Relaxed) {
                if let Ok(n) = back.recv(&mut buf) {
                    if m.load(Ordering::Relaxed) {
                        continue;
                    }
                    if let Some(to) = *client.lock().unwrap() {
                        let _ = front.send_to(&buf[..n], to);
                    }
                }
            }
        });
        ViscaProxy { addr, mute, cut, heard, quit }
    }

    /// When each pan and tilt position question arrived: one per adapter cycle.
    fn cycles(&self) -> Vec<Instant> {
        let ask = [0x81, 0x09, 0x06, 0x12, 0xFF];
        self.heard.lock().unwrap().iter().filter(|(_, p)| p.len() > 8 && p[8..] == ask).map(|(at, _)| *at).collect()
    }
}

impl Drop for ViscaProxy {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

/// Between the adapter and the stand-in leg box. While `stall` is set it accepts the
/// connection and then says nothing, which is what a hung unit does.
struct PodProxy {
    addr: SocketAddr,
    stall: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
}

impl PodProxy {
    fn start(real: SocketAddr) -> PodProxy {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let (stall, quit) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let (s, q) = (stall.clone(), quit.clone());
        std::thread::spawn(move || {
            let mut held: Vec<TcpStream> = Vec::new();
            while !q.load(Ordering::Relaxed) {
                let Ok((mut client, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                };
                if s.load(Ordering::Relaxed) {
                    held.push(client);
                    continue;
                }
                held.clear();
                let _ = client.set_nonblocking(false);
                let _ = client.set_read_timeout(Some(Duration::from_millis(500)));
                let mut request = Vec::new();
                let mut chunk = [0u8; 512];
                while !request.ends_with(b"\r\n\r\n") {
                    match client.read(&mut chunk) {
                        Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                        _ => break,
                    }
                }
                let Ok(mut upstream) = TcpStream::connect(real) else { continue };
                let _ = upstream.write_all(&request);
                let mut reply = Vec::new();
                let _ = upstream.read_to_end(&mut reply);
                let _ = client.write_all(&reply);
            }
        });
        PodProxy { addr, stall, quit }
    }
}

impl Drop for PodProxy {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

struct Rig {
    fr7: FakeFr7,
    pod: FakeAutoPod,
    visca: ViscaProxy,
    http: PodProxy,
}

impl Rig {
    fn start() -> Rig {
        let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
        let (visca, http) = (ViscaProxy::start(fr7.visca), PodProxy::start(pod.addr));
        Rig { fr7, pod, visca, http }
    }

    fn settings(&self, camera: &mut CameraConfig) {
        camera.rig.kind = RigKind::Hardware;
        camera.rig.fr7_host = self.visca.addr.to_string();
        camera.rig.fr7_http_port = self.fr7.http.port();
        camera.rig.fr7_user = self.fr7.user.clone();
        camera.rig.fr7_password = self.fr7.password.clone();
        camera.rig.autopod_host = self.http.addr.to_string();
        camera.rig.autopod_calibration = FakeAutoPod::height_map();
    }

    fn is_moving(&self) -> bool {
        self.fr7.is_moving() || self.pod.is_moving()
    }
}

/// Twelve cameras on stand-ins, a committed song that moves every one of them for about ten seconds, armed.
fn twelve_armed(api: &Api, rigs: &[Rig]) -> Project {
    let mut project = api.project().unwrap();
    while project.cameras.len() < rigs.len() {
        let id = project.free_camera_id();
        let name = format!("Camera {}", project.cameras.len() + 1);
        project.cameras.push(CameraConfig::new(&id, &name));
    }
    let mut project = api.put_project(&project).unwrap();
    for (camera, rig) in project.cameras.iter_mut().zip(rigs) {
        rig.settings(camera);
    }
    let project = api.put_project(&project).unwrap();
    let ready = |c: &CameraStatus| c.hardware && c.adapters.len() == 2 && c.adapters.iter().all(|a| a.connected);
    wait_for(api, |s| s.cameras.len() == rigs.len() && s.cameras.iter().all(ready));

    let created = api.new_song("Twelve", None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let mut song = created.song;
    for camera in &project.cameras {
        let pose = Pose { pan: 150.0, elevation: 400.0, ..Pose::home() };
        let mark: Preset = api
            .post(
                "/presets",
                &StorePreset { name: format!("Far {}", camera.id), pose: Some(pose), camera: Some(camera.id.clone()) },
            )
            .unwrap();
        song = edit::insert_preset_move(&song, &project, &mark, 25).unwrap().0;
    }
    api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    do_act(api, Action::Arm);
    let st = wait_for(api, |s| s.transport.output == Output::Armed || s.telemetry.fault.is_some());
    assert_eq!(st.telemetry.fault, None);
    project
}

fn gaps_ms(times: &[Instant]) -> (f64, f64, f64) {
    let mut gaps: Vec<f64> = times.windows(2).map(|w| w[1].duration_since(w[0]).as_secs_f64() * 1000.0).collect();
    gaps.sort_by(f64::total_cmp);
    if gaps.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    (gaps[gaps.len() / 2], gaps[gaps.len() * 99 / 100], gaps[gaps.len() - 1])
}

#[test]
fn measure_estop_latency_and_loop_jitter_with_twelve_cameras() {
    let t = TestServer::start("audit-measure-twelve");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs: Vec<Rig> = (0..MAX_CAMERAS).map(|_| Rig::start()).collect();
    twelve_armed(&api, &rigs);

    // Loop jitter seen by a healthy camera (the first), all twelve healthy.
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.frame > 50);
    let from = Instant::now();
    std::thread::sleep(Duration::from_millis(2500));
    let healthy: Vec<Instant> = rigs[0].visca.cycles().into_iter().filter(|at| *at > from).collect();
    let (median, p99, worst) = gaps_ms(&healthy);
    println!("MEASURE adapter cycle at camera 1, twelve healthy: median {median:.1} ms, p99 {p99:.1} ms, worst {worst:.1} ms");

    // One camera's head goes dark and another's leg box hangs with the connection open.
    rigs[5].visca.cut.store(true, Ordering::Relaxed);
    rigs[7].http.stall.store(true, Ordering::Relaxed);
    let from = Instant::now();
    std::thread::sleep(Duration::from_millis(2500));
    let with_stalled: Vec<Instant> = rigs[0].visca.cycles().into_iter().filter(|at| *at > from).collect();
    let (median, p99, worst_stalled) = gaps_ms(&with_stalled);
    println!(
        "MEASURE adapter cycle at camera 1, camera 6 silent and camera 8's leg box hung: median {median:.1} ms, p99 {p99:.1} ms, worst {worst_stalled:.1} ms"
    );
    let st = api.status().unwrap();
    let held: Vec<&str> = st.cameras.iter().filter(|c| c.held).map(|c| c.id.as_str()).collect();
    println!("MEASURE held after the two failures: {held:?}; output {:?}", st.transport.output);
    assert_eq!(held, ["cam6", "cam8"]);
    assert!(worst_stalled < 100.0, "a stalled device delayed a healthy camera's cycle to {worst_stalled:.0} ms");

    // E-Stop with ten cameras mid-move, one silent and one hung.
    let live: Vec<usize> = (0..rigs.len()).filter(|i| *i != 5 && *i != 7).collect();
    assert!(live.iter().all(|i| rigs[*i].is_moving()));
    let pressed = Instant::now();
    let st = api.post_empty::<Status>("/estop").unwrap();
    let answered = pressed.elapsed();
    assert!(st.transport.estop);
    let mut stopped_at = vec![None; rigs.len()];
    while stopped_at.iter().enumerate().any(|(i, s): (usize, &Option<Duration>)| live.contains(&i) && s.is_none()) {
        for i in &live {
            if stopped_at[*i].is_none() && !rigs[*i].is_moving() {
                stopped_at[*i] = Some(pressed.elapsed());
            }
        }
        assert!(pressed.elapsed() < Duration::from_secs(3), "not every healthy rig stopped: {stopped_at:?}");
        std::thread::sleep(Duration::from_micros(500));
    }
    let slowest = live.iter().filter_map(|i| stopped_at[*i]).max().unwrap();
    println!(
        "MEASURE E-Stop with one camera silent and one leg box hung: HTTP answered in {:.1} ms, last of the ten healthy rigs stopped {:.1} ms after the press",
        answered.as_secs_f64() * 1000.0,
        slowest.as_secs_f64() * 1000.0
    );
    assert!(slowest < Duration::from_millis(200));
}

#[test]
fn measure_estop_latency_with_twelve_healthy_cameras_and_a_busy_server() {
    let t = TestServer::start("audit-measure-estop");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rigs: Vec<Rig> = (0..MAX_CAMERAS).map(|_| Rig::start()).collect();
    twelve_armed(&api, &rigs);
    do_act(&api, Action::Play);
    wait_for(&api, |s| s.frame > 60);
    assert!(rigs.iter().all(Rig::is_moving));

    // Four other stations hammer the server while the button is pressed from a station without the lease.
    let busy = Arc::new(AtomicBool::new(true));
    let workers: Vec<_> = (0..4)
        .map(|n| {
            let (api, busy) = (t.client(&format!("busy{n}")), busy.clone());
            std::thread::spawn(move || {
                let mut done = 0u32;
                while busy.load(Ordering::Relaxed) {
                    let _ = api.get::<Vec<LogRecord>>("/log?since=0");
                    let _ = api.songs();
                    let _ = api.working("walk-in");
                    let _ = api.act(Action::Play, &TransportRequest::default());
                    done += 4;
                }
                done
            })
        })
        .collect();
    std::thread::sleep(Duration::from_millis(300));
    let watcher = t.client("watcher");
    let pressed = Instant::now();
    let st = watcher.post_empty::<Status>("/estop").unwrap();
    let answered = pressed.elapsed();
    assert!(st.transport.estop);
    let mut last = Duration::ZERO;
    let mut pending: Vec<usize> = (0..rigs.len()).collect();
    while !pending.is_empty() {
        pending.retain(|i| rigs[*i].is_moving());
        last = pressed.elapsed();
        assert!(last < Duration::from_secs(3), "rigs still moving: {pending:?}");
        std::thread::sleep(Duration::from_micros(500));
    }
    busy.store(false, Ordering::Relaxed);
    let requests: u32 = workers.into_iter().map(|w| w.join().unwrap()).sum();
    println!(
        "MEASURE E-Stop, twelve healthy cameras, {requests} other requests in flight around it: HTTP answered in {:.1} ms, all twelve heads and columns stopped {:.1} ms after the press",
        answered.as_secs_f64() * 1000.0,
        last.as_secs_f64() * 1000.0
    );
    assert!(last < Duration::from_millis(200));
    let st = wait_for(&watcher, |s| !s.telemetry.halting);
    assert!(st.transport.estop && !st.hardware_live);

    // Latched: nothing moves until Reset, and Reset needs the lease.
    for r in &rigs {
        r.fr7.clear_seen();
        r.pod.clear_seen();
    }
    assert!(refused(api.act(Action::Arm, &TransportRequest::default()), 409).contains("Reset E-Stop"));
    assert!(refused(api.act(Action::Play, &TransportRequest::default()), 409).contains("Reset E-Stop"));
    assert!(refused(api.sync_presets(CAM), 409).contains("Reset E-Stop"));
    assert_eq!(refused(watcher.post_empty::<Status>("/estop/reset"), 409), "Take the edit lease to control the show");
    // A second press while latched is accepted and changes nothing.
    assert!(watcher.post_empty::<Status>("/estop").unwrap().transport.estop);
    std::thread::sleep(Duration::from_millis(500));
    let stop = "#PTS5050".to_string();
    for (n, r) in rigs.iter().enumerate() {
        assert!(!r.is_moving(), "rig {} moved while latched", n + 1);
        let moves: Vec<Vec<u8>> = r
            .fr7
            .commands()
            .into_iter()
            .filter(|c| {
                *c != ptz_server::hw::visca::pan_tilt_stop()
                    && *c != ptz_server::hw::visca::zoom_stop()
                    && *c != ptz_server::hw::visca::focus_stop()
            })
            .collect();
        assert!(moves.is_empty(), "rig {} was sent {moves:02X?} while latched", n + 1);
        assert!(r.pod.orders().iter().all(|c| *c == stop), "rig {} leg box was sent {:?}", n + 1, r.pod.orders());
    }
    let halted: Vec<u64> = api
        .get::<Vec<LogRecord>>("/log?since=0")
        .unwrap()
        .iter()
        .filter(|r| r.kind == "estop_halted")
        .map(|r| r.data["after_ms"].as_u64().unwrap())
        .collect();
    println!("MEASURE estop_halted after_ms as logged, per camera: {halted:?}");
}

#[test]
fn measure_the_jog_deadman() {
    let t = TestServer::start("audit-measure-jog");
    let api = t.client("a");
    let other = t.client("b");
    api.take_lease("Luke-MacBook", false).unwrap();
    let pan = |api: &Api| api.status().unwrap().first().rig_pose.pan;

    // How long after the last jog message the simulated rig last moved.
    let last_motion_after = |after_jog: &dyn Fn()| -> f64 {
        api.take_lease("Luke-MacBook", true).unwrap();
        api.jog(CAM, &Jog { pan: 0.3, ..Jog::default() }).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        api.jog(CAM, &Jog { pan: 0.3, ..Jog::default() }).unwrap();
        let sent = Instant::now();
        after_jog();
        let (mut at, mut last_change) = (pan(&other), Duration::ZERO);
        while sent.elapsed() < Duration::from_millis(700) {
            let now = pan(&other);
            if now != at {
                (at, last_change) = (now, sent.elapsed());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        last_change.as_secs_f64() * 1000.0
    };

    let silence: Vec<f64> = (0..10).map(|_| last_motion_after(&|| {})).collect();
    let worst = silence.iter().copied().fold(0.0, f64::max);
    println!(
        "MEASURE jog deadman, messages stop arriving: last motion {silence:.0?} ms after the last message (worst {worst:.0} ms)"
    );

    let takeover: Vec<f64> = (0..5)
        .map(|_| {
            last_motion_after(&|| {
                other.take_lease("Booth", true).unwrap();
            })
        })
        .collect();
    println!("MEASURE jog deadman, another station takes the lease right after the last message: {takeover:.0?} ms");

    let other_camera: Vec<f64> = (0..5)
        .map(|_| {
            last_motion_after(&|| {
                // The operator selects something else and the app stops sending for this camera.
                let _ = api.status();
            })
        })
        .collect();
    println!("MEASURE jog deadman, the station stays connected but stops jogging: {other_camera:.0?} ms");

    api.take_lease("Luke-MacBook", true).unwrap();
    api.jog(CAM, &Jog { pan: 0.3, ..Jog::default() }).unwrap();
    let st = do_act(&api, Action::Arm);
    let armed_at = Instant::now();
    assert_ne!(st.transport.output, Output::Rehearse);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    let settled = pan(&api);
    std::thread::sleep(Duration::from_millis(400));
    println!(
        "MEASURE Arm pressed mid-jog: armed {:.0} ms later with the rig at pan {settled:.3}, and at pan {:.3} 400 ms after that",
        armed_at.elapsed().as_secs_f64() * 1000.0 - 400.0,
        pan(&api)
    );
    assert!(worst <= 250.0 + 17.0 + 15.0, "the rig moved {worst:.0} ms after the last jog message");
}
