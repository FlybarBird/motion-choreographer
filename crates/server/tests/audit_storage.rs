//! Audit: what is on disk after a crash or a bad file, and what the server makes of it.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::edit;
use ptz_core::pose::{KeepOutRule, Project};
use ptz_core::transport::Action;
use std::time::Duration;

#[test]
fn a_crash_between_a_revision_and_its_record_does_not_block_every_later_commit() {
    let t = TestServer::start("audit-commit");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let project = api.project().unwrap();
    let created = api.new_song("Finale", None).unwrap();
    api.load_song("finale").unwrap();
    api.save("finale").unwrap();
    assert_eq!(api.commit("finale").unwrap().number, 1);

    // What a crash (or a full disk) leaves when it falls between the two writes of a commit:
    // `rev/0002.json` is there, `rev/0002.meta.json` is not.
    let rev = t.dir.join("songs/finale/rev");
    std::fs::copy(rev.join("0001.json"), rev.join("0002.json")).unwrap();
    let dir = t.stop_keeping_data();
    let t = TestServer::reopen(&dir);
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    api.load_song("finale").unwrap();

    // The operator carries on: one edit, Save, Commit.
    let song = edit::add_marker(&created.song, &project, 50, "Doors").unwrap();
    let w = api.working("finale").unwrap();
    api.put_working(&PutWorking { expected_counter: w.edit_counter, song }).unwrap();
    api.save("finale").unwrap();
    let commit = api.commit("finale");
    let arm = api.status().unwrap().guidance.reason(Action::Arm).map(str::to_string);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(commit.is_ok(), "Commit answers {commit:?}; Arm is blocked with: {arm:?}");
}

#[test]
fn a_project_file_that_cannot_be_read_is_not_silently_replaced_by_the_defaults() {
    let t = TestServer::start("audit-project");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let mut project = api.project().unwrap();
    project.cameras[0].axes.pan.max = 90.0;
    project.cameras[0].axes.elevation.max = 400.0;
    project.cameras[0].keep_out = vec![KeepOutRule::example()];
    let saved = api.put_project(&project).unwrap();
    let before: Vec<LogRecord> = api.get("/log?since=0").unwrap();
    let last = before.last().map(|r| r.seq).unwrap_or(0);
    drop(api);
    let dir = t.stop_keeping_data();

    // The file loses its second half: a failing disk, or a copy that was cut short.
    let path = dir.join("project.json");
    let bytes = std::fs::read(&path).unwrap();
    std::fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();

    let reopened = std::panic::catch_unwind(|| TestServer::reopen(&dir));
    let Ok(t) = reopened else {
        // Refusing to start on a broken project is a fine answer.
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };
    let api = t.client("a");
    let now: Project = api.project().unwrap();
    let st = api.status().unwrap();
    let log: Vec<LogRecord> = api.get(&format!("/log?since={last}")).unwrap();
    let told =
        st.telemetry.fault.is_some() || log.iter().any(|r| r.kind.contains("project") || r.data.to_string().contains("project"));
    let kept_aside = std::fs::read_dir(&dir).unwrap().flatten().any(|e| {
        let name = e.file_name().to_string_lossy().to_string();
        name != "project.json" && name.contains("project")
    });
    assert!(
        now == saved || told,
        "the server came up on a default project and told nobody (log since the restart: {:?}; damaged file kept aside: {kept_aside}): pan limit {} (was {}), column limit {} (was {}), \
         {} keep-out rules (was {}); Arm is {}",
        log.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>(),
        now.cameras[0].axes.pan.max,
        saved.cameras[0].axes.pan.max,
        now.cameras[0].axes.elevation.max,
        saved.cameras[0].axes.elevation.max,
        now.cameras[0].keep_out.len(),
        saved.cameras[0].keep_out.len(),
        st.guidance.reason(Action::Arm).unwrap_or("allowed"),
    );
}

#[test]
fn a_project_file_with_an_unusable_limit_does_not_stop_the_show_clock() {
    let t = TestServer::start("audit-limits");
    let dir = t.stop_keeping_data();
    // A hand edit that swaps the two ends of the pan range.
    let path = dir.join("project.json");
    let mut project: serde_json::Value = read_json(&path);
    project["cameras"][0]["axes"]["pan"]["min"] = 170.0.into();
    project["cameras"][0]["axes"]["pan"]["max"] = (-170.0).into();
    std::fs::write(&path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();

    let Ok(t) = std::panic::catch_unwind(|| TestServer::reopen(&dir)) else {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    };
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    // The operator nudges the simulated rig, then presses Play.
    let jog = api.jog(ptz_core::FIRST_CAMERA, &Jog { pan: 0.5, ..Jog::default() });
    std::thread::sleep(Duration::from_millis(400));
    let play = api.act(Action::Play, &TransportRequest::default());
    std::thread::sleep(Duration::from_millis(1000));
    let st = api.status().unwrap();
    assert!(jog.is_ok() || play.is_ok(), "{jog:?} {play:?}");
    assert!(
        play.is_err() || st.frame > 10,
        "Play was accepted, the transport says {:?}, and one second later the playhead is at frame {}",
        st.transport.run,
        st.frame
    );
}
