//! Audit: what the adapters do when a device answers badly or cannot be heard.

use ptz_core::pose::{AxisTable, Pose, RigConfig, RigKind};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use ptz_server::hw::HardwareRig;
use ptz_server::rig::{Drive, RigAdapter};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Sits between the adapter and the stand-in leg box. While `garble` holds bytes, the
/// answer to the position question is replaced by them; every command still reaches the box.
struct PodProxy {
    addr: SocketAddr,
    garble: Arc<Mutex<Option<Vec<u8>>>>,
    quit: Arc<AtomicBool>,
}

impl PodProxy {
    fn start(real: SocketAddr) -> PodProxy {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let garble: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let quit = Arc::new(AtomicBool::new(false));
        let (g, q) = (garble.clone(), quit.clone());
        std::thread::spawn(move || {
            while !q.load(Ordering::Relaxed) {
                let Ok((mut client, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                };
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
                let is_position = request.starts_with(b"GET /cgi-bin/aw_ptz?cmd=%23APC&res=1 ");
                if let Some(body) = g.lock().unwrap().clone().filter(|_| is_position) {
                    reply = b"HTTP/1.0 200 OK\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\n".to_vec();
                    reply.extend_from_slice(&body);
                    reply.extend_from_slice(b"\r\n");
                }
                let _ = client.write_all(&reply);
            }
        });
        PodProxy { addr, garble, quit }
    }

    fn garble(&self, body: Option<&[u8]>) {
        *self.garble.lock().unwrap() = body.map(<[u8]>::to_vec);
    }
}

impl Drop for PodProxy {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

fn pod_rig(host: SocketAddr) -> RigConfig {
    RigConfig {
        kind: RigKind::Hardware,
        autopod_host: host.to_string(),
        autopod_calibration: FakeAutoPod::height_map(),
        ..RigConfig::default()
    }
}

/// Drive the column upward until the stand-in is really moving.
fn raise(rig: &mut HardwareRig, pod: &FakeAutoPod) {
    let drive = Drive::to(Pose { elevation: 600.0, ..Pose::home() }, 1.0);
    let began = Instant::now();
    while began.elapsed() < Duration::from_millis(400) {
        rig.drive(&drive);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(pod.is_moving(), "the column is on its way up");
}

#[test]
fn parse_position_does_not_panic_on_a_reply_that_is_not_ascii() {
    // What `String::from_utf8_lossy` makes of the body bytes `aPC80\xE9000`.
    let answer = String::from_utf8_lossy(b"aPC80\xE9000").to_string();
    assert_eq!(ptz_server::hw::autopod::parse_position(&answer), None);
}

#[test]
fn a_garbled_position_answer_does_not_stop_the_column_being_stopped() {
    let pod = FakeAutoPod::start();
    let proxy = PodProxy::start(pod.addr);
    let axes = AxisTable::default();
    let mut rig = HardwareRig::start(&pod_rig(proxy.addr), &axes);
    wait_until("the leg box answers", || rig.problem().is_none());
    raise(&mut rig, &pod);

    // One byte of line noise in the position answer, then the operator presses E-Stop.
    proxy.garble(Some(b"aPC80\xE9000"));
    std::thread::sleep(Duration::from_millis(150));
    proxy.garble(None);
    rig.stop();
    std::thread::sleep(Duration::from_millis(1500));

    let height = pod.height_mm();
    let stops = pod.seen().iter().filter(|c| *c == "#PTS5050").count();
    assert!(
        !pod.is_moving(),
        "1.5 s after stop the column is still rising (now at {height:.0} mm), {stops} stop commands reached it, \
         and the server says: connected={}, problem={:?}",
        rig.status()[0].connected,
        rig.problem()
    );
}

#[test]
fn a_column_whose_position_cannot_be_read_is_still_told_to_stop() {
    let pod = FakeAutoPod::start();
    let proxy = PodProxy::start(pod.addr);
    let axes = AxisTable::default();
    let mut rig = HardwareRig::start(&pod_rig(proxy.addr), &axes);
    wait_until("the leg box answers", || rig.problem().is_none());
    raise(&mut rig, &pod);

    // The position answer turns to rubbish and stays that way. Speed commands still get through.
    proxy.garble(Some(b"aPC????"));
    std::thread::sleep(Duration::from_millis(100));
    rig.stop();
    std::thread::sleep(Duration::from_millis(1500));

    let stops = pod.seen().iter().filter(|c| *c == "#PTS5050").count();
    assert!(
        !pod.is_moving(),
        "1.5 s after stop the column is still rising (now at {:.0} mm); {stops} stop commands reached it; problem={:?}",
        pod.height_mm(),
        rig.problem()
    );
}

/// Sits between the adapter and the stand-in camera. While `mute` is set the camera
/// still hears every packet, but none of its answers get back.
struct ViscaProxy {
    addr: SocketAddr,
    mute: Arc<AtomicBool>,
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
        let (mute, quit) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let client: Arc<Mutex<Option<SocketAddr>>> = Arc::new(Mutex::new(None));
        let (f, b, c, q) = (front.try_clone().unwrap(), back.try_clone().unwrap(), client.clone(), quit.clone());
        std::thread::spawn(move || {
            let mut buf = [0u8; 256];
            while !q.load(Ordering::Relaxed) {
                if let Ok((n, from)) = f.recv_from(&mut buf) {
                    *c.lock().unwrap() = Some(from);
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
        ViscaProxy { addr, mute, quit }
    }
}

impl Drop for ViscaProxy {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

#[test]
fn a_camera_that_hears_but_cannot_be_heard_is_still_told_to_stop() {
    let fr7 = FakeFr7::start();
    let proxy = ViscaProxy::start(fr7.visca);
    let axes = AxisTable::default();
    let cfg = RigConfig {
        kind: RigKind::Hardware,
        fr7_host: proxy.addr.to_string(),
        fr7_http_port: fr7.http.port(),
        fr7_user: fr7.user.clone(),
        fr7_password: fr7.password.clone(),
        ..RigConfig::default()
    };
    let mut rig = HardwareRig::start(&cfg, &axes);
    wait_until("the camera answers", || rig.problem().is_none());

    // A long pan at full speed.
    let drive = Drive::to(Pose { pan: 160.0, ..Pose::home() }, 1.0);
    let began = Instant::now();
    while began.elapsed() < Duration::from_millis(400) {
        rig.drive(&drive);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(fr7.is_moving());

    // The return path fails: the camera's answers are lost, but it still hears the server.
    proxy.mute.store(true, Ordering::Relaxed);
    std::thread::sleep(Duration::from_millis(100));
    fr7.clear_seen();
    rig.stop();
    std::thread::sleep(Duration::from_millis(2000));

    let (pan, ..) = fr7.pose();
    assert!(
        !fr7.is_moving(),
        "2 s after stop the head is still panning (now at {pan:.0} degrees); commands that reached it since: {:02X?}; problem={:?}",
        fr7.commands(),
        rig.problem()
    );
}
