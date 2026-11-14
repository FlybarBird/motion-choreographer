//! Audit: the bring-up tool interrupted in the middle of a run.

use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Run one moving step of `ptz-bringup --yes` against a stand-in, press Ctrl-C as soon as the
/// stand-in moves, and say whether it is still moving a second and a half later.
fn interrupt(args: &[String], moving: &dyn Fn() -> bool) -> (bool, String) {
    let report = std::env::temp_dir().join(format!("audit-bringup-{}-{}.json", std::process::id(), args.len()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_ptz-bringup"))
        .args(args)
        .args(["--yes", "--quick", "--report", report.to_str().unwrap()])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("ptz-bringup binary");
    let deadline = Instant::now() + Duration::from_secs(30);
    while !moving() {
        assert!(Instant::now() < deadline && child.try_wait().unwrap().is_none(), "the step never moved the rig");
        std::thread::sleep(Duration::from_millis(5));
    }
    std::thread::sleep(Duration::from_millis(100));
    assert!(Command::new("kill").args(["-INT", &child.id().to_string()]).status().unwrap().success());
    let out = child.wait_with_output().unwrap();
    std::thread::sleep(Duration::from_millis(1500));
    let _ = std::fs::remove_file(&report);
    (moving(), format!("exit {:?}", out.status))
}

#[test]
fn ctrl_c_during_the_autopod_step_stops_the_column() {
    let pod = FakeAutoPod::start();
    pod.place(200.0);
    let args = ["--autopod".to_string(), pod.addr.to_string(), "--only".into(), "autopod".into()];
    let (still_moving, how) = interrupt(&args, &|| pod.is_moving());
    assert!(!still_moving, "1.5 s after Ctrl-C ({how}) the column is still running, now at {:.0} mm", pod.height_mm());
}

#[test]
fn ctrl_c_during_the_speeds_step_stops_the_head() {
    let fr7 = FakeFr7::start();
    let args = [
        "--fr7".to_string(),
        fr7.visca.to_string(),
        "--fr7-http".into(),
        fr7.http.port().to_string(),
        "--only".into(),
        "speeds".into(),
    ];
    let (still_moving, how) = interrupt(&args, &|| fr7.is_moving());
    assert!(!still_moving, "1.5 s after Ctrl-C ({how}) the head is still turning, now at pan {:.1}", fr7.pose().0);
}
