//! File timecode over real HTTP: where a stamp comes from, where a stamped
//! file lands, Lock to a file, and the proxy frames of a video.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::chase::ChaseState;
use ptz_core::client::Api;
use ptz_core::edit;
use ptz_core::eval::pose_at_f;
use ptz_core::pose::Axis;
use ptz_core::song::{Song, Track};
use ptz_core::time::{frame_to_sample, FrameRate, Timecode};
use ptz_core::transport::{Action, Run, TimeMode};
use ptz_server::media::{click_track, wav};
use std::time::{Duration, Instant};

/// A broadcast WAV whose time reference says it starts at `tc`.
fn stamped_wav(tc: &str, rate: FrameRate, seconds: f64) -> Vec<u8> {
    let start = Timecode::parse_at(tc, rate).unwrap().to_frames(rate);
    let reference = frame_to_sample(start, 48_000, rate) as u64;
    wav::bytes(48_000, 1, &click_track(48_000, seconds, 1.0), Some(reference))
}

fn stamp(api: &Api, id: &str, text: &str) -> Result<MediaInfo, ptz_core::client::ApiErr> {
    api.post(&format!("/media/{id}/start_tc"), &StoreTc { start_tc: text.into() })
}

fn have_ffmpeg() -> bool {
    let ok = ptz_server::video::have_tools();
    if !ok {
        eprintln!("skipped: ffmpeg and ffprobe are needed to make and read the video fixture");
    }
    ok
}

#[test]
fn a_broadcast_wav_reports_its_stamp_at_25_and_at_29_97_drop_frame() {
    let t = TestServer::start("stamp-bext");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();

    let info = import(&api, &t, "Stamped 25.wav", &stamped_wav("01:00:10:00", FrameRate::F25, 2.0)).unwrap();
    assert_eq!((info.start_tc.as_deref(), info.start_tc_source.as_deref()), (Some("01:00:10:00"), Some("bext")));
    assert_eq!(info.start_samples, Some(173_280_000));
    // A plain WAV has no stamp.
    let plain = import(&api, &t, "Plain.wav", &wav::bytes(48_000, 1, &click_track(48_000, 1.0, 0.5), None)).unwrap();
    assert_eq!((plain.start_tc, plain.start_tc_source), (None, None));

    let mut project = api.project().unwrap();
    project.rate = FrameRate::F2997Df;
    api.put_project(&project).unwrap();
    let info = import(&api, &t, "Stamped DF.wav", &stamped_wav("01:00:10;00", FrameRate::F2997Df, 2.0)).unwrap();
    assert_eq!((info.start_tc.as_deref(), info.start_tc_source.as_deref()), (Some("01:00:10;00"), Some("bext")));
    // The file imported at 25 says the same time of day in the new rate's numbering.
    let library = api.media().unwrap();
    let earlier = library.iter().find(|m| m.name == "Stamped 25.wav").unwrap();
    assert_eq!(earlier.start_tc.as_deref(), Some("01:00:10;00"));

    // The stamp survives a restart.
    let dir = t.stop_keeping_data();
    let t = TestServer::reopen(&dir);
    let again = t.client("a").media().unwrap();
    assert_eq!(again.iter().find(|m| m.name == "Stamped DF.wav").unwrap().start_tc.as_deref(), Some("01:00:10;00"));
}

#[test]
fn a_stamp_can_be_typed_or_read_from_an_ltc_channel() {
    let t = TestServer::start("stamp-ltc");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rate = FrameRate::F25;

    // Left: a tone. Right: 12 frames of nothing, then LTC from 01:00:10:00.
    let sr = 48_000u32;
    let code = Timecode::parse_at("01:00:10:00", rate).unwrap().to_frames(rate);
    let mut ltc = vec![0.0f32; frame_to_sample(12, sr, rate) as usize];
    ltc.extend(ptz_core::ltc::encode(code, 75, rate, sr, 0.5, 1.0));
    let samples: Vec<f32> = ltc
        .iter()
        .enumerate()
        .flat_map(|(i, c)| [(i as f32 * 2.0 * std::f32::consts::PI * 440.0 / sr as f32).sin() * 0.4, *c])
        .collect();
    let info = import(&api, &t, "Band and code.wav", &wav::bytes(sr, 2, &samples, None)).unwrap();
    assert_eq!((info.start_tc.clone(), info.ltc_channel), (None, None));

    assert!(refused(stamp(&t.client("watcher"), &info.id, "01:00:00:00"), 409).contains("Take the edit lease"));
    assert_eq!(refused(stamp(&api, &info.id, "ltc:1"), 422), "Band and code.wav: no LTC was found on channel 1");
    assert_eq!(
        refused(stamp(&api, &info.id, "ltc:3"), 422),
        "Band and code.wav: the file has 2 channels, so there is no channel 3"
    );
    assert!(refused(stamp(&api, &info.id, "ltc:left"), 400).contains("counting from 1"));

    // The first frame decoded is 01:00:10:00, twelve frames into the file, so the file starts at 01:00:09:13.
    let read = stamp(&api, &info.id, "ltc:2").unwrap();
    assert_eq!(read.start_tc.as_deref(), Some("01:00:09:13"));
    assert_eq!((read.start_tc_source.as_deref(), read.ltc_channel), (Some("ltc channel 2"), Some(1)));
    assert_eq!(api.media().unwrap().iter().find(|m| m.id == info.id), Some(&read));
    let on_disk: MediaInfo = read_json(&t.dir.join("media").join(&info.id).join("info.json"));
    assert_eq!(on_disk.start_tc, read.start_tc);

    // Typed over it; a wrong one is refused in words; empty takes it off again.
    let typed = stamp(&api, &info.id, "01:00:20:00").unwrap();
    assert_eq!((typed.start_tc.as_deref(), typed.start_tc_source.as_deref()), (Some("01:00:20:00"), Some("typed")));
    assert_eq!(refused(stamp(&api, &info.id, "01:00:20:25"), 422), "01:00:20:25 is not a timecode at 25 fps. Use HH:MM:SS:FF");
    let cleared = stamp(&api, &info.id, "").unwrap();
    assert_eq!((cleared.start_tc, cleared.start_tc_source, cleared.ltc_channel), (None, None, None));
    assert!(refused(stamp(&api, "nothing", "01:00:00:00"), 404).contains("No media with id nothing"));

    // LTC at another rate is named, not guessed at.
    let other = ptz_core::ltc::encode(108_000, 60, FrameRate::F30, sr, 0.5, 1.0);
    let info = import(&api, &t, "Thirty.wav", &wav::bytes(sr, 1, &other, None)).unwrap();
    assert_eq!(
        refused(stamp(&api, &info.id, "ltc:1"), 422),
        "Thirty.wav: channel 1 carries 30 fps LTC and the project is 25 fps"
    );
}

fn working(api: &Api, id: &str) -> Song {
    api.working(id).unwrap().song
}

fn put(api: &Api, song: Song) -> Song {
    let counter = api.working(&song.id).unwrap().edit_counter;
    api.put_working(&PutWorking { expected_counter: counter, song }).unwrap().song
}

fn lock_file(api: &Api, clip: &str) -> Result<Status, ptz_core::client::ApiErr> {
    api.act(Action::Lock, &TransportRequest { source: Some(format!("file:{clip}")), ..Default::default() })
}

fn lookup(points: &[(f64, f64)], frame: f64) -> Option<f64> {
    let i = points.partition_point(|p| p.0 <= frame);
    if i == 0 || i == points.len() {
        return None;
    }
    let (a, b) = (points[i - 1], points[i]);
    Some(a.1 + (b.1 - a.1) * (frame - a.0) / (b.0 - a.0).max(1e-9))
}

#[test]
fn a_stamped_file_lands_on_its_code_locks_the_playhead_and_a_recorded_pan_replays_to_the_same_code() {
    let t = TestServer::start("stamp-e2e");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let project = api.project().unwrap();
    let rate = project.rate;
    assert_eq!(api.status().unwrap().song.unwrap().id, "walk-in");

    // Drop the stamped file on Walk-In: it lands where its code is, 14 seconds into a song that starts on the hour.
    let info = import(&api, &t, "Stamped.wav", &stamped_wav("01:00:14:00", rate, 8.0)).unwrap();
    let song = working(&api, "walk-in");
    let lands = info.frame_in(&song, rate).unwrap();
    assert_eq!(lands, 350);
    let (song, clip) =
        edit::add_media_clip(&song, &project, &info.id, &info.name, Track::Audio, info.duration_frames, lands).unwrap();
    let song = put(&api, song);
    assert_eq!(song.clock(rate).timecode_at(song.media_clip(&clip).unwrap().start_frame).to_string(), "01:00:14:00");
    let (first, end) = (350, 550);

    // Lock to it. The file is its own code, so the lock is there at once and Cue still works.
    assert!(refused(lock_file(&api, "m99"), 409).contains("not on this song's timeline"));
    let st = lock_file(&api, &clip).unwrap();
    assert_eq!(st.transport.time, TimeMode::LockedFile);
    assert_eq!((st.lock.source.clone(), st.lock.chase), (Some(format!("file:{clip}")), ChaseState::Locked));
    assert_eq!(st.guidance.primary_label, "Play to code");
    assert_eq!(do_act(&api, Action::Cue).frame, 0);

    // Lock to code and record: Play to code rolls the file from its first frame, and a pan is jogged over it.
    api.act(Action::RecordOn, &TransportRequest { axes: Some(vec![Axis::Pan]), ..Default::default() }).unwrap();
    let st = do_act(&api, Action::Play);
    assert_eq!(st.transport.run, Run::Playing, "the server rolls the file itself, so there is nothing to wait for");
    assert!((first as f64..first as f64 + 3.0).contains(&st.frame_f), "playback began at the file, not at {}", st.frame_f);
    let jog = Jog { pan: 8.0 / project.axes.get(Axis::Pan).max_speed, ..Jog::default() };
    let mut recorded: Vec<(f64, f64)> = Vec::new();
    let mut codes: Vec<(f64, String)> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let st = api.status().unwrap();
        assert!(Instant::now() < deadline, "the pass never reached frame 470: {st:?}");
        assert_eq!((st.transport.run, st.transport.time), (Run::Playing, TimeMode::LockedFile));
        // The code the lock reports is the timeline's own timecode: the file sits on its stamp.
        assert_eq!(st.lock.last_code.as_deref(), Some(st.timecode.as_str()));
        assert!(st.audio.drift_frames.abs() < 0.05, "the file is the clock: {} frames of drift", st.audio.drift_frames);
        if recorded.last().is_none_or(|l| st.frame_f > l.0) {
            recorded.push((st.frame_f, st.rig_pose.pan));
            codes.push((st.frame_f, st.timecode.clone()));
        }
        if st.frame_f >= 470.0 {
            break;
        }
        let held = (380.0..440.0).contains(&st.frame_f);
        api.jog(&if held { jog } else { Jog::default() }).unwrap();
        std::thread::sleep(Duration::from_millis(8));
    }
    do_act(&api, Action::Stop);
    let song = working(&api, "walk-in");
    let pass = song.motion.iter().find(|c| c.recorded_pass.is_some()).expect("the pass is on the timeline");
    assert_eq!(pass.start_frame, first, "the pass begins on the file's first frame");
    let moved = pose_at_f(&song, 460.0).pan - pose_at_f(&song, 370.0).pan;
    assert!(moved.abs() > 10.0, "the pan was recorded: it travels {moved} degrees");

    // Cue, Play to code: the pan replays on the same timecodes.
    assert_eq!(do_act(&api, Action::Cue).frame, 0);
    let st = do_act(&api, Action::Play);
    assert_eq!(st.transport.run, Run::Playing);
    let mut compared = 0;
    loop {
        let st = api.status().unwrap();
        if st.frame_f >= 465.0 || st.transport.run != Run::Playing {
            break;
        }
        assert_eq!(st.pose_source, PoseSource::Timeline);
        if let Some(was) = lookup(&recorded, st.frame_f) {
            assert!(
                (st.pose.pan - was).abs() < 0.3,
                "at {} the pan was recorded at {was} and replays at {}",
                st.timecode,
                st.pose.pan
            );
            // The same frame carries the same code as it did during the pass.
            let then = codes.iter().find(|(f, _)| f.floor() == st.frame_f.floor()).map(|(_, tc)| tc.clone());
            if let Some(then) = then {
                assert_eq!(then, st.timecode);
            }
            compared += 1;
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    assert!(compared > 100, "{compared} frames compared");

    // The file's code ends where the file does, and so does the show.
    let st = wait_for(&api, |s| s.transport.run == Run::Stopped);
    assert_eq!((st.frame, st.transport.time), (end, TimeMode::LockedFile));
    assert_eq!(st.timecode, "01:00:22:00");

    // Play from inside the file starts there; from past its end, at its first frame.
    api.act(Action::Seek, &TransportRequest { frame: Some(500), ..Default::default() }).unwrap();
    let st = do_act(&api, Action::Play);
    assert!((500.0..503.0).contains(&st.frame_f), "{}", st.frame_f);
    do_act(&api, Action::Stop);
    api.act(Action::Seek, &TransportRequest { frame: Some(900), ..Default::default() }).unwrap();
    let st = do_act(&api, Action::Play);
    assert!((350.0..353.0).contains(&st.frame_f), "{}", st.frame_f);
    do_act(&api, Action::Stop);

    // Moved off its code, the file no longer says what the timeline says: the lock lets go and says why.
    let song = working(&api, "walk-in");
    let moved = edit::move_clip(&song, &project, &clip, 600).unwrap();
    put(&api, moved);
    let st = api.status().unwrap();
    assert_eq!(st.transport.time, TimeMode::FreeRun);
    let why = "Stamped.wav starts at 01:00:14:00 but sits at 01:00:24:00 on the timeline. Move it to its timecode to lock to it";
    assert_eq!(st.lock.problem.as_deref(), Some(format!("Unlocked. {why}").as_str()));
    assert_eq!(refused(lock_file(&api, &clip), 409), why);

    // With the stamp taken off (typed over, then cleared is the file's own again) it locks by position.
    stamp(&api, &info.id, "01:00:24:00").unwrap();
    let st = lock_file(&api, &clip).unwrap();
    assert_eq!(st.transport.time, TimeMode::LockedFile);
    // Retyping the stamp while locked lets go for the same reason.
    stamp(&api, &info.id, "01:00:30:00").unwrap();
    assert_eq!(api.status().unwrap().transport.time, TimeMode::FreeRun);

    // A file with no stamp locks from its first sample at the clip's position.
    let click = working(&api, "walk-in").media.iter().find(|c| c.media_id == "click-12-4s").unwrap().id.clone();
    let st = lock_file(&api, &click).unwrap();
    assert_eq!(st.transport.time, TimeMode::LockedFile);
    let st = do_act(&api, Action::Play);
    assert!(st.frame_f < 3.0);
    do_act(&api, Action::Stop);
    assert_eq!(do_act(&api, Action::Unlock).transport.time, TimeMode::FreeRun);

    let log: Vec<LogRecord> = api.get("/log?since=0").unwrap();
    assert!(log.iter().any(|r| r.kind == "lock_released"));
    assert!(log.iter().any(|r| r.kind == "media_stamped"));
}

#[test]
fn a_quicktime_clip_reports_its_timecode_and_serves_a_proxy_for_every_frame() {
    if !have_ffmpeg() {
        return;
    }
    let t = TestServer::start("stamp-video");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();

    // Two seconds of test pattern with a tone and a timecode track.
    let dir = t.dir.join("fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    let clip = dir.join("Reference.mov");
    let made = ptz_server::video::tool("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y"])
        .args(["-f", "lavfi", "-i", "testsrc=size=640x360:rate=25:duration=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=2"])
        .args(["-timecode", "01:00:10:00", "-c:v", "mjpeg", "-q:v", "5", "-c:a", "pcm_s16le"])
        .arg(&clip)
        .status()
        .unwrap();
    assert!(made.success(), "ffmpeg could not make the fixture");

    let info = api.upload_media("Reference.mov", std::fs::File::open(&clip).unwrap()).unwrap();
    assert_eq!(info.kind, MediaKind::Video);
    assert_eq!((info.start_tc.as_deref(), info.start_tc_source.as_deref()), (Some("01:00:10:00"), Some("quicktime")));
    assert_eq!(info.duration_frames, 50);
    assert!((49..=51).contains(&info.proxy_frames), "{} proxy frames for 50 timeline frames", info.proxy_frames);
    assert_eq!(info.proxy_rate, Some(FrameRate::F25));

    // Frames are addressed by timeline frame; each is a JPEG 480 pixels wide.
    let frame = |n: i64| api.get_bytes(&format!("/media/{}/frames/{n}.jpg", info.id));
    let first = frame(0).unwrap();
    assert_eq!(&first[..3], &[0xFF, 0xD8, 0xFF]);
    let size = |jpeg: &[u8]| {
        // The frame header (SOF0) carries height then width.
        let at = jpeg.windows(2).position(|w| w == [0xFF, 0xC0]).expect("a baseline JPEG");
        (u16::from_be_bytes([jpeg[at + 7], jpeg[at + 8]]), u16::from_be_bytes([jpeg[at + 5], jpeg[at + 6]]))
    };
    assert_eq!(size(&first), (480, 270));
    let later = frame(40).unwrap();
    assert_ne!(first, later, "the test pattern moves, so frames differ");
    let last = frame(info.proxy_frames - 1).unwrap();
    assert_eq!(last, frame(5000).unwrap(), "past the end is the last frame");
    assert_eq!(first, frame(-3).unwrap(), "before the start is the first");
    assert!(refused(api.get_bytes(&format!("/media/{}/frames/first.jpg", info.id)), 404).contains("<frame number>.jpg"));
    assert!(refused(api.get_bytes("/media/click-12-4s/frames/0.jpg"), 404).contains("no picture"));

    // On the timeline it is a Reference video clip that can be locked to.
    let project = api.project().unwrap();
    let song_id = api.new_song("Video Test", Some("01:00:00:00")).unwrap().song.id;
    api.load_song(&song_id).unwrap();
    let song = working(&api, &song_id);
    let lands = info.frame_in(&song, project.rate).unwrap();
    assert_eq!(lands, 250);
    let (song, clip_id) =
        edit::add_media_clip(&song, &project, &info.id, &info.name, Track::Video, info.duration_frames, lands).unwrap();
    put(&api, song);
    assert_eq!(lock_file(&api, &clip_id).unwrap().transport.time, TimeMode::LockedFile);
    do_act(&api, Action::Unlock);

    // A typed stamp can be undone back to the file's own.
    assert_eq!(stamp(&api, &info.id, "02:00:00:00").unwrap().start_tc.as_deref(), Some("02:00:00:00"));
    let own = stamp(&api, &info.id, "").unwrap();
    assert_eq!((own.start_tc.as_deref(), own.start_tc_source.as_deref()), (Some("01:00:10:00"), Some("quicktime")));

    // The proxies go when the media goes.
    let folder = t.dir.join("media").join(&info.id);
    assert!(folder.join("frames").join("000000.jpg").exists());
    assert!(refused(api.delete_media(&info.id), 409).contains("Remove it from the timeline first"));
    api.delete_song(&song_id).unwrap();
    api.delete_media(&info.id).unwrap();
    assert!(!folder.exists(), "the media folder and its proxy frames are gone");

    // A picture with no sound still imports, with a silent track of its length.
    let mute = dir.join("Mute.mp4");
    let made = ptz_server::video::tool("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", "testsrc=size=320x180:rate=25:duration=1"])
        .args(["-c:v", "mjpeg", "-q:v", "5"])
        .arg(&mute)
        .status()
        .unwrap();
    assert!(made.success());
    let info = api.upload_media("Mute.mp4", std::fs::File::open(&mute).unwrap()).unwrap();
    assert_eq!((info.kind, info.duration_frames, info.start_tc.clone()), (MediaKind::Video, 25, None));
    assert!(info.proxy_frames >= 24);
}
