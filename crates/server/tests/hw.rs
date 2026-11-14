//! The FR7 and AutoPod adapters against the protocol fakes: every byte that
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
    assert_eq!(fake.seen(), vec!["#RER", "#APC", "#PTS5099", "#PTS5026", "#PTS5050"]);
    assert!(!fake.is_moving());

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
    assert_eq!(rig.status().iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["FR7", "AutoPod"]);
    assert_eq!(rig.status()[1].calibrated, Some(true));

    // Connected and idle, the adapters only ask where the rig is.
    std::thread::sleep(Duration::from_millis(400));
    assert!(fr7.commands().is_empty(), "idle sent {:02X?}", fr7.commands());
    assert!(fr7.seen().len() > 6, "position is polled");
    assert!(pod.seen().iter().all(|c| c == "#APC" || c == "#RER"), "{:?}", pod.seen());
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

    let ride = Ride { pan: 30.0, tilt: -10.0, seconds: 2.0 };
    let began = Instant::now();
    while began.elapsed() < Duration::from_millis(2600) {
        let target = Pose { pan: 30.0 * (began.elapsed().as_secs_f64() / 2.0).min(1.0), ..Pose::home() };
        // The time left shrinks every tick; it is still the same move.
        let left = (2.0 - began.elapsed().as_secs_f64()).max(0.0);
        rig.drive(&Drive { target, velocity: Pose::default(), scale: 1.0, ride: Some(Ride { seconds: left, ..ride }) });
        std::thread::sleep(Duration::from_millis(16));
    }
    let absolute: Vec<Vec<u8>> = fr7.commands().into_iter().filter(|c| c[..4] == [0x81, 0x01, 0x06, 0x02]).collect();
    let table = visca::speed_table();
    let (pan_step, tilt_step) = (visca::step_at_least(&table, 15.0), visca::step_at_least(&table, 5.0));
    assert_eq!(
        absolute,
        vec![vec![0x81, 0x01, 0x06, 0x02, pan_step, tilt_step, 0x00, 0x01, 0x0B, 0x0A, 0x08, 0x0F, 0x0F, 0x06, 0x0C, 0x08, 0xFF]]
    );
    assert!(!fr7.commands().iter().any(|c| c[..4] == [0x81, 0x01, 0x06, 0x01]), "no velocity commands during the move");
    let (pan, tilt, _, _) = fr7.pose();
    assert!((pan - 30.0).abs() < 0.01 && (tilt + 10.0).abs() < 0.01, "ended at {pan}, {tilt}");
}

#[test]
fn a_missing_device_and_an_uncalibrated_pedestal_are_named() {
    let axes = AxisTable::default();
    let none = HardwareRig::start(&RigConfig { kind: RigKind::Hardware, ..RigConfig::default() }, &axes);
    assert!(none.problem().unwrap().contains("no FR7 or AutoPod address"));

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
