//! Crash safety: kill the server binary in the middle of a write loop, restart,
//! and every file still parses.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::Api;
use ptz_core::edit;
use ptz_core::song::Song;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn launch(dir: &std::path::Path) -> (Child, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ptz-server"))
        .args(["--data", dir.to_str().unwrap(), "--bind", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("ptz-server binary");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let url = line.split_whitespace().last().expect("listening line").to_string();
    assert!(url.starts_with("http://127.0.0.1:"), "unexpected first line: {line}");
    (child, url)
}

#[test]
fn killing_the_server_mid_write_never_leaves_a_broken_song() {
    let dir = temp_dir("crash");
    for round in 0..3 {
        let (mut child, url) = launch(&dir);
        let api = Api::new(&url, "writer", None);
        api.take_lease("writer", true).unwrap();
        let project = api.project().unwrap();
        let mut w = api.working("walk-in").unwrap();

        let writer = std::thread::spawn(move || {
            let mut n = 0;
            loop {
                let frame = 700 + (n % 200);
                let Ok(song) = edit::add_marker(&w.song, &project, frame, &format!("m{n}")) else { break };
                match api.put_working(&PutWorking { expected_counter: w.edit_counter, song }) {
                    Ok(next) => w = next,
                    Err(_) => break,
                }
                if n % 3 == 0 && api.save("walk-in").is_err() {
                    break;
                }
                n += 1;
            }
            n
        });

        std::thread::sleep(Duration::from_millis(250 + 90 * round));
        child.kill().unwrap();
        child.wait().unwrap();
        let writes = writer.join().unwrap();
        assert!(writes > 5, "the write loop barely ran ({writes} writes)");

        // Every JSON file in the data folder parses, and no song is half-written.
        let mut stack = vec![dir.clone()];
        let mut checked = 0;
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "json") {
                    let _: serde_json::Value = read_json(&p);
                    checked += 1;
                }
            }
        }
        assert!(checked >= 8);
        let working: Song = read_json(&dir.join("songs/walk-in/working.json"));
        working.validate(ptz_core::FrameRate::F25).unwrap();
        let _: Song = read_json(&dir.join("songs/walk-in/draft.json"));
    }

    // The server restarts on the same folder and serves the song.
    let (mut child, url) = launch(&dir);
    let api = Api::new(&url, "reader", None);
    let deadline = Instant::now() + Duration::from_secs(5);
    let songs = loop {
        match api.songs() {
            Ok(s) => break s,
            Err(e) => assert!(Instant::now() < deadline, "{e}"),
        }
    };
    assert_eq!(songs.len(), 3);
    assert!(api.working("walk-in").unwrap().song.markers.len() > 2);
    child.kill().unwrap();
    child.wait().unwrap();

    // The run log survived three kills with its chain intact.
    let out =
        Command::new(env!("CARGO_BIN_EXE_ptz-server")).args(["--data", dir.to_str().unwrap(), "--verify-log"]).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let _ = std::fs::remove_dir_all(&dir);
}
