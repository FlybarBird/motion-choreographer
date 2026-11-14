//! The bring-up checklist run against the protocol fakes: what it measures has
//! to be what the fakes were set to, and it has to leave them where it found them.

mod common;

use common::*;
use ptz_core::api::{PutWorking, StorePreset};
use ptz_core::edit;
use ptz_core::pose::{Pose, Preset, RigKind};
use ptz_core::transport::{Action, Output};
use ptz_server::bringup::{self, Operator, Options, Report, Settings, Step, Unattended};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7, POD_RAW_PER_MM, POD_RAW_ZERO, POD_TOP_MM_S};
use ptz_server::hw::fr7::Fr7Config;
use ptz_server::hw::visca;

fn config(fake: &FakeFr7) -> Fr7Config {
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

/// Someone at the rig: agrees to every move and reads the column's height off a tape.
struct AtTheRig<'a> {
    pod: &'a FakeAutoPod,
    agree: bool,
    asked: Vec<String>,
    said: Vec<String>,
}

impl Operator for AtTheRig<'_> {
    fn say(&mut self, line: &str) {
        self.said.push(line.to_string());
    }

    fn confirm(&mut self, question: &str) -> bool {
        self.asked.push(question.to_string());
        self.agree
    }

    fn number(&mut self, question: &str, default: Option<f64>) -> Option<f64> {
        self.asked.push(question.to_string());
        if question.contains("millimetres") {
            Some(self.pod.height_mm().round())
        } else {
            default
        }
    }
}

fn outcomes(report: &Report) -> Vec<(&str, &str)> {
    report.sections.iter().map(|s| (s.step.as_str(), s.outcome.as_str())).collect()
}

#[test]
fn the_checklist_measures_what_the_rig_does_and_its_report_sets_up_the_project() {
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    // A head that counts pan the other way round and runs every step a fifth slow.
    fr7.set_count_signs(-1.0, 1.0);
    fr7.set_speed_factor(0.8);
    fr7.place(12.0, 4.0, 0.2, 0.5);
    pod.place(100.0);

    let opts = Options { fr7: Some(config(&fr7)), autopod: Some(pod.addr.to_string()), quick: true, ..Options::default() };
    let mut op = AtTheRig { pod: &pod, agree: true, asked: Vec::new(), said: Vec::new() };
    let report = bringup::run(&opts, &mut op);
    let text = report.sections.iter().flat_map(|s| s.lines.clone()).collect::<Vec<_>>().join("\n");
    assert!(outcomes(&report).iter().all(|(_, o)| *o == "passed"), "{:?}\n{text}", outcomes(&report));
    assert_eq!(report.problems, Vec::<String>::new(), "{text}");
    assert_eq!(report.sections.len(), 7);

    // Every step that moves asked first.
    let moving = op.asked.iter().filter(|q| q.contains("clear to move")).count();
    assert_eq!(moving, 5, "{:?}", op.asked);

    let s = &report.settings;
    assert_eq!((s.fr7_invert_pan, s.fr7_invert_tilt), (Some(true), Some(false)));
    assert_eq!(s.fr7_counts_per_degree, Some(236.0));
    assert_eq!(report.measured.plus_is_left, Some(true));

    // The table is the assumed one at four fifths of the speed, measured or filled in between.
    let table = s.fr7_speed_steps.as_ref().expect("a speed table");
    let assumed = visca::speed_table();
    assert_eq!(table.len(), assumed.len());
    for (i, (got, base)) in table.iter().zip(&assumed).enumerate() {
        let want = base * 0.8;
        assert!((got - want).abs() / want < 0.12, "step {}: {got} against {want}", i + 1);
    }
    assert!(table.windows(2).all(|w| w[1] >= w[0]));
    assert_eq!(report.measured.pan_step_speeds.len(), 5);
    let top = s.max_speed_pan.unwrap();
    assert!((top - 60.0 * 0.8 * 0.9).abs() < 5.0, "{top}");

    // With the table it measured, steering follows the curve; so it chooses to steer.
    assert_eq!(report.decision.as_deref(), Some("velocity tracking"));
    assert_eq!(s.fr7_absolute_moves, Some(false));
    assert!(report.measured.velocity_worst_error.unwrap() < 0.6, "{:?}", report.measured);
    assert!(report.measured.absolute_landing_error.unwrap() <= 0.05);
    assert_eq!(s.tolerance_pan, Some(1.0));
    assert!(s.query_delay_ms.unwrap() > 0.0 && s.query_delay_ms.unwrap() < 20.0);
    assert!(s.max_speed_zoom.unwrap() > 0.2 && s.max_speed_focus.unwrap() > 0.3);

    // The height map is the fake's own scale, and the top speed its top speed.
    let map = s.autopod_calibration.as_ref().expect("three heights");
    assert_eq!(map.len(), 3);
    for point in map {
        let mm = (point.raw - POD_RAW_ZERO) / POD_RAW_PER_MM;
        assert!((mm - point.mm).abs() <= 0.6, "{point:?}");
    }
    assert!(map[2].mm - map[0].mm > 30.0, "the heights are spread: {map:?}");
    let up = report.measured.autopod_up_mm_s.unwrap();
    assert!((up - POD_TOP_MM_S).abs() < 4.0, "{up}");
    assert!((report.measured.autopod_half_speed_share.unwrap() - 0.5).abs() < 0.06);
    assert!(s.max_speed_elevation.unwrap() < POD_TOP_MM_S);

    assert!(report.measured.estop_fr7_ms.unwrap() < 500.0 && report.measured.estop_autopod_ms.unwrap() < 500.0);

    // Everything is back where it stood, and standing still.
    assert!(!fr7.is_moving() && !pod.is_moving());
    let (pan, tilt, zoom, focus) = fr7.pose();
    assert!((pan - 12.0).abs() < 0.05 && (tilt - 4.0).abs() < 0.05, "head left at {pan}, {tilt}");
    assert!((zoom - 0.2).abs() < 0.005 && (focus - 0.5).abs() < 0.005, "lens left at {zoom}, {focus}");
    assert!((pod.height_mm() - 100.0).abs() < 2.5, "column left at {}", pod.height_mm());

    // The report survives the file, and applying it to a server's project sets the rig up.
    let dir = temp_dir("bringup-report");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("report.json");
    report.save(&path).unwrap();
    let loaded = Report::load(&path).unwrap();
    assert_eq!(loaded, report);
    assert!(report.summary().contains("Decision: velocity tracking"));
    let _ = std::fs::remove_dir_all(&dir);

    let t = TestServer::start("bringup-apply");
    let api = t.client("bringup");
    api.take_lease("ptz-bringup", false).unwrap();
    let mut project = api.project().unwrap();
    let before = project.clone();
    let changes = loaded.settings.apply(&mut project.cameras[0]);
    assert!(changes.iter().any(|c| c.starts_with("Pan counts inverted")), "{changes:?}");
    assert!(changes.iter().any(|c| c.starts_with("AutoPod+ height map: 3")));
    assert_eq!(project.cameras[0].rig.kind, before.cameras[0].rig.kind, "the rig kind is the operator's to switch");
    assert!(project.cameras[0].axes.pan.max_speed <= before.cameras[0].axes.pan.max_speed);
    assert!(project.cameras[0].axes.elevation.max_speed <= before.cameras[0].axes.elevation.max_speed);
    assert_eq!(loaded.settings.apply(&mut project.cameras[0]), Vec::<String>::new(), "applying twice changes nothing more");

    // The project it made drives the same rig: it connects, arms and reaches the playhead.
    project.cameras[0].rig.kind = RigKind::Hardware;
    project.cameras[0].rig.fr7_password = fr7.password.clone();
    api.put_project(&project).unwrap();
    wait_for(&api, |s| {
        s.first().adapters.len() == 2
            && s.first().adapters.iter().all(|a| a.connected)
            && s.first().adapters[1].calibrated == Some(true)
    });
    let st = api.status().unwrap();
    assert!((st.first().rig_pose.pan - 12.0).abs() < 0.1, "pan reads the right way round: {:?}", st.first().rig_pose);
    assert!((st.first().rig_pose.elevation - 100.0).abs() < 3.0, "{:?}", st.first().rig_pose);
    let mark: Preset = api
        .post(
            "/presets",
            &StorePreset { name: "Mark".into(), pose: Some(Pose { pan: 30.0, elevation: 160.0, ..Pose::home() }), camera: None },
        )
        .unwrap();
    let created = api.new_song("After bring-up", None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let (song, clip) = edit::insert_preset_move(&created.song, &project, &mark, 10).unwrap();
    let end = 10 + song.motion_clip(&clip).unwrap().duration_frames;
    api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed || s.telemetry.fault.is_some());
    do_act(&api, Action::Play);
    let st = wait_for(&api, |s| s.frame > end + 12 || s.telemetry.fault.is_some());
    assert_eq!(st.telemetry.fault, None);
    let (pan, _, _, _) = fr7.pose();
    assert!((pan - 30.0).abs() < 0.5 && (pod.height_mm() - 160.0).abs() < 4.0, "ended at pan {pan}, {} mm", pod.height_mm());
    do_act(&api, Action::Disarm);
}

#[test]
fn declining_at_the_prompt_moves_nothing() {
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    let opts = Options { fr7: Some(config(&fr7)), autopod: Some(pod.addr.to_string()), quick: true, ..Options::default() };
    let mut op = AtTheRig { pod: &pod, agree: false, asked: Vec::new(), said: Vec::new() };
    let report = bringup::run(&opts, &mut op);
    assert_eq!(
        outcomes(&report),
        [
            ("connect", "passed"),
            ("moves", "skipped"),
            ("timing", "passed"),
            ("speeds", "skipped"),
            ("tracking", "skipped"),
            ("autopod", "skipped"),
            ("estop", "skipped")
        ]
    );
    assert!(!report.failed());
    // The timing step sends stop for axes that already stand still; nothing else is a command.
    assert!(fr7.commands().iter().all(|c| *c == visca::pan_tilt_stop()), "{:02X?}", fr7.commands());
    assert_eq!(pod.orders(), Vec::<String>::new());
    assert_eq!(fr7.pose(), (0.0, 0.0, 0.0, fr7.pose().3));
    assert_eq!(report.settings.fr7_invert_pan, None);
    assert!(report.settings.query_delay_ms.is_some());
    assert_eq!(report.section(Step::Moves).unwrap().lines, ["Declined at the prompt; nothing was moved"]);
}

#[test]
fn a_rig_that_is_not_there_or_not_ready_is_reported_and_nothing_else_is_tried() {
    let pod = FakeAutoPod::start();
    let fr7 = FakeFr7::start();
    let dead = Fr7Config { visca: "127.0.0.1:9".into(), ..config(&fr7) };
    let opts = Options { fr7: Some(dead), autopod: Some(pod.addr.to_string()), quick: true, ..Options::default() };
    let report = bringup::run(&opts, &mut Unattended { echo: false });
    assert!(report.failed());
    assert_eq!(report.sections[0].outcome, "failed");
    assert_eq!(report.sections[0].lines, ["Failed: The FR7 is not answering at 127.0.0.1:9"]);
    assert!(report.sections[1..].iter().all(|s| s.outcome == "skipped"), "{:?}", outcomes(&report));
    assert!(report.sections[1].lines[0].starts_with("Skipped because connecting failed"));
    assert!(pod.orders().iter().all(|c| c == "#PTS5050"), "{:?}", pod.orders());

    // The leg box names itself at connect; in standby it is not driven at all.
    pod.clear_seen();
    let opts = Options { autopod: Some(pod.addr.to_string()), quick: true, steps: vec![Step::Connect], ..Options::default() };
    let report = bringup::run(&opts, &mut Unattended { echo: false });
    assert_eq!(outcomes(&report), [("connect", "passed")]);
    assert_eq!(report.measured.autopod_model.as_deref(), Some("AUTOPOD+"));
    assert_eq!(report.measured.autopod_firmware.as_deref(), Some("V3.49e"));
    assert!(report.sections[0].lines.iter().any(|l| l.starts_with("AutoPod+ V3.49e at ")), "{:?}", report.sections[0].lines);
    assert!(report.sections[0].lines.iter().any(|l| l == "AutoPod+ is switched on."));
    pod.set_power(false);
    let report = bringup::run(&opts, &mut Unattended { echo: false });
    assert_eq!(report.sections[0].lines.last().unwrap(), "Failed: The AutoPod+ is in standby. Switch it on at the leg box");
    assert!(pod.orders().iter().all(|c| c == "#PTS5050"), "a unit in standby is sent a stop at most: {:?}", pod.orders());
    pod.set_power(true);

    // An uncalibrated pedestal is named at connect and its step refuses to move it.
    pod.set_calibration("rER53");
    pod.clear_seen();
    let opts = Options {
        autopod: Some(pod.addr.to_string()),
        quick: true,
        steps: vec![Step::Connect, Step::AutoPod],
        ..Options::default()
    };
    let report = bringup::run(&opts, &mut Unattended { echo: false });
    assert_eq!(outcomes(&report), [("connect", "passed"), ("autopod", "failed")]);
    assert!(report.problems.iter().any(|p| p.contains("not calibrated")), "{:?}", report.problems);
    assert!(pod.orders().iter().all(|c| c == "#PTS5050"), "{:?}", pod.orders());
    assert_eq!(pod.height_mm(), 0.0);

    // Nobody to measure: the height map is left out, the speeds are still taken, and it says so.
    pod.set_calibration("rER00");
    pod.place(50.0);
    let opts = Options { autopod: Some(pod.addr.to_string()), quick: true, steps: vec![Step::AutoPod], ..Options::default() };
    let report = bringup::run(&opts, &mut Unattended { echo: false });
    assert_eq!(outcomes(&report), [("autopod", "passed")]);
    assert_eq!(report.settings.autopod_calibration, None);
    assert_eq!(report.settings.max_speed_elevation, None);
    assert!(report.sections[0].lines.iter().any(|l| l.starts_with("Height map: not taken")));
    assert!((pod.height_mm() - 50.0).abs() < 2.5);

    // A second run that starts from an earlier report's map gives the speed in millimetres.
    let start = Settings { autopod_calibration: Some(FakeAutoPod::height_map()), ..Settings::default() };
    let opts =
        Options { autopod: Some(pod.addr.to_string()), quick: true, steps: vec![Step::AutoPod], start, ..Options::default() };
    let report = bringup::run(&opts, &mut Unattended { echo: false });
    assert!(report.settings.max_speed_elevation.is_some_and(|v| v > 30.0 && v < POD_TOP_MM_S), "{:?}", report.settings);
}
