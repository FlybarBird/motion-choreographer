//! The FR7 and AutoPod+ adapters against the protocol fakes: every byte that
//! goes on the wire is checked against the published command lists, and the
//! control loop is checked against a simulated head and column.

use ptz_core::pose::{Axis, AxisTable, Pose, RigConfig, RigKind};
use ptz_server::hw::autopod::{self, Calibration};
use ptz_server::hw::cgi::Cgi;
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7, FAKE_PICTURE};
use ptz_server::hw::fr7::{self, Fr7Config};
use ptz_server::hw::visca::{self, Turn};
use ptz_server::hw::HardwareRig;
use ptz_server::rig::{Drive, Ride, RigAdapter};
use std::time::{Duration, Instant};

fn fr7_config(fake: &FakeFr7) -> Fr7Config {
    Fr7Config {
        visca: fake.visca.to_string(),
        http: fake.http.to_string(),
        user: fake.user.clone(),
        password: fake.password.clone(),
        counts_per_degree: 236.0,
        pan_sign: 1.0,
        tilt_sign: 1.0,
        absolute_moves: false,
        speed_steps: Vec::new(),
    }
}

fn rig_config(fr7: Option<&FakeFr7>, pod: Option<&FakeAutoPod>) -> RigConfig {
    RigConfig {
        kind: RigKind::Hardware,
        fr7_host: fr7.map(|f| f.visca.to_string()).unwrap_or_default(),
        fr7_http_port: fr7.map(|f| f.http.port()).unwrap_or(80),
        fr7_user: fr7.map(|f| f.user.clone()).unwrap_or_default(),
        fr7_password: fr7.map(|f| f.password.clone()).unwrap_or_default(),
        autopod_host: pod.map(|p| p.addr.to_string()).unwrap_or_default(),
        autopod_calibration: FakeAutoPod::height_map(),
        ..RigConfig::default()
    }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn every_fr7_command_reaches_the_camera_as_the_published_bytes() {
    let fake = FakeFr7::start();
    fake.place(10.0, -5.0, 0.25, 0.5);
    let mut link = fr7::Link::connect(&fr7_config(&fake)).unwrap();

    let head = link.read_head().unwrap();
    assert!((head.pan - 10.0).abs() < 0.01 && (head.tilt + 5.0).abs() < 0.01, "{head:?}");
    assert!((head.zoom - 0.25).abs() < 0.001 && (head.focus - 0.5).abs() < 0.001, "{head:?}");

    link.drive_steps((0x0C, Turn::Plus), (0x05, Turn::Minus)).unwrap();
    link.command(&visca::pan_tilt_stop()).unwrap();
    link.go_to((10.0, -5.0), (30.0, -10.0), 1.0).unwrap();
    link.zoom_to(0.5).unwrap();
    let refused = link.focus_to(0.25).unwrap_err();
    assert!(refused.contains("current mode"), "focus is automatic until it is set to manual: {refused}");
    link.command(&visca::focus_manual()).unwrap();
    link.focus_to(0.25).unwrap();
    link.stop_all().unwrap();
    link.recall(2).unwrap();

    let table = visca::speed_table();
    let pan_step = visca::step_at_least(&table, 20.0);
    let tilt_step = visca::step_at_least(&table, 5.0);
    let seen: Vec<Vec<u8>> = fake.seen().into_iter().map(|s| s.payload).collect();
    let expect: Vec<Vec<u8>> = vec![
        vec![0x81, 0x09, 0x06, 0x12, 0xFF],
        vec![0x81, 0x09, 0x04, 0x47, 0xFF],
        vec![0x81, 0x09, 0x04, 0x48, 0xFF],
        vec![0x81, 0x01, 0x06, 0x01, 0x0C, 0x05, 0x01, 0x02, 0xFF],
        vec![0x81, 0x01, 0x06, 0x01, 0x18, 0x18, 0x03, 0x03, 0xFF],
        // 30 degrees is 7080 counts (0x01BA8); -10 degrees is -2360 (0xFF6C8).
        vec![0x81, 0x01, 0x06, 0x02, pan_step, tilt_step, 0x00, 0x01, 0x0B, 0x0A, 0x08, 0x0F, 0x0F, 0x06, 0x0C, 0x08, 0xFF],
        vec![0x81, 0x01, 0x04, 0x47, 0x02, 0x00, 0x00, 0x00, 0xFF],
        vec![0x81, 0x01, 0x04, 0x48, 0x04, 0x00, 0x00, 0x00, 0xFF],
        vec![0x81, 0x01, 0x04, 0x38, 0x03, 0xFF],
        vec![0x81, 0x01, 0x04, 0x48, 0x04, 0x00, 0x00, 0x00, 0xFF],
        vec![0x81, 0x01, 0x06, 0x01, 0x18, 0x18, 0x03, 0x03, 0xFF],
        vec![0x81, 0x01, 0x04, 0x07, 0x00, 0xFF],
        vec![0x81, 0x01, 0x04, 0x08, 0x00, 0xFF],
        vec![0x81, 0x01, 0x04, 0x3F, 0x02, 0x01, 0xFF],
    ];
    assert_eq!(seen, expect, "\nseen:\n{seen:02X?}");
    // Inquiries travel as inquiries, commands as commands.
    for s in fake.seen() {
        let inquiry = s.payload[1] == 0x09;
        assert_eq!(s.kind, if inquiry { visca::TYPE_INQUIRY } else { visca::TYPE_COMMAND });
    }

    // The recall runs the head to preset 2 on its own.
    wait_until("the head reaches preset 2", || {
        let (pan, tilt, zoom, _) = fake.pose();
        (pan + 24.0).abs() < 0.01 && (tilt - 1.5).abs() < 0.01 && (zoom - 0.62).abs() < 0.001
    });
}

#[test]
fn lost_packets_are_sent_again_and_a_foreign_sequence_number_is_resynced() {
    let fake = FakeFr7::start();
    let mut link = fr7::Link::connect(&fr7_config(&fake)).unwrap();
    link.read_counts().unwrap();

    // Two packets vanish: the same message is repeated until it is answered, and arrives once.
    fake.clear_seen();
    fake.drop_next(2);
    link.command(&visca::zoom_direct(0x1000)).unwrap();
    assert_eq!(fake.commands(), vec![visca::zoom_direct(0x1000)]);

    // More lost than the link retries: an error that says what happened, not a hang.
    fake.drop_next(5);
    let began = Instant::now();
    let err = link.read_counts().unwrap_err();
    assert!(err.contains("stopped answering"), "{err}");
    assert!(began.elapsed() < Duration::from_secs(1));
    fake.drop_next(0);

    // Another controller moved the camera's counter on. The link resets it and carries on.
    let mut other = fr7::Link::connect(&fr7_config(&fake)).unwrap();
    other.read_counts().unwrap();
    other.read_counts().unwrap();
    link.read_counts().expect("resynced after the other controller's reset");
    link.read_counts().unwrap();

    // A camera that counts from zero after a reset is accepted as well.
    let zero = FakeFr7::start();
    zero.expect_zero_after_reset();
    let mut link = fr7::Link::connect(&fr7_config(&zero)).unwrap();
    assert_eq!(link.read_counts().unwrap(), (0, 0));
    assert_eq!(link.read_counts().unwrap(), (0, 0));

    // Nothing listening at all.
    let dead = Fr7Config { visca: "127.0.0.1:9".into(), ..fr7_config(&fake) };
    let err = fr7::Link::connect(&dead).err().unwrap();
    assert!(err.contains("not answering at 127.0.0.1:9"), "{err}");
}

#[test]
fn preset_names_and_pictures_come_through_digest_authentication() {
    let fake = FakeFr7::start();
    let mut cgi = Cgi::new(&fake.http.to_string(), &fake.user, &fake.password, Duration::from_secs(2));
    let names = fr7::preset_names(&mut cgi).unwrap();
    assert_eq!(names, vec![(1, "Wide".to_string()), (2, "Lectern".to_string()), (3, "Two-Shot".to_string())]);
    assert_eq!(fr7::preset_picture(&mut cgi, 2), Some(FAKE_PICTURE.to_vec()));
    assert_eq!(fr7::preset_picture(&mut cgi, 9), None);
    assert_eq!(
        fake.web_requests(),
        vec!["/command/inquiry.cgi?inq=presetposition", "/preset/presetimg2.jpg", "/preset/presetimg9.jpg"]
    );

    let mut wrong = Cgi::new(&fake.http.to_string(), &fake.user, "not-the-password", Duration::from_secs(2));
    let err = fr7::preset_names(&mut wrong).unwrap_err();
    assert!(err.contains("refused the user name or password"), "{err}");
}

#[test]
fn every_autopod_command_reaches_the_leg_box_as_the_published_text() {
    let fake = FakeAutoPod::start();
    fake.place(100.0);
    let mut link = autopod::Link::new(&fake.addr.to_string());
    assert_eq!(link.calibration().unwrap(), Calibration::Calibrated);
    let raw = link.position().unwrap();
    assert_eq!(autopod::height_mm(&FakeAutoPod::height_map(), raw as f64).map(f64::round), Some(100.0));
    link.velocity(1.0).unwrap();
    link.velocity(-0.5).unwrap();
    link.stop().unwrap();
    link.velocity(-1.0).unwrap();
    link.stop().unwrap();
    assert_eq!(fake.seen(), vec!["#RER", "#APC", "#PTS5099", "#PTS5025", "#PTS5050", "#PTS5000", "#PTS5050"]);
    assert!(!fake.is_moving());

    // What the unit says about itself, asked the way the TX asks.
    fake.clear_seen();
    let identity = link.identity().unwrap();
    assert_eq!(identity.label(), "AutoPod+ V3.49e");
    assert_eq!(link.power().unwrap(), Some(true));
    assert_eq!(fake.seen(), vec!["QID", "#V?", "#O"]);
    fake.set_identity("AW-UE150", "V1.2");
    assert_eq!(link.identity().unwrap().label(), "AW-UE150 (not an AutoPod+) V1.2");

    // A command the unit does not have is refused, which is not the same as the unit being gone.
    let err = link.send("#XYZ").unwrap_err();
    assert_eq!(err, "the AutoPod+ refused #XYZ (ER1)");
    assert!(link.last_refused);
    fake.refuse(Some("QID"));
    assert_eq!(link.identity().unwrap().model, None, "a unit that will not say its model is still usable");
    fake.refuse(None);
    assert!(link.position().is_ok() && !link.last_refused);

    fake.set_calibration("rER53");
    assert_eq!(link.calibration().unwrap(), Calibration::Uncalibrated);
    fake.set_silent(true);
    let err = link.position().unwrap_err();
    assert!(err.contains("not answering"), "{err}");
}

/// A pose that eases from `from` to `to` over `seconds`, and its velocity.
fn eased(from: Pose, to: Pose, seconds: f64, t: f64) -> (Pose, Pose) {
    let u = (t / seconds).clamp(0.0, 1.0);
    let s = u * u * (3.0 - 2.0 * u);
    let ds = if t > 0.0 && t < seconds { 6.0 * u * (1.0 - u) / seconds } else { 0.0 };
    (Pose::from_fn(|a| from.get(a) + (to.get(a) - from.get(a)) * s), Pose::from_fn(|a| (to.get(a) - from.get(a)) * ds))
}

#[test]
fn the_rig_follows_a_moving_target_and_touches_nothing_until_it_is_driven() {
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    let axes = AxisTable::default();
    let mut rig = HardwareRig::start(&rig_config(Some(&fr7), Some(&pod)), &axes);
    wait_until("both devices answer", || rig.problem().is_none());
    assert_eq!(rig.status().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["FR7", "AutoPod+"]);
    assert_eq!(rig.status()[1].calibrated, Some(true));

    // On connecting each adapter sends a stop, in case a server that died left the rig moving.
    fr7.settle_after_connect();
    pod.settle_after_connect();
    // Connected and idle, the adapters only ask where the rig is.
    std::thread::sleep(Duration::from_millis(400));
    assert!(fr7.commands().is_empty(), "idle sent {:02X?}", fr7.commands());
    assert!(fr7.seen().len() > 6, "position is polled");
    assert_eq!(pod.orders(), Vec::<String>::new());
    assert!(rig.jog(Default::default()).is_err(), "hand control is the simulator's");

    // A four second move on every axis, fed at the engine's rate.
    let from = Pose { pan: 0.0, tilt: 0.0, zoom: 0.0, focus: 0.5, elevation: 0.0 };
    let to = Pose { pan: 40.0, tilt: 12.0, zoom: 0.5, focus: 0.3, elevation: 80.0 };
    let began = Instant::now();
    let mut worst = Pose::default();
    let mut asked: Vec<(Instant, Pose)> = Vec::new();
    loop {
        let t = began.elapsed().as_secs_f64();
        let (target, velocity) = eased(from, to, 4.0, t);
        rig.drive(&Drive { target, velocity, scale: 1.0, ride: None });
        rig.tick(1.0 / 60.0, &axes);
        asked.push((Instant::now(), target));
        // Compare each reading with what was asked at the moment it was taken.
        let reading = rig.reading();
        let then = |a: Axis| {
            asked.iter().rev().find(|(at, _)| *at <= reading.at[a.index()]).map(|(_, p)| p.get(a)).unwrap_or(from.get(a))
        };
        if t > 0.3 {
            worst = Pose::from_fn(|a| worst.get(a).max((reading.pose.get(a) - then(a)).abs()));
        }
        if t > 5.0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    assert!(worst.pan < 0.6 && worst.tilt < 0.3, "pan and tilt strayed {worst:?}");
    assert!(worst.zoom < 0.02 && worst.focus < 0.02 && worst.elevation < 2.0, "strayed {worst:?}");
    let (pan, tilt, zoom, focus) = fr7.pose();
    assert!((pan - 40.0).abs() < 0.1 && (tilt - 12.0).abs() < 0.1, "ended at {pan}, {tilt}");
    assert!((zoom - 0.5).abs() < 0.002 && (focus - 0.3).abs() < 0.002);
    assert!((pod.height_mm() - 80.0).abs() < 1.0, "column ended at {}", pod.height_mm());

    // Going live sets focus to manual once, before the first focus position.
    let commands = fr7.commands();
    assert_eq!(commands[0], visca::focus_manual());
    assert_eq!(commands.iter().filter(|c| **c == visca::focus_manual()).count(), 1);
    assert!(commands.iter().all(|c| visca::parse_command(c).is_some()));
    assert!(pod.seen().iter().any(|c| c.starts_with("#PTS50") && c != "#PTS5050"));

    // Stop goes to every axis, more than once, until the rig reads as standing still.
    fr7.clear_seen();
    pod.clear_seen();
    rig.drive(&Drive { target: from, velocity: Pose::default(), scale: 1.0, ride: None });
    std::thread::sleep(Duration::from_millis(200));
    assert!(fr7.is_moving() && pod.is_moving());
    rig.stop();
    wait_until("the rig reads as standing still", || !rig.is_moving());
    assert!(!fr7.is_moving() && !pod.is_moving());
    let commands = fr7.commands();
    let stops = |bytes: Vec<u8>| commands.iter().filter(|c| **c == bytes).count();
    assert!(stops(visca::pan_tilt_stop()) >= 2 && stops(visca::zoom_stop()) >= 2 && stops(visca::focus_stop()) >= 2);
    assert!(pod.seen().iter().filter(|c| *c == "#PTS5050").count() >= 2);

    // A show that stops sending targets is a show that has hung: the adapters stop the rig themselves.
    rig.drive(&Drive { target: to, velocity: Pose::default(), scale: 1.0, ride: None });
    std::thread::sleep(Duration::from_millis(150));
    assert!(fr7.is_moving());
    wait_until("the watchdog stops the head", || !fr7.is_moving() && !pod.is_moving());
    assert!(rig.problem().unwrap().contains("stopped sending it targets"));
}

#[test]
fn an_absolute_move_is_one_command_and_the_head_runs_its_own_ramp() {
    let fr7 = FakeFr7::start();
    let axes = AxisTable::default();
    let mut cfg = rig_config(Some(&fr7), None);
    cfg.fr7_absolute_moves = true;
    let mut rig = HardwareRig::start(&cfg, &axes);
    wait_until("the camera answers", || rig.problem().is_none());
    fr7.settle_after_connect();

    // The step for each axis is the slowest that covers its distance in the time left when the
    // adapter first acts on the move, which is up to one of its turns after the move began. These
    // distances call for the same steps with 0.3 s of the two already gone, so which step is sent
    // does not hang on when that turn falls.
    let (pan_to, tilt_to) = (25.5, -8.25);
    let table = visca::speed_table();
    let steps = |seconds: f64| (visca::step_at_least(&table, pan_to / seconds), visca::step_at_least(&table, -tilt_to / seconds));
    let (pan_step, tilt_step) = steps(2.0);
    assert_eq!(steps(1.7), (pan_step, tilt_step), "the move sits too near a speed step to make a steady test");

    let ride = Ride { pan: pan_to, tilt: tilt_to, seconds: 2.0 };
    let began = Instant::now();
    while began.elapsed() < Duration::from_millis(2600) {
        let target = Pose { pan: pan_to * (began.elapsed().as_secs_f64() / 2.0).min(1.0), ..Pose::home() };
        // The time left shrinks every tick; it is still the same move.
        let left = (2.0 - began.elapsed().as_secs_f64()).max(0.0);
        rig.drive(&Drive { target, velocity: Pose::default(), scale: 1.0, ride: Some(Ride { seconds: left, ..ride }) });
        std::thread::sleep(Duration::from_millis(16));
    }
    let absolute: Vec<Vec<u8>> = fr7.commands().into_iter().filter(|c| c[..4] == [0x81, 0x01, 0x06, 0x02]).collect();
    // 25.5 degrees is 6018 counts, 0x01782; 8.25 degrees down is -1947, 0xFF865 in twenty bits.
    assert_eq!(
        absolute,
        vec![vec![0x81, 0x01, 0x06, 0x02, pan_step, tilt_step, 0x00, 0x01, 0x07, 0x08, 0x02, 0x0F, 0x0F, 0x08, 0x06, 0x05, 0xFF]]
    );
    assert!(!fr7.commands().iter().any(|c| c[..4] == [0x81, 0x01, 0x06, 0x01]), "no velocity commands during the move");
    let (pan, tilt, _, _) = fr7.pose();
    assert!((pan - pan_to).abs() < 0.01 && (tilt - tilt_to).abs() < 0.01, "ended at {pan}, {tilt}");
}

#[test]
fn a_camera_left_at_a_low_speed_is_given_its_full_speed_when_it_is_connected_to() {
    let axes = AxisTable::default();
    let speed_sets =
        |fake: &FakeFr7| -> Vec<String> { fake.web_requests().into_iter().filter(|r| r.contains("Speed")).collect() };
    let detail = |rig: &HardwareRig| rig.status()[0].detail.clone();

    // As whoever used it last left it: pan and tilt turned down to 58, zoom to 38, slow mode on,
    // and counting 50 speed steps where this program sends 24.
    let low = FakeFr7::start();
    low.set_speed_settings(58, 38, true);
    low.set_extended_speed_steps(true);
    let rig = HardwareRig::start(&rig_config(Some(&low), None), &axes);
    wait_until("the camera answers", || rig.problem().is_none());
    assert_eq!(low.speed_settings(), (100, 100, false));
    assert!(!low.extended_speed_steps());
    assert_eq!(
        speed_sets(&low),
        vec![
            "/command/ptzf.cgi?PanTiltSpeedScale=100",
            "/command/ptzf.cgi?ZoomSpeedScale=100",
            "/command/ptzf.cgi?PanTiltSpeedMode=normal",
            "/command/ptzf.cgi?PanTiltSpeedStep=normal"
        ]
    );
    let said = detail(&rig);
    assert!(
        said.ends_with(
            "Raised to full speed: pan and tilt speed scale from 58 to 100, zoom speed scale from 38 to 100, pan and tilt speed mode from slow to normal, pan and tilt speed steps from extended to normal"
        ),
        "{said}"
    );

    // A camera already at full speed is asked for nothing, and nothing is said about it.
    let full = FakeFr7::start();
    let rig = HardwareRig::start(&rig_config(Some(&full), None), &axes);
    wait_until("the camera answers", || rig.problem().is_none());
    assert_eq!(speed_sets(&full), Vec::<String>::new());
    assert!(detail(&rig).ends_with(" ms"), "{}", detail(&rig));

    // One that will not take the change is driven as it is, and the operator is told it is slow.
    let locked = FakeFr7::start();
    locked.set_speed_settings(58, 100, false);
    locked.lock_speed_settings();
    let rig = HardwareRig::start(&rig_config(Some(&locked), None), &axes);
    wait_until("the camera answers", || rig.problem().is_none());
    assert_eq!(locked.speed_settings(), (58, 100, false));
    let said = detail(&rig);
    assert!(said.contains("Left below full speed: pan and tilt speed scale stays at 58") && said.contains("403"), "{said}");
}

#[test]
fn a_missing_device_and_an_uncalibrated_pedestal_are_named() {
    let axes = AxisTable::default();
    let none = HardwareRig::start(&RigConfig { kind: RigKind::Hardware, ..RigConfig::default() }, &axes);
    assert!(none.problem().unwrap().contains("no FR7 or AutoPod+ address"));

    let pod = FakeAutoPod::start();
    pod.set_calibration("rER53");
    let rig = HardwareRig::start(&rig_config(None, Some(&pod)), &axes);
    wait_until("the leg box answers", || rig.status()[0].connected);
    assert_eq!(rig.status()[0].calibrated, Some(false));
    assert!(rig.problem().unwrap().contains("not calibrated"));
    pod.set_calibration("rER00");
    wait_until("calibration is noticed", || rig.problem().is_none());
    assert_eq!(rig.status()[0].raw_position, Some(4096.0));

    // Fewer than three measured heights is not a height map.
    let mut cfg = rig_config(None, Some(&pod));
    cfg.autopod_calibration.truncate(2);
    let rig = HardwareRig::start(&cfg, &axes);
    wait_until("the leg box answers", || rig.status()[0].connected);
    wait_until("the short map is named", || rig.problem().is_some_and(|p| p.contains("2 of the 3 measured heights")));

    pod.set_silent(true);
    wait_until("the silence is noticed", || rig.problem().is_some_and(|p| p.contains("not answering")));
}

/// One raw exchange with the stand-in, as bytes.
fn raw_exchange(addr: std::net::SocketAddr, request: &[u8]) -> String {
    use std::io::{Read, Write};
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream.write_all(request).unwrap();
    let mut reply = String::new();
    stream.read_to_string(&mut reply).unwrap();
    reply
}

#[test]
fn the_stand_in_leg_box_frames_its_replies_as_the_real_rx_does() {
    let fake = FakeAutoPod::start();
    fake.place(0.0);
    let reply = raw_exchange(fake.addr, &autopod::request_bytes("#APC"));
    assert_eq!(
        reply, "HTTP/1.0 200 OK\r\nServer: AutoPod+/1.7\r\nConnection: close\r\nContent-Type: text/plain\r\n\r\naPC80001000\r\n",
        "HTTP/1.0, no length, Content-Type last, body ending CRLF"
    );
    let body = |cmd: &str| autopod::parse_reply(raw_exchange(fake.addr, &autopod::request_bytes(cmd)).as_bytes()).unwrap();
    assert_eq!(body("#RER"), (200, "rER00".into()));
    assert_eq!(body("QID"), (200, "OID:AUTOPOD+".into()));
    assert_eq!(body("#V?"), (200, "V3.49e".into()));
    assert_eq!(body("#O"), (200, "p1".into()));
    assert_eq!(body("#T50"), (200, "tS50".into()));
    assert_eq!(body("#PTS5050"), (200, "pTS5050".into()));
    assert_eq!(body("#D0"), (200, "d00".into()));
    assert_eq!(body("#NOPE"), (200, "ER1:NOPE".into()), "an unknown command is refused the Panasonic way");
    let get = |path: &str| raw_exchange(fake.addr, format!("GET {path} HTTP/1.0\r\n\r\n").as_bytes());
    assert!(get("/cgi-bin/get_state").starts_with("HTTP/1.0 404 "), "RX v3.49e has no get_state");
    assert!(get("/cgi-bin/aw_cam?cmd=QID&res=1").ends_with("OID:AUTOPOD+\r\n"));
    assert!(get("/live/camdata.html").contains("OID:AUTOPOD+\r\nTITLE:"));

    // Full down is 00 and full up is 99; both move the column at its top speed.
    body("#PTS5000");
    assert!(fake.is_moving());
    body("#T50");
    assert!(!fake.is_moving(), "the #T form stops it too");
}

#[test]
fn standby_and_a_refused_speed_command_are_problems_that_keep_the_connection() {
    let pod = FakeAutoPod::start();
    let axes = AxisTable::default();
    let mut rig = HardwareRig::start(&rig_config(None, Some(&pod)), &axes);
    wait_until("the leg box answers", || rig.problem().is_none());
    let status = rig.status();
    assert!(status[0].detail.starts_with("AutoPod+ V3.49e at "), "{}", status[0].detail);

    pod.set_power(false);
    wait_until("standby is noticed", || rig.problem().is_some());
    assert_eq!(rig.problem().unwrap(), "The AutoPod+ is in standby. Switch it on at the leg box");
    assert!(rig.status()[0].connected);
    assert!(!pod.orders().iter().any(|c| c.starts_with("#O")), "the show never switches the unit on by itself");
    pod.set_power(true);
    wait_until("power is noticed", || rig.problem().is_none());

    // The unit is there but refuses the speed command: named, connected, and cleared once it accepts again.
    pod.refuse(Some("#PTS"));
    let target = Pose { elevation: 200.0, ..Pose::home() };
    let drive = Drive::to(target, 1.0);
    let began = Instant::now();
    while rig.problem().is_none() && began.elapsed() < Duration::from_secs(3) {
        rig.drive(&drive);
        std::thread::sleep(Duration::from_millis(20));
    }
    let why = rig.problem().expect("the refusal is reported");
    assert!(why.starts_with("The AutoPod+ refused #PTS50") && why.ends_with("(ER1)"), "{why}");
    assert!(rig.status()[0].connected, "a refusal is not a lost connection");
    assert_eq!(pod.height_mm(), 0.0);
    pod.refuse(None);
    let began = Instant::now();
    while rig.problem().is_some() && began.elapsed() < Duration::from_secs(3) {
        rig.drive(&drive);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(rig.problem(), None);
    rig.stop();
}

#[test]
fn a_leg_box_that_misses_a_few_requests_is_driven_on_and_one_that_falls_silent_is_named() {
    let pod = FakeAutoPod::start();
    let axes = AxisTable::default();
    let mut rig = HardwareRig::start(&rig_config(None, Some(&pod)), &axes);
    wait_until("the leg box answers", || rig.problem().is_none());
    pod.settle_after_connect();

    // Raised 60 mm, with three requests in a row going unanswered six times on the way.
    let drive = Drive::to(Pose { elevation: 60.0, ..Pose::home() }, 1.0);
    let began = Instant::now();
    let mut dropped = 0u32;
    while began.elapsed() < Duration::from_secs(3) {
        if dropped < 6 && began.elapsed() > Duration::from_millis(300) * (dropped + 1) {
            pod.drop_next(3);
            dropped += 1;
        }
        rig.drive(&drive);
        assert_eq!(rig.problem(), None, "after {dropped} gaps");
        assert!(rig.status()[0].connected, "after {dropped} gaps");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(dropped, 6);
    assert!((pod.height_mm() - 60.0).abs() < 1.5, "column ended at {}", pod.height_mm());

    // A leg box that answers nothing at all for a second is gone.
    pod.set_silent(true);
    let began = Instant::now();
    wait_until("the silence is noticed", || {
        rig.drive(&drive);
        rig.problem().is_some_and(|p| p.contains("not answering"))
    });
    assert!(began.elapsed() >= Duration::from_secs(1), "called gone after {:?}", began.elapsed());
    assert!(!rig.status()[0].connected);
}
