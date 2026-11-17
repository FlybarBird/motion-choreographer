//! The server over real HTTP: songs on disk, edit counters, the lease, E-Stop.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::{Api, ApiErr};
use ptz_core::edit;
use ptz_core::pose::Preset;
use ptz_core::song::{ClipBody, Song};
use ptz_core::transport::{Action, Output, Run, TransportState};
use std::time::Duration;

fn refused(result: Result<impl std::fmt::Debug, ApiErr>) -> String {
    match result {
        Err(ApiErr::Refused { code: 409, msg }) => msg,
        other => panic!("expected a 409 refusal, got {other:?}"),
    }
}

#[test]
fn round_trip_new_edit_save_commit_load_with_files_checked() {
    let t = TestServer::start("roundtrip");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();

    let project = api.project().unwrap();
    let presets = api.presets().unwrap();
    assert_eq!(presets.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Stage Wide", "Lectern MCU", "Awards Two-Shot"]);

    let created = api.new_song("Finale", Some("03:00:00:00")).unwrap();
    assert_eq!(created.song.id, "finale");
    assert!(!created.dirty);
    let dir = t.dir.join("songs/finale");
    assert!(dir.join("working.json").exists() && dir.join("draft.json").exists());

    // Edit: drop a preset at two seconds.
    let (song, clip) = edit::insert_preset_move(&created.song, &project, &presets[1], 50).unwrap();
    let after = api.put_working(&PutWorking { expected_counter: created.edit_counter, song: song.clone() }).unwrap();
    assert_eq!(after.edit_counter, created.edit_counter + 1);
    assert!(after.dirty);
    assert!(after.issues.is_empty());
    let on_disk: Song = read_json(&dir.join("working.json"));
    assert_eq!(on_disk, song);
    assert!(on_disk.motion_clip(&clip).is_some());
    let draft: Song = read_json(&dir.join("draft.json"));
    assert!(draft.motion.is_empty(), "the draft only changes on Save");

    // Commit needs a saved draft.
    assert_eq!(refused(api.commit("finale")), "Save the song before committing");
    assert!(!api.save("finale").unwrap().dirty);
    assert_eq!(read_json::<Song>(&dir.join("draft.json")), song);
    let rev = api.commit("finale").unwrap();
    assert_eq!(rev.number, 1);
    let bytes = std::fs::read(dir.join("rev/0001.json")).unwrap();
    assert_eq!(ptz_server::storage::sha256_hex(&bytes), rev.sha256);
    assert_eq!(serde_json::from_slice::<Song>(&bytes).unwrap(), song);
    assert!(refused(api.commit("finale")).contains("Revision 1 already holds this song"));

    // Load it and check status reflects it.
    api.load_song("finale").unwrap();
    let st = api.status().unwrap();
    let s = st.song.unwrap();
    assert_eq!((s.id.as_str(), s.revision, s.dirty, s.committed_clean), ("finale", Some(1), false, true));
    assert_eq!(st.timecode, "03:00:00:00");
    assert_eq!(st.rate.label(), "25");

    // A second edit makes it dirty and uncommitted; Revert restores the draft.
    let (song2, _) = edit::insert_preset_move(&song, &project, &presets[2], 400).unwrap();
    let w = api.put_working(&PutWorking { expected_counter: after.edit_counter, song: song2 }).unwrap();
    assert!(w.dirty);
    assert_eq!(api.status().unwrap().guidance.reason(Action::Arm), Some("Commit the song first"));
    let reverted = api.revert("finale").unwrap();
    assert_eq!(reverted.song, song);
    assert!(!reverted.dirty);

    // Save As copies under a new id without the revisions.
    let copy = api.copy_song("finale", "Finale Alt").unwrap();
    assert_eq!(copy.song.id, "finale-alt");
    assert_eq!(copy.song.motion, song.motion);
    assert!(t.dir.join("songs/finale-alt/working.json").exists());
    assert!(api.get::<Vec<RevisionInfo>>("/songs/finale-alt/revisions").unwrap().is_empty());

    // A restart finds the same songs.
    let dir_keep = t.stop_keeping_data();
    let t2 = TestServer::reopen(&dir_keep);
    let api2 = t2.client("b");
    let ids: Vec<String> = api2.songs().unwrap().into_iter().map(|s| s.id).collect();
    assert!(ids.contains(&"finale".to_string()) && ids.contains(&"finale-alt".to_string()));
    assert_eq!(api2.working("finale").unwrap().song, song);
    assert_eq!(api2.working("finale").unwrap().revision, Some(1));
}

#[test]
fn stale_edit_counter_and_invalid_songs_are_refused() {
    let t = TestServer::start("stale");
    let a = t.client("a");
    a.take_lease("A", false).unwrap();
    let project = a.project().unwrap();
    let presets = a.presets().unwrap();
    let w = a.working("walk-in").unwrap();

    let (song, _) = edit::insert_preset_move(&w.song, &project, &presets[0], 900).unwrap();
    let ok = a.put_working(&PutWorking { expected_counter: w.edit_counter, song: song.clone() }).unwrap();
    // A second window still holding the old counter cannot overwrite it.
    let msg = refused(a.put_working(&PutWorking { expected_counter: w.edit_counter, song: w.song.clone() }));
    assert!(msg.contains("changed on the server"), "{msg}");
    assert_eq!(a.working("walk-in").unwrap().song, song);

    // Overlapping clips never reach disk.
    let mut bad = song.clone();
    let mut dup = bad.motion[0].clone();
    dup.id = "dup".into();
    bad.motion.push(dup);
    match a.put_working(&PutWorking { expected_counter: ok.edit_counter, song: bad }) {
        Err(ApiErr::Refused { code: 422, msg }) => assert!(msg.contains("Overlaps another move"), "{msg}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(a.working("walk-in").unwrap().edit_counter, ok.edit_counter);

    // Watchers cannot edit.
    let b = t.client("b");
    assert_eq!(
        refused(b.put_working(&PutWorking { expected_counter: ok.edit_counter, song })),
        "Take the edit lease to control the show"
    );
}

#[test]
fn lease_expiry_refuses_edits_and_arm_but_never_estop() {
    let t = TestServer::start_with("lease", |cfg| cfg.lease_ttl = Duration::from_millis(400));
    let a = t.client("a");
    let b = t.client("b");
    a.take_lease("Luke-MacBook", false).unwrap();

    // Someone else needs to confirm before taking over.
    assert_eq!(refused(b.take_lease("Booth", false)), "Luke-MacBook holds the edit lease. Take it over to edit");
    let st = b.status().unwrap();
    assert!(!st.you_hold_lease);
    assert_eq!(st.lease.as_ref().unwrap().holder, "Luke-MacBook");
    assert_eq!(st.guidance.enabled, vec![Action::Estop]);

    // Heartbeats keep it alive past the expiry time.
    for _ in 0..6 {
        std::thread::sleep(Duration::from_millis(120));
        assert!(a.status().unwrap().you_hold_lease);
    }
    a.act(Action::Arm, &TransportRequest::default()).unwrap();
    wait_for(&a, |s| s.transport.output == Output::Armed);
    a.act(Action::Play, &TransportRequest::default()).unwrap();

    // Silence: the lease lapses. The watcher asks, so the holder is not renewed.
    std::thread::sleep(Duration::from_millis(700));
    let st = b.status().unwrap();
    assert!(st.lease.is_none());
    assert_eq!(st.transport.run, Run::Playing, "a running show continues when the holder goes away");
    assert_eq!(st.transport.output, Output::Armed, "and stays armed");
    assert!(st.hardware_live);

    let w = a.working("walk-in").unwrap();
    assert_eq!(
        refused(a.put_working(&PutWorking { expected_counter: w.edit_counter, song: w.song })),
        "Take the edit lease to control the show"
    );
    assert_eq!(refused(a.act(Action::Stop, &TransportRequest::default())), "Take the edit lease to control the show");
    assert_eq!(refused(a.act(Action::Arm, &TransportRequest::default())), "Take the edit lease to control the show");

    // E-Stop is accepted from anyone, lease or not.
    let st = b.act(Action::Estop, &TransportRequest::default()).unwrap();
    assert!(st.transport.estop && !st.hardware_live);
    assert_eq!(st.transport.run, Run::Stopped);

    // The watcher takes the lease and resets.
    b.take_lease("Booth", false).unwrap();
    let st = b.act(Action::ResetEstop, &TransportRequest::default()).unwrap();
    assert_eq!(st.transport, TransportState::IDLE);
}

#[test]
fn rehearse_plays_in_real_time_and_stops_at_the_end() {
    let t = TestServer::start("play");
    let api = t.client("a");
    api.take_lease("A", false).unwrap();
    let st = api.status().unwrap();
    assert_eq!(st.guidance.primary_label, "Rehearse");
    assert_eq!(st.frame, 0);
    assert_eq!(st.timecode, "01:00:00:00");

    api.act(Action::Play, &TransportRequest::default()).unwrap();
    std::thread::sleep(Duration::from_millis(1000));
    let st = api.act(Action::Stop, &TransportRequest::default()).unwrap();
    assert!((23..=28).contains(&st.frame), "one second at 25 fps, got frame {}", st.frame);
    assert!(!st.hardware_live);
    let held = st.frame;
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(api.status().unwrap().frame, held, "Stop holds the playhead");

    // The monitor pose follows the timeline: by frame 400 the rig is on Lectern MCU.
    let st = api.act(Action::Seek, &TransportRequest { frame: Some(500), ..Default::default() }).unwrap();
    assert_eq!(st.frame, 500);
    assert_eq!(st.timecode, "01:00:20:00");
    assert!((st.first().pose.pan - -24.0).abs() < 1e-6, "{:?}", st.first().pose);
    assert_eq!(st.first().pose_source, PoseSource::Timeline);

    // Playing off the end stops at the last frame and leaves Play available.
    let len = st.song.unwrap().length_frames;
    api.act(Action::Seek, &TransportRequest { frame: Some(len - 5), ..Default::default() }).unwrap();
    api.act(Action::Play, &TransportRequest::default()).unwrap();
    let st = wait_for(&api, |s| s.transport.run == Run::Stopped);
    assert_eq!(st.frame, len);
    assert!(st.guidance.allows(Action::Play));
    let st = api.act(Action::Play, &TransportRequest::default()).unwrap();
    assert!(st.frame < 5, "Play at the end starts again from the top");
    let st = api.act(Action::Cue, &TransportRequest::default()).unwrap();
    assert_eq!((st.frame, st.transport), (0, TransportState::IDLE));

    let log = api.get::<Vec<LogRecord>>("/log?since=0").unwrap();
    assert!(log.iter().any(|r| r.kind == "transport" && r.data["action"] == "play"));
}

#[test]
fn access_key_is_required_when_set_and_remote_bind_needs_one() {
    let err = ptz_server::http::start(&ptz_server::engine::Config::new(temp_dir("nokey")), "0.0.0.0:0", None).err().unwrap();
    assert!(err.contains("--access-key"), "{err}");

    let t = TestServer::start_keyed("key", "s3cret");
    let no_key = Api::new(&t.url, "a", None);
    assert!(matches!(no_key.status(), Err(ApiErr::Refused { code: 401, .. })));
    let wrong = Api::new(&t.url, "a", Some("nope"));
    assert!(matches!(wrong.status(), Err(ApiErr::Refused { code: 401, .. })));
    let right = Api::new(&t.url, "a", Some("s3cret"));
    assert!(right.status().is_ok());
}

#[test]
fn websocket_streams_status_and_edit_events() {
    use std::io::{Read, Write};
    let t = TestServer::start("ws");
    let api = t.client("ws-client");
    api.take_lease("A", false).unwrap();

    // A minimal WebSocket client: handshake, then read unmasked server text frames.
    let addr = t.url.trim_start_matches("http://").to_string();
    let mut sock = std::net::TcpStream::connect(&addr).unwrap();
    sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        sock,
        "GET /api/v1/ws?client=ws-client HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        sock.read_exact(&mut byte).unwrap();
        head.push(byte[0]);
    }
    assert!(String::from_utf8_lossy(&head).starts_with("HTTP/1.1 101"));

    let mut read_msg = || -> WsMsg {
        let mut h = [0u8; 2];
        sock.read_exact(&mut h).unwrap();
        let mut len = (h[1] & 0x7f) as usize;
        if len == 126 {
            let mut b = [0u8; 2];
            sock.read_exact(&mut b).unwrap();
            len = u16::from_be_bytes(b) as usize;
        } else if len == 127 {
            let mut b = [0u8; 8];
            sock.read_exact(&mut b).unwrap();
            len = u64::from_be_bytes(b) as usize;
        }
        let mut payload = vec![0u8; len];
        sock.read_exact(&mut payload).unwrap();
        serde_json::from_slice(&payload).unwrap()
    };

    match read_msg() {
        WsMsg::Status(s) => assert!(s.you_hold_lease && s.song.is_some()),
        other => panic!("first message should be status, got {other:?}"),
    }

    // An edit made over HTTP is announced on the socket.
    let w = api.working("walk-in").unwrap();
    let project = api.project().unwrap();
    let song = edit::add_marker(&w.song, &project, 700, "Bow").unwrap();
    api.put_working(&PutWorking { expected_counter: w.edit_counter, song }).unwrap();
    let mut saw_change = false;
    for _ in 0..40 {
        if let WsMsg::SongChanged { song_id, edit_counter, .. } = read_msg() {
            assert_eq!((song_id.as_str(), edit_counter), ("walk-in", w.edit_counter + 1));
            saw_change = true;
            break;
        }
    }
    assert!(saw_change);

    // While playing, status arrives at about 30 Hz with a moving playhead.
    api.act(Action::Play, &TransportRequest::default()).unwrap();
    let started = std::time::Instant::now();
    let mut frames = Vec::new();
    while started.elapsed() < Duration::from_millis(1000) {
        if let WsMsg::Status(s) = read_msg() {
            if s.transport.run == Run::Playing {
                frames.push(s.frame_f);
            }
        }
    }
    assert!(frames.len() >= 20, "expected about 30 status messages in a second, got {}", frames.len());
    assert!(frames.windows(2).all(|w| w[1] >= w[0]));
    assert!(frames.last().unwrap() - frames.first().unwrap() > 15.0);
}

#[test]
fn save_writes_every_song_with_unsaved_changes_whichever_is_loaded() {
    let t = TestServer::start("save-all");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let project = api.project().unwrap();
    let unsaved = |api: &Api| -> Vec<String> { api.songs().unwrap().into_iter().filter(|s| s.dirty).map(|s| s.name).collect() };
    assert_eq!(api.save_all().unwrap().text(), "Nothing to save");

    // Two songs are edited and left unsaved; the second is the one that is loaded.
    let mut edited = Vec::new();
    for name in ["Opener", "Closer"] {
        let made = api.new_song(name, None).unwrap();
        api.load_song(&made.song.id).unwrap();
        let song = edit::set_song_length(&made.song, &project, 900).unwrap();
        let counter = api.working(&song.id).unwrap().edit_counter;
        assert!(api.put_working(&PutWorking { expected_counter: counter, song: song.clone() }).unwrap().dirty);
        edited.push(song);
    }
    assert_eq!(unsaved(&api), ["Closer", "Opener"]);

    let saved = api.save_all().unwrap();
    assert_eq!(saved.songs, ["Closer", "Opener"]);
    assert_eq!(saved.text(), "Saved 2 songs");
    let loaded = saved.working.expect("the loaded song comes back");
    assert_eq!((loaded.song.name.as_str(), loaded.dirty), ("Closer", false));
    assert!(unsaved(&api).is_empty());
    for song in &edited {
        let draft: Song = read_json(&t.dir.join("songs").join(&song.id).join("draft.json"));
        assert_eq!(draft, *song, "{} is on disk as it was edited", song.name);
    }
    assert_eq!(api.save_all().unwrap().text(), "Nothing to save");

    // Without the lease nothing is saved.
    let other = t.client("b");
    assert!(matches!(other.save_all(), Err(ApiErr::Refused { .. })));
}

#[test]
fn updating_a_preset_updates_every_timeline_move_made_from_it() {
    let t = TestServer::start("preset-link");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let project = api.project().unwrap();
    let presets = api.presets().unwrap();
    let created = api.new_song("Show", None).unwrap();
    let (song, clip) = edit::insert_preset_move(&created.song, &project, &presets[1], 40).unwrap();
    api.put_working(&PutWorking { expected_counter: created.edit_counter, song }).unwrap();

    let mut pose = presets[1].pose;
    pose.pan += 12.0;
    let updated: Preset =
        api.put(&format!("/presets/{}", presets[1].id), &UpdatePreset { pose: Some(pose), elevation: None }).unwrap();
    let working = api.working(&created.song.id).unwrap();
    match &working.song.motion_clip(&clip).unwrap().body {
        ClipBody::PresetMove { to, .. } => assert_eq!(to.pose, updated.pose),
        other => panic!("{other:?}"),
    }
    // A song that is not loaded follows too.
    let other = api.new_song("Other", None).unwrap();
    let (song, clip) = edit::insert_preset_move(&other.song, &project, &updated, 10).unwrap();
    api.put_working(&PutWorking { expected_counter: other.edit_counter, song }).unwrap();
    pose.tilt = -4.0;
    api.put::<_, Preset>(&format!("/presets/{}", updated.id), &UpdatePreset { pose: Some(pose), elevation: None }).unwrap();
    let working = api.working(&other.song.id).unwrap();
    match &working.song.motion_clip(&clip).unwrap().body {
        ClipBody::PresetMove { to, .. } => assert!((to.pose.tilt + 4.0).abs() < 1e-9 && (to.pose.pan - pose.pan).abs() < 1e-9),
        other => panic!("{other:?}"),
    }
}
