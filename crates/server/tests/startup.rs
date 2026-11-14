//! What the server makes of a project file it cannot trust: the file holds each rig's
//! limits and keep-out rules, so the server does not start without them.

mod common;

use common::*;
use ptz_core::pose::KeepOutRule;
use ptz_server::engine::{AudioChoice, Config};
use ptz_server::http;
use std::path::Path;

fn start(dir: &Path) -> Result<http::Running, String> {
    let mut cfg = Config::new(dir);
    cfg.audio = AudioChoice::Off;
    http::start(&cfg, "127.0.0.1:0", None)
}

#[test]
fn a_damaged_project_file_stops_the_server_from_starting_on_default_limits() {
    let t = TestServer::start("startup-damaged");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let mut project = api.project().unwrap();
    project.cameras[0].axes.pan.max = 90.0;
    project.cameras[0].axes.elevation.max = 400.0;
    project.cameras[0].keep_out = vec![KeepOutRule::example()];
    let saved = api.put_project(&project).unwrap();
    drop(api);
    let dir = t.stop_keeping_data();

    // The file loses its second half: a failing disk, or a copy that was cut short.
    let path = dir.join("project.json");
    let whole = std::fs::read(&path).unwrap();
    let damaged = &whole[..whole.len() / 2];
    std::fs::write(&path, damaged).unwrap();

    // Starting on the defaults would widen the pan and column limits and drop the keep-out rule.
    let why = start(&dir).err().expect("the server does not start");
    assert!(why.contains("project.json") && why.contains("limits and keep-out rules"), "{why}");
    assert_eq!(std::fs::read(&path).unwrap(), damaged, "the damaged file is left as it was found");

    // With the file put back, the server starts on what was saved.
    std::fs::write(&path, &whole).unwrap();
    let running = start(&dir).unwrap();
    assert_eq!(ptz_core::client::Api::new(&running.url(), "a", None).project().unwrap(), saved);
    drop(running);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_project_file_with_limits_that_cannot_be_used_stops_the_server_from_starting() {
    let t = TestServer::start("startup-limits");
    let dir = t.stop_keeping_data();
    // A hand edit that swaps the two ends of the pan range.
    let path = dir.join("project.json");
    let mut project: serde_json::Value = read_json(&path);
    project["cameras"][0]["axes"]["pan"]["min"] = 170.0.into();
    project["cameras"][0]["axes"]["pan"]["max"] = (-170.0).into();
    std::fs::write(&path, serde_json::to_vec_pretty(&project).unwrap()).unwrap();

    let why = start(&dir).err().expect("the server does not start");
    assert!(why.contains("project.json") && why.contains("Pan limits are not usable"), "{why}");
    let _ = std::fs::remove_dir_all(&dir);
}
