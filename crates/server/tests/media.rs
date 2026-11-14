//! Media over real HTTP: import, waveform levels, playback with the sample
//! counter as the clock, routing, and refusals that leave nothing behind.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::{Api, ApiErr};
use ptz_core::edit;
use ptz_core::pose::{ChannelRoute, Routing};
use ptz_core::song::Track;
use ptz_core::transport::{Action, Run};
use ptz_server::media::{click_track, wav};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn refusal(result: Result<impl std::fmt::Debug, ApiErr>, code: u16) -> String {
    match result {
        Err(ApiErr::Refused { code: got, msg }) if got == code => msg,
        other => panic!("expected a {code} refusal, got {other:?}"),
    }
}

fn fixture(t: &TestServer, name: &str, bytes: &[u8]) -> PathBuf {
    let dir = t.dir.join("fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn upload(api: &Api, t: &TestServer, name: &str, bytes: &[u8]) -> Result<MediaInfo, ApiErr> {
    let path = fixture(t, name, bytes);
    api.upload_media(name, std::fs::File::open(path).unwrap())
}

fn click_wav() -> Vec<u8> {
    wav::bytes(48_000, 1, &click_track(48_000, 12.4, 2.5), None)
}

#[test]
fn the_seeded_click_plays_at_two_and_a_half_seconds_and_status_agrees() {
    let t = TestServer::start("click");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();

    let library = api.media().unwrap();
    assert_eq!(library.len(), 2);
    let click = library.iter().find(|m| m.id == "click-12-4s").unwrap();
    assert_eq!((click.name.as_str(), click.channels, click.sample_rate, click.samples), ("Click 12.4s.wav", 1, 48_000, 595_200));
    assert_eq!(click.duration_frames, 310);
    assert!((click.duration_s - 12.4).abs() < 1e-9);

    let st = api.status().unwrap();
    assert_eq!(st.song.as_ref().unwrap().id, "walk-in");
    assert_eq!(st.audio.output, None);
    assert!(st.audio.silent_reason.as_deref().unwrap().contains("Running silent"), "{:?}", st.audio.silent_reason);
    assert_eq!(st.audio.device_rate, 48_000);
    assert_eq!(st.audio.routing, Routing::default());

    // Start just before the click and watch the meters.
    api.act(Action::Seek, &TransportRequest { frame: Some(40), ..Default::default() }).unwrap();
    let callbacks_before = api.status().unwrap().audio.callbacks;
    api.act(Action::Play, &TransportRequest::default()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut heard_at = None;
    let mut last = 40.0;
    while Instant::now() < deadline {
        let st = api.status().unwrap();
        assert!(st.frame_f + 1e-9 >= last, "the playhead never runs backwards: {} then {}", last, st.frame_f);
        last = st.frame_f;
        // The playhead follows the device clock with the callback jitter taken out: 0.05 frames is 2 ms.
        assert!(
            st.audio.drift_frames.abs() < 0.05,
            "free-run: the playhead is {} frames off the audio clock",
            st.audio.drift_frames
        );
        assert_eq!(st.audio.peak_right, 0.0, "Right is routed Silent");
        if st.audio.peak_left > 0.3 {
            heard_at = Some(st.frame_f);
            break;
        }
        assert!(st.frame_f < 80.0, "no click by frame {}", st.frame_f);
        std::thread::sleep(Duration::from_millis(4));
    }
    let heard_at = heard_at.expect("the click reached the left meter");
    // The click is at 2.5 s = frame 62.5. The playhead is read at 60 Hz, so it may trail the
    // renderer by up to half a frame, and polling adds a few milliseconds the other way.
    assert!((61.8..68.5).contains(&heard_at), "click heard with the playhead at frame {heard_at}");
    let st = api.status().unwrap();
    assert!(st.audio.callbacks > callbacks_before + 20, "callbacks rise: {} then {}", callbacks_before, st.audio.callbacks);
    assert!((st.audio.position_frame - st.frame_f).abs() < 1.5);

    // Swap the routing while playing: Program moves to the right.
    api.act(Action::Stop, &TransportRequest::default()).unwrap();
    // Let the left meter fall back before looking at the other side.
    wait_for(&api, |s| s.audio.peak_left < 0.01);
    let mut project = api.project().unwrap();
    project.routing = Routing { left: ChannelRoute::Silent, right: ChannelRoute::Program };
    api.put_project(&project).unwrap();
    api.act(Action::Seek, &TransportRequest { frame: Some(55), ..Default::default() }).unwrap();
    api.act(Action::Play, &TransportRequest::default()).unwrap();
    let st = wait_for(&api, |s| s.audio.peak_right > 0.3 || s.frame > 80);
    assert!(st.audio.peak_right > 0.3 && st.audio.peak_left < 0.05, "{:?}", st.audio);
    assert_eq!(st.audio.routing.right, ChannelRoute::Program);
}

#[test]
fn import_place_trim_and_delete() {
    let t = TestServer::start("import");
    let api = t.client("a");
    let watcher = t.client("b");

    // Importing needs the lease, and the refusal arrives intact.
    assert_eq!(refusal(upload(&watcher, &t, "Bed.wav", &click_wav()), 409), "Take the edit lease to control the show");
    api.take_lease("Luke-MacBook", false).unwrap();

    // A stereo 44.1 kHz file, resampled once for the 48 kHz renderer.
    let stereo: Vec<f32> = (0..44_100 * 3)
        .flat_map(|i| {
            let on = (44_100..44_100 + 2_000).contains(&i);
            let v = if on { 0.8 * ((i as f32) * 0.2).sin() } else { 0.0 };
            [v, v]
        })
        .collect();
    let bed = upload(&api, &t, "Awards Bed (final).wav", &wav::bytes(44_100, 2, &stereo, None)).unwrap();
    assert_eq!(bed.id, "awards-bed-final");
    assert_eq!((bed.channels, bed.sample_rate, bed.samples, bed.duration_frames), (2, 44_100, 132_300, 75));
    assert_eq!(bed.kind, MediaKind::Audio);
    let dir = t.dir.join("media/awards-bed-final");
    for file in ["original.wav", "audio.f32", "peaks.bin", "info.json"] {
        assert!(dir.join(file).exists(), "{file}");
    }
    assert_eq!(std::fs::read(dir.join("original.wav")).unwrap(), wav::bytes(44_100, 2, &stereo, None), "the original is kept");
    assert!(std::fs::read_dir(t.dir.join("media"))
        .unwrap()
        .flatten()
        .all(|e| !e.file_name().to_string_lossy().contains("upload")));
    // The same name again gets its own id.
    assert_eq!(upload(&api, &t, "Awards Bed (final).wav", &click_wav()).unwrap().id, "awards-bed-final-2");
    assert_eq!(api.media().unwrap().len(), 4);

    // Waveform: zoomed in gets the finest level, zoomed out a coarser one, and the burst is at one second in both.
    let fine = api.peaks(&bed.id, 100).unwrap();
    let coarse = api.peaks(&bed.id, 1000).unwrap();
    assert_eq!((fine.samples_per_bucket, coarse.samples_per_bucket), (256, 512));
    assert_eq!(fine.sample_rate, 44_100);
    for level in [&fine, &coarse] {
        let loud: Vec<usize> = (0..level.pairs.len()).filter(|i| level.pairs[*i].1 > 60).collect();
        let (first, last) =
            (loud[0] as f64 * level.bucket_seconds(), (*loud.last().unwrap() + 1) as f64 * level.bucket_seconds());
        assert!((first - 1.0).abs() < 0.02 && (last - 1.045).abs() < 0.02, "burst from {first} to {last}");
    }
    assert_eq!(refusal(api.peaks("nope", 256), 404), "No media with id nope");

    // Place it on a new song at frame 100, trimmed to start half a second in.
    let project = api.project().unwrap();
    let song = api.new_song("Bed Test", None).unwrap();
    api.load_song("bed-test").unwrap();
    let (placed, clip) =
        edit::add_media_clip(&song.song, &project, &bed.id, &bed.name, Track::Audio, bed.duration_frames, 100).unwrap();
    let trimmed = edit::trim_clip_start(&placed, &project, &clip, 112).unwrap();
    let c = trimmed.media_clip(&clip).unwrap();
    assert_eq!((c.start_frame, c.in_frame, c.out_frame), (112, 12, 75), "trimming the head keeps the sound where it was");
    api.put_working(&PutWorking { expected_counter: song.edit_counter, song: trimmed }).unwrap();

    // The burst is one second into the file: timeline frame 100 + 25.
    api.act(Action::Seek, &TransportRequest { frame: Some(105), ..Default::default() }).unwrap();
    api.act(Action::Play, &TransportRequest::default()).unwrap();
    let st = wait_for(&api, |s| s.audio.peak_left > 0.3 || s.frame > 140);
    assert!((124.3..131.0).contains(&st.frame_f), "burst heard at frame {}", st.frame_f);
    api.act(Action::Stop, &TransportRequest::default()).unwrap();
    assert_eq!(api.status().unwrap().transport.run, Run::Stopped);

    // In use: cannot be deleted until its clip is gone.
    assert_eq!(refusal(api.delete_media(&bed.id), 409), "Bed Test uses this file. Remove it from the timeline first");
    let now = api.working("bed-test").unwrap();
    let cleared = edit::delete(&now.song, &project, &[clip]).unwrap();
    api.put_working(&PutWorking { expected_counter: now.edit_counter, song: cleared }).unwrap();
    let left = api.delete_media(&bed.id).unwrap();
    assert_eq!(left.len(), 3);
    assert!(!dir.exists());

    // The library survives a restart.
    let dir = t.stop_keeping_data();
    let t = TestServer::reopen(&dir);
    let ids: Vec<String> = t.client("a").media().unwrap().into_iter().map(|m| m.id).collect();
    assert_eq!(ids, ["awards-bed-final-2", "awards-bed-walk-on", "click-12-4s"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn files_that_cannot_be_used_are_refused_and_nothing_is_added() {
    let t = TestServer::start("refuse");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let before = api.media().unwrap();

    let msg = refusal(upload(&api, &t, "notes.wav", b"these are notes, not audio"), 422);
    assert!(msg.starts_with("notes.wav was not imported: not a WAV, AIFF or MP3 file"), "{msg}");
    let msg = refusal(upload(&api, &t, "running order.pdf", b"%PDF-1.4"), 422);
    assert_eq!(msg, "running order.pdf was not imported: only WAV, AIFF and MP3 audio and QuickTime or MP4 video can be used");
    let msg = refusal(upload(&api, &t, "empty.wav", b""), 422);
    assert_eq!(msg, "empty.wav was not imported: the file is empty");
    // A WAV header with no samples.
    let msg = refusal(upload(&api, &t, "silent.wav", &wav::bytes(48_000, 1, &[], None)), 422);
    assert!(msg.starts_with("silent.wav was not imported:"), "{msg}");

    assert_eq!(api.media().unwrap(), before);
    let mut folders: Vec<String> =
        std::fs::read_dir(t.dir.join("media")).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).collect();
    folders.sort();
    assert_eq!(folders, ["awards-bed-walk-on", "click-12-4s"], "no folder or temporary file is left");
    // The name is free again after a refusal.
    assert_eq!(upload(&api, &t, "notes.wav", &click_wav()).unwrap().id, "notes");
}
