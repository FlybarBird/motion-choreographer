//! Lock to code over real HTTP. Art-Net arrives as real UDP packets; LTC and
//! MTC are scripted through the same readers the devices use. Every source is
//! paced in real time, the way a console or a tape would send it.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::chase::ChaseState;
use ptz_core::client::Api;
use ptz_core::codes::{artnet_build, mtc_full_frame, mtc_quarter_frames, CodeRate};
use ptz_core::time::{FrameRate, Timecode};
use ptz_core::transport::{Action, Run, TimeMode};
use ptz_server::lock::{Feed, Feeder, Kind, LtcReader, MtcReader};
use std::net::UdpSocket;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const RATE: FrameRate = FrameRate::F25;
const FPS: f64 = 25.0;

fn code(tc: &str) -> i64 {
    Timecode::parse_at(tc, RATE).unwrap().to_frames(RATE)
}

fn sleep_until(at: Instant) {
    let now = Instant::now();
    if at > now {
        std::thread::sleep(at - now);
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Desk {
    Silent,
    /// Code rolls: it was `code` at `since`.
    Roll {
        code: i64,
        since: Instant,
    },
    /// The same frame, sent over and over.
    Parked(i64),
    Quit,
}

/// Something on the network sending `ArtTimeCode`, one packet per frame.
struct ArtNetDesk {
    state: Arc<Mutex<Desk>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ArtNetDesk {
    fn start(to: std::net::SocketAddr, family: CodeRate) -> ArtNetDesk {
        let state = Arc::new(Mutex::new(Desk::Silent));
        let shared = state.clone();
        let fps = family.counting_rate().fps();
        let thread = std::thread::spawn(move || {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            let t0 = Instant::now();
            for tick in 0u64.. {
                sleep_until(t0 + Duration::from_secs_f64(tick as f64 / fps));
                let frame = match *shared.lock().unwrap() {
                    Desk::Quit => break,
                    Desk::Silent => continue,
                    Desk::Parked(code) => code,
                    Desk::Roll { code, since } => code + (since.elapsed().as_secs_f64() * fps).floor() as i64,
                };
                let tc = Timecode::from_frames(frame, family.counting_rate());
                let _ = socket.send_to(&artnet_build(tc, family), to);
            }
        });
        ArtNetDesk { state, thread: Some(thread) }
    }

    fn set(&self, desk: Desk) {
        *self.state.lock().unwrap() = desk;
    }

    fn roll_from(&self, tc: &str) {
        self.set(Desk::Roll { code: code(tc), since: Instant::now() });
    }

    /// Where the code is now, in frames since midnight, while it rolls.
    fn now(&self) -> f64 {
        match *self.state.lock().unwrap() {
            Desk::Roll { code, since } => code as f64 + since.elapsed().as_secs_f64() * FPS,
            other => panic!("the desk is not rolling: {other:?}"),
        }
    }
}

impl Drop for ArtNetDesk {
    fn drop(&mut self) {
        self.set(Desk::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A server with one empty song at 01:00:00:00, 60 seconds long, and the lease taken.
fn server_with_song(name: &str) -> (TestServer, Api, String) {
    let t = TestServer::start(name);
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let id = api.new_song("Lock Test", Some("01:00:00:00")).unwrap().song.id;
    api.load_song(&id).unwrap();
    (t, api, id)
}

/// Point the Art-Net input at a free port on this machine.
fn art_net_port(api: &Api) -> std::net::SocketAddr {
    let probe = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let mut project = api.project().unwrap();
    project.inputs.artnet_bind = addr.to_string();
    api.put_project(&project).unwrap();
    addr
}

fn lock_to(api: &Api, source: &str) -> Result<Status, ptz_core::client::ApiErr> {
    api.act(Action::Lock, &TransportRequest { source: Some(source.into()), ..Default::default() })
}

/// The playhead is where the code says, to within what a status round trip costs.
fn assert_follows(st: &Status, code_now: f64, what: &str) {
    let want = code_now - code("01:00:00:00") as f64;
    assert!((st.frame_f - want).abs() < 2.0, "{what}: the playhead is at {} and the code at {want}", st.frame_f);
}

#[test]
fn art_net_code_is_chased_through_dropouts_a_jump_a_stop_and_the_end_of_the_song() {
    let (t, api, _song) = server_with_song("lock-artnet");
    let addr = art_net_port(&api);

    assert!(refused(lock_to(&api, "tape"), 400).contains("Lock needs a source"));
    let st = lock_to(&api, "artnet").unwrap();
    assert_eq!(st.transport.time, TimeMode::LockedExternal);
    assert_eq!((st.lock.source.as_deref(), st.lock.chase), (Some("artnet"), ChaseState::NoCode));
    assert_eq!((st.lock.last_code.clone(), st.lock.frames_since_code, st.lock.problem.clone()), (None, None, None));
    assert_eq!(refused(api.act(Action::Cue, &TransportRequest::default()), 409), "Unlock to cue");
    assert_eq!(st.guidance.primary_label, "Play to code");

    // Play to code with nothing arriving: Play stays pressed and the playhead does not move.
    let st = do_act(&api, Action::Play);
    assert_eq!(st.transport.run, Run::WaitingForCode);
    std::thread::sleep(Duration::from_millis(200));
    let st = api.status().unwrap();
    assert_eq!((st.transport.run, st.frame), (Run::WaitingForCode, 0));

    // Code rolls from one second before the song: trusted after four frames, entered at the song start.
    let desk = ArtNetDesk::start(addr, CodeRate::F25);
    desk.roll_from("00:59:59:00");
    let st = wait_for(&api, |s| s.lock.chase == ChaseState::Locked);
    assert_eq!(st.transport.run, Run::WaitingForCode, "locked to code that is still before the song");
    assert_eq!(st.lock.announced_rate.as_deref(), Some("25"));
    assert!(st.lock.last_code.as_deref().unwrap().starts_with("00:59:59:"), "{:?}", st.lock.last_code);
    assert!(st.lock.frames_since_code.unwrap() < 3.0);
    let st = wait_for(&api, |s| s.transport.run == Run::Playing);
    assert!(st.frame_f < 4.0, "playback began as the code entered the song, not at frame {}", st.frame_f);
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(25));
        let st = api.status().unwrap();
        assert_eq!(st.transport.run, Run::Playing);
        assert_follows(&st, desk.now(), "rolling");
        // The sound was started on the code and is held within a frame of it.
        assert!(st.audio.drift_frames.abs() <= 1.0, "audio is {} frames off the code", st.audio.drift_frames);
        assert!((st.audio.position_frame - st.frame_f).abs() < 2.0);
    }

    // A dropout shorter than the freewheel window: the show carries on.
    let rolling = *desk.state.lock().unwrap();
    desk.set(Desk::Silent);
    let gap = Instant::now();
    let mut freewheeled = false;
    while gap.elapsed() < Duration::from_millis(200) {
        let st = api.status().unwrap();
        assert_eq!(st.transport.run, Run::Playing, "five missing frames do not stop the show");
        freewheeled |= st.lock.chase == ChaseState::Freewheel;
        std::thread::sleep(Duration::from_millis(10));
    }
    desk.set(rolling);
    assert!(freewheeled, "the chase reported Freewheel during the gap");
    let st = wait_for(&api, |s| s.lock.chase == ChaseState::Locked);
    assert_eq!(st.transport.run, Run::Playing);
    assert_follows(&api.status().unwrap(), desk.now(), "after the short dropout");

    // A dropout longer than the window: the playhead holds, Play stays pressed, code returning resumes.
    desk.set(Desk::Silent);
    let st = wait_for(&api, |s| s.transport.run == Run::WaitingForCode);
    assert_eq!(st.lock.chase, ChaseState::NoCode);
    let held = st.frame_f;
    std::thread::sleep(Duration::from_millis(150));
    let st = api.status().unwrap();
    assert_eq!(st.frame_f, held, "the playhead holds while there is no code");
    assert!(st.lock.frames_since_code.unwrap() > 8.0);
    assert_eq!(st.audio.drift_frames, 0.0);
    desk.set(rolling);
    wait_for(&api, |s| s.transport.run == Run::Playing);
    assert_follows(&api.status().unwrap(), desk.now(), "after the long dropout");

    // A jump: the playhead follows the code to its new place.
    desk.roll_from("01:00:30:00");
    wait_for(&api, |s| s.transport.run == Run::Playing && s.frame_f >= 750.0);
    assert_follows(&api.status().unwrap(), desk.now(), "after the jump");

    // The code stops (the desk repeats one frame): hold. It rolls again: resume.
    desk.set(Desk::Parked(code("01:00:40:00")));
    let st = wait_for(&api, |s| s.transport.run == Run::WaitingForCode);
    let held = st.frame_f;
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(api.status().unwrap().frame_f, held);
    desk.roll_from("01:00:40:00");
    wait_for(&api, |s| s.transport.run == Run::Playing && s.frame_f >= 1000.0);

    // The code leaves the song: hold again, still waiting.
    desk.roll_from("01:05:00:00");
    let st = wait_for(&api, |s| s.transport.run == Run::WaitingForCode && s.lock.chase == ChaseState::Locked);
    assert!(st.lock.last_code.as_deref().unwrap().starts_with("01:05:0"), "{:?}", st.lock.last_code);

    // Stop, then Unlock, leaves the state; Cue works again.
    let st = do_act(&api, Action::Stop);
    assert_eq!((st.transport.run, st.transport.time), (Run::Stopped, TimeMode::LockedExternal));
    let st = do_act(&api, Action::Unlock);
    assert_eq!((st.transport.time, st.lock.source.clone(), st.lock.chase), (TimeMode::FreeRun, None, ChaseState::NoCode));
    assert_eq!(do_act(&api, Action::Cue).frame, 0);

    let log: Vec<LogRecord> = api.get("/log?since=0").unwrap();
    let events: Vec<&str> = log.iter().filter(|r| r.kind == "event").filter_map(|r| r.data["event"].as_str()).collect();
    assert!(events.iter().filter(|e| **e == "CodeEntered").count() >= 4, "{events:?}");
    assert!(events.iter().filter(|e| **e == "CodeLost").count() >= 4, "{events:?}");
    drop(desk);
    drop(t);
}

#[test]
fn unlock_while_playing_carries_on_in_free_run() {
    let (_t, api, _song) = server_with_song("lock-unlock");
    let addr = art_net_port(&api);
    lock_to(&api, "artnet").unwrap();
    do_act(&api, Action::Play);
    let desk = ArtNetDesk::start(addr, CodeRate::F25);
    desk.roll_from("01:00:05:00");
    wait_for(&api, |s| s.transport.run == Run::Playing && s.frame_f > 135.0);
    let st = do_act(&api, Action::Unlock);
    assert_eq!((st.transport.run, st.transport.time), (Run::Playing, TimeMode::FreeRun));
    let at = st.frame_f;
    drop(desk);
    // No code any more, and the playhead keeps going on the server's own clock.
    let st = wait_for(&api, |s| s.frame_f > at + 20.0);
    assert_eq!(st.transport.run, Run::Playing);
    assert!(st.frame_f < at + 40.0, "it carried on from where it was, at {} then {}", at, st.frame_f);
}

#[test]
fn code_at_another_rate_is_refused_with_the_reason() {
    let (_t, api, _song) = server_with_song("lock-mismatch");
    let addr = art_net_port(&api);
    lock_to(&api, "artnet").unwrap();
    do_act(&api, Action::Play);
    let desk = ArtNetDesk::start(addr, CodeRate::F30);
    desk.set(Desk::Roll { code: Timecode::new(1, 0, 0, 0).to_frames(FrameRate::F30), since: Instant::now() });
    let st = wait_for(&api, |s| s.lock.problem.is_some());
    assert_eq!(
        st.lock.problem.as_deref(),
        Some("Lock refused: the Art-Net timecode carries 30 fps code and the project is 25 fps")
    );
    assert_eq!((st.transport.time, st.transport.run), (TimeMode::FreeRun, Run::Stopped));
    assert_eq!(st.lock.source, None);
    assert!(st.guidance.allows(Action::Lock));
    let log: Vec<LogRecord> = api.get("/log?since=0").unwrap();
    assert!(log.iter().any(|r| r.kind == "lock_released" && r.data["reason"].as_str().unwrap().contains("30 fps")));
    // The reason stays up until the operator does something.
    std::thread::sleep(Duration::from_millis(100));
    assert!(api.status().unwrap().lock.problem.is_some());
    assert_eq!(do_act(&api, Action::Cue).lock.problem, None);

    // 29.97 drop-frame is its own family; 30 non-drop does not pass for it.
    let mut project = api.project().unwrap();
    project.rate = FrameRate::F2997Df;
    api.put_project(&project).unwrap();
    lock_to(&api, "artnet").unwrap();
    let st = wait_for(&api, |s| s.lock.problem.is_some());
    assert!(st.lock.problem.as_deref().unwrap().contains("carries 30 fps code and the project is 29.97df fps"));
    drop(desk);
}

/// Hand a scripted source to the show in place of a device.
fn lock_scripted(t: &TestServer, kind: Kind) -> Feeder {
    let (feeder, feed) = Feed::scripted(kind);
    let show = &t.running.as_ref().unwrap().show;
    ptz_server::http::lock(show).lock_with("a", kind, Box::new(feed)).unwrap();
    feeder
}

/// LTC as audio, 10 ms at a time, each buffer stamped with the moment its last sample was captured.
fn play_ltc(feeder: Feeder, rate: FrameRate, first: i64, frames: i64, t0: Instant) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let sr = 48_000u32;
        let audio = ptz_core::ltc::encode(first, frames, rate, sr, 0.4, 1.0);
        let mut reader = LtcReader::new(sr);
        let mut fed = 0usize;
        for chunk in audio.chunks(480) {
            fed += chunk.len();
            let end = t0 + Duration::from_secs_f64(fed as f64 / sr as f64);
            sleep_until(end);
            for s in reader.feed(chunk, end) {
                if !feeder.send(s) {
                    return;
                }
            }
        }
    })
}

#[test]
fn ltc_audio_locks_the_playhead_to_the_frame() {
    let (t, api, _song) = server_with_song("lock-ltc");
    let feeder = lock_scripted(&t, Kind::Ltc);
    let st = api.status().unwrap();
    assert_eq!((st.transport.time, st.lock.source.as_deref()), (TimeMode::LockedExternal, Some("ltc")));
    do_act(&api, Action::Play);

    // Four seconds of LTC starting half a second before the song.
    let t0 = Instant::now();
    let first = code("00:59:59:12");
    let tape = play_ltc(feeder.clone(), RATE, first, 100, t0);
    wait_for(&api, |s| s.transport.run == Run::Playing);
    let mut worst = 0.0f64;
    for _ in 0..40 {
        std::thread::sleep(Duration::from_millis(25));
        let st = api.status().unwrap();
        if st.transport.run != Run::Playing {
            break;
        }
        // The status was computed a moment before it was read here, so the code has moved on a little.
        let want = first as f64 + t0.elapsed().as_secs_f64() * FPS - code("01:00:00:00") as f64;
        worst = worst.max((st.frame_f - want).abs());
        assert_eq!(st.lock.announced_rate.as_deref(), Some("25"));
        assert_eq!(st.lock.chase, ChaseState::Locked);
    }
    assert!(worst < 1.0, "LTC holds the playhead within a frame; the worst reading was {worst} frames out");
    tape.join().unwrap();
    // The tape ran out: the playhead holds where the code stopped and waits.
    let st = wait_for(&api, |s| s.transport.run == Run::WaitingForCode);
    let end = (first + 100 - code("01:00:00:00")) as f64;
    assert!((st.frame_f - end).abs() <= 10.0, "held at {} after code that ended at {end}", st.frame_f);
    assert!(st.lock.last_code.as_deref().unwrap().starts_with("01:00:03:"), "{:?}", st.lock.last_code);

    // LTC at 30 into a 25 fps project is refused, naming the input.
    do_act(&api, Action::Stop);
    do_act(&api, Action::Unlock);
    let feeder = lock_scripted(&t, Kind::Ltc);
    let tape = play_ltc(feeder, FrameRate::F30, 108_000, 30, Instant::now());
    let st = wait_for(&api, |s| s.lock.problem.is_some());
    assert_eq!(st.lock.problem.as_deref(), Some("Lock refused: the LTC input carries 30 fps code and the project is 25 fps"));
    assert_eq!(st.transport.time, TimeMode::FreeRun);
    tape.join().unwrap();
}

#[test]
fn midi_timecode_locks_and_a_full_frame_message_relocates() {
    let (t, api, _song) = server_with_song("lock-mtc");
    let feeder = lock_scripted(&t, Kind::Mtc);
    assert_eq!(api.status().unwrap().lock.source.as_deref(), Some("mtc"));
    do_act(&api, Action::Play);

    // Quarter frames: four a frame, eight to a timecode, so a full code every two frames.
    let roll = |feeder: Feeder, first: i64, blocks: i64, t0: Instant| {
        std::thread::spawn(move || {
            let mut reader = MtcReader::new();
            for block in 0..blocks {
                let tc = Timecode::from_frames(first + 2 * block, RATE);
                for (i, piece) in mtc_quarter_frames(tc, CodeRate::F25).iter().enumerate() {
                    let at = t0 + Duration::from_secs_f64((block * 8 + i as i64) as f64 / (4.0 * FPS));
                    sleep_until(at);
                    if let Some(s) = reader.feed(piece, at) {
                        feeder.send(s);
                    }
                }
            }
        })
    };
    let t0 = Instant::now();
    let first = code("01:00:02:00");
    let sequencer = roll(feeder.clone(), first, 40, t0);
    wait_for(&api, |s| s.transport.run == Run::Playing);
    let mut worst = 0.0f64;
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(25));
        let st = api.status().unwrap();
        if st.transport.run != Run::Playing {
            break;
        }
        // The two frames a quarter-frame code lags by have been added back.
        let want = first as f64 + t0.elapsed().as_secs_f64() * FPS - code("01:00:00:00") as f64;
        worst = worst.max((st.frame_f - want).abs());
        assert_eq!(st.lock.chase, ChaseState::Locked);
    }
    assert!(worst < 1.0, "MTC holds the playhead within a frame; the worst reading was {worst} frames out");
    sequencer.join().unwrap();
    wait_for(&api, |s| s.transport.run == Run::WaitingForCode);

    // The sequencer locates to 01:00:20:00 and rolls from there: the playhead goes with it.
    let mut reader = MtcReader::new();
    let target = Timecode::parse_at("01:00:20:00", RATE).unwrap();
    feeder.send(reader.feed(&mtc_full_frame(target, CodeRate::F25), Instant::now()).unwrap());
    std::thread::sleep(Duration::from_millis(100));
    let st = api.status().unwrap();
    assert_eq!(st.transport.run, Run::WaitingForCode, "a locate is a position, not rolling code");
    assert_eq!(st.lock.last_code.as_deref(), Some("01:00:20:00"));
    let t0 = Instant::now();
    let sequencer = roll(feeder, code("01:00:20:00"), 30, t0);
    let st = wait_for(&api, |s| s.transport.run == Run::Playing);
    assert!((500.0..520.0).contains(&st.frame_f), "resumed at {}", st.frame_f);
    sequencer.join().unwrap();
}

/// Audit (scratch): code that jumps while the show plays live on hardware.
#[test]
fn a_jump_in_the_code_while_playing_live_is_travelled_at_the_arm_speed() {
    use ptz_core::edit;
    use ptz_core::pose::{Pose, Preset, RigKind};
    use ptz_core::transport::Output;
    use ptz_server::hw::fake::{FakeAutoPod, FakeFr7};

    let t = TestServer::start("audit-jump");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let (fr7, pod) = (FakeFr7::start(), FakeAutoPod::start());
    let mut project = api.project().unwrap();
    project.cameras[0].rig.kind = RigKind::Hardware;
    project.cameras[0].rig.fr7_host = fr7.visca.to_string();
    project.cameras[0].rig.fr7_http_port = fr7.http.port();
    project.cameras[0].rig.fr7_user = fr7.user.clone();
    project.cameras[0].rig.fr7_password = fr7.password.clone();
    project.cameras[0].rig.autopod_host = pod.addr.to_string();
    project.cameras[0].rig.autopod_calibration = FakeAutoPod::height_map();
    let project = api.put_project(&project).unwrap();
    wait_for(&api, |s| s.first().adapters.len() == 2 && s.first().adapters.iter().all(|a| a.connected));
    let arm_speed = project.cameras[0].rig.positioning_speed * project.cameras[0].axes.pan.max_speed;

    let created = api.new_song("Jump", Some("01:00:00:00")).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let far: Preset = api
        .post("/presets", &StorePreset { name: "Far".into(), pose: Some(Pose { pan: 150.0, ..Pose::home() }), camera: None })
        .unwrap();
    let (song, clip) = edit::insert_preset_move(&created.song, &project, &far, 25).unwrap();
    let end = {
        let c = song.motion_clip(&clip).unwrap();
        c.start_frame + c.duration_frames
    };
    api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    api.save(&id).unwrap();
    api.commit(&id).unwrap();

    let addr = art_net_port(&api);
    lock_to(&api, "artnet").unwrap();
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    let desk = ArtNetDesk::start(addr, CodeRate::F25);
    desk.set(Desk::Roll { code: code("01:00:00:00"), since: Instant::now() });
    wait_for(&api, |s| s.transport.run == Run::Playing);
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(api.status().unwrap().transport.output, Output::Armed);

    // The desk locates to after the end of the move, where the head belongs at 150 degrees, and rolls on.
    let after = code("01:00:00:00") + end + 50;
    desk.set(Desk::Roll { code: after, since: Instant::now() });
    let began = Instant::now();
    let mut samples: Vec<(Instant, f64)> = Vec::new();
    let mut outputs = Vec::new();
    while began.elapsed() < Duration::from_millis(2500) {
        samples.push((Instant::now(), fr7.pose().0));
        let st = api.status().unwrap();
        if outputs.last() != Some(&(st.transport.output, st.transport.run)) {
            outputs.push((st.transport.output, st.transport.run));
        }
        assert_eq!(st.telemetry.fault, None);
        std::thread::sleep(Duration::from_millis(10));
    }
    let fastest = samples
        .iter()
        .enumerate()
        .filter_map(|(i, (t0, p0))| {
            let (t1, p1) = samples[i..].iter().find(|(t, _)| t.duration_since(*t0) >= Duration::from_millis(300))?;
            Some((p1 - p0).abs() / t1.duration_since(*t0).as_secs_f64())
        })
        .fold(0.0, f64::max);
    let log: Vec<LogRecord> = api.get("/log?since=0").unwrap();
    let chase: Vec<String> = log.iter().filter(|r| r.kind == "chase" || r.kind == "event").map(|r| r.data.to_string()).collect();
    println!("MEASURE fastest pan {fastest:.1} deg/s (Arm speed {arm_speed}); states seen {outputs:?}; log {chase:?}");
    assert!(outputs.iter().any(|(o, _)| *o == Output::Positioning), "the jump was never treated as a relocation: {outputs:?}");
    assert!(fastest < arm_speed * 1.15, "the head went to the new code at {fastest:.1} deg/s");
    drop(desk);
}
