//! Audit: live output over real HTTP against the stand-in devices.

mod common;

use common::*;
use ptz_core::api::*;
use ptz_core::client::Api;
use ptz_core::edit;
use ptz_core::pose::{Axis, Cmp, KeepOutRule, Pose, Preset, Project, RigKind};
use ptz_core::transport::{Action, Output, Run};
use ptz_server::hw::fake::{FakeAutoPod, FakeFr7, FakePreset};
use std::time::{Duration, Instant};

struct Rig {
    fr7: FakeFr7,
    pod: FakeAutoPod,
}

impl Rig {
    fn start() -> Rig {
        Rig { fr7: FakeFr7::start(), pod: FakeAutoPod::start() }
    }

    fn connect_with(&self, api: &Api, tweak: impl FnOnce(&mut Project)) -> Project {
        let mut project = api.project().unwrap();
        project.cameras[0].rig.kind = RigKind::Hardware;
        project.cameras[0].rig.fr7_host = self.fr7.visca.to_string();
        project.cameras[0].rig.fr7_http_port = self.fr7.http.port();
        project.cameras[0].rig.fr7_user = self.fr7.user.clone();
        project.cameras[0].rig.fr7_password = self.fr7.password.clone();
        project.cameras[0].rig.autopod_host = self.pod.addr.to_string();
        project.cameras[0].rig.autopod_calibration = FakeAutoPod::height_map();
        tweak(&mut project);
        let project = api.put_project(&project).unwrap();
        wait_for(api, |s| {
            s.first().adapters.len() == 2
                && s.first().adapters.iter().all(|a| a.connected)
                && s.first().adapters[1].calibrated == Some(true)
        });
        project
    }

    fn connect(&self, api: &Api) -> Project {
        self.connect_with(api, |_| {})
    }
}

fn store(api: &Api, name: &str, pose: Pose) -> Preset {
    api.post("/presets", &StorePreset { name: name.into(), pose: Some(pose), camera: None }).unwrap()
}

/// A saved, committed song with one move to `preset` starting at frame 25.
fn song_with_move(api: &Api, project: &Project, name: &str, preset: &Preset) -> (String, i64) {
    let created = api.new_song(name, None).unwrap();
    let id = created.song.id.clone();
    api.load_song(&id).unwrap();
    let (song, clip) = edit::insert_preset_move(&created.song, project, preset, 25).unwrap();
    let end = {
        let c = song.motion_clip(&clip).unwrap();
        c.start_frame + c.duration_frames
    };
    let w = api.put_working(&PutWorking { expected_counter: 0, song }).unwrap();
    assert!(w.issues.is_empty(), "{:?}", w.issues);
    api.save(&id).unwrap();
    api.commit(&id).unwrap();
    (id, end)
}

fn seek(api: &Api, frame: i64) -> Status {
    api.act(Action::Seek, &TransportRequest { frame: Some(frame), ..Default::default() }).unwrap()
}

/// Fastest pan of the stand-in head over any 100 ms while `during` runs, in degrees per second.
fn fastest_pan(fr7: &FakeFr7, for_ms: u64) -> f64 {
    let began = Instant::now();
    let mut samples: Vec<(Instant, f64)> = Vec::new();
    while began.elapsed() < Duration::from_millis(for_ms) {
        samples.push((Instant::now(), fr7.pose().0));
        std::thread::sleep(Duration::from_millis(10));
    }
    samples
        .iter()
        .enumerate()
        .filter_map(|(i, (t0, p0))| {
            let (t1, p1) = samples[i..].iter().find(|(t, _)| t.duration_since(*t0) >= Duration::from_millis(100))?;
            Some((p1 - p0).abs() / t1.duration_since(*t0).as_secs_f64())
        })
        .fold(0.0, f64::max)
}

#[test]
fn play_at_the_end_of_an_armed_song_does_not_swing_the_rig_back_at_full_speed() {
    let t = TestServer::start("audit-rewind");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    let project = rig.connect(&api);
    let arm_speed = project.cameras[0].rig.positioning_speed * project.cameras[0].axes.pan.max_speed;
    assert_eq!(arm_speed, 30.0, "Arm moves at half of 60 degrees per second");

    let mark = store(&api, "Far", Pose { pan: 150.0, ..Pose::home() });
    let (_, end) = song_with_move(&api, &project, "Rewind", &mark);
    let length = api.status().unwrap().song.unwrap().length_frames;

    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    // To the end of the move: the rig is positioned there at Arm speed, as the README says.
    let st = seek(&api, end);
    assert_eq!(st.transport.output, Output::Positioning);
    let on_the_way = fastest_pan(&rig.fr7, 1500);
    assert!(on_the_way < arm_speed * 1.15, "positioning ran at {on_the_way:.1} deg/s");
    wait_for(&api, |s| s.transport.output == Output::Armed);
    assert!((rig.fr7.pose().0 - 150.0).abs() < 1.0);

    // Play Live through the last frames of the song; it stops at the end and stays armed.
    seek(&api, length - 10);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);
    let st = wait_for(&api, |s| s.transport.run == Run::Stopped);
    assert_eq!((st.frame, st.transport.output), (length, Output::Armed));

    // The operator presses Play Live again.
    let st = do_act(&api, Action::Play);
    let speed = fastest_pan(&rig.fr7, 1200);
    let now = api.status().unwrap();
    assert!(
        speed < arm_speed * 1.15,
        "Play at the end rewound to frame {} with output {:?} and the head swung back at {speed:.1} deg/s \
         (Arm speed is {arm_speed} deg/s); 1.2 s later it is at pan {:.0}, fault: {:?}",
        st.frame,
        st.transport.output,
        rig.fr7.pose().0,
        now.telemetry.fault
    );
}

#[test]
fn a_plain_move_keeps_the_rig_out_of_a_keep_out_zone_the_timeline_stays_out_of() {
    keep_out_during_a_move(true);
}

/// The same move steered along the curve: the control for the test above.
#[test]
fn a_steered_move_keeps_the_rig_out_of_a_keep_out_zone_the_timeline_stays_out_of() {
    keep_out_during_a_move(false);
}

fn keep_out_during_a_move(plain_moves: bool) {
    let t = TestServer::start("audit-ride");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    let rule = KeepOutRule {
        name: "Desk".into(),
        when_axis: Axis::Elevation,
        when: Cmp::Below,
        when_value: 100.0,
        then_axis: Axis::Tilt,
        must_be: Cmp::Above,
        must_value: -8.0,
    };
    let project = rig.connect_with(&api, |p| {
        p.cameras[0].rig.fr7_absolute_moves = plain_moves;
        p.cameras[0].keep_out = vec![rule.clone()];
    });

    // Column up 400 mm while tilting down 30 degrees. On the timeline both ease together, so
    // the column is above 100 mm (a quarter of the way) before tilt passes -7.5 degrees.
    let mark = store(&api, "Over the desk", Pose { tilt: -30.0, elevation: 400.0, ..Pose::home() });
    let (_, end) = song_with_move(&api, &project, "Plain", &mark);
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);
    do_act(&api, Action::Play);

    let mut worst: Option<Pose> = None;
    loop {
        let (_, tilt, ..) = rig.fr7.pose();
        let at = Pose { tilt, elevation: rig.pod.height_mm(), ..Pose::home() };
        if rule.violated(&at) && worst.is_none_or(|w| at.tilt < w.tilt) {
            worst = Some(at);
        }
        let st = api.status().unwrap();
        if st.frame > end || st.transport.output != Output::Armed {
            assert_eq!(st.telemetry.fault, None);
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        worst.is_none(),
        "the real rig broke \"{}\" with no fault raised: tilt reached {:.1} degrees while the column was at {:.0} mm",
        rule.describe(),
        worst.unwrap().tilt,
        worst.unwrap().elevation
    );
}

#[test]
fn preset_sync_does_not_take_the_head_into_a_keep_out_zone() {
    let t = TestServer::start("audit-sync");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    let rule = KeepOutRule::example();
    rig.connect_with(&api, |p| p.cameras[0].keep_out = vec![rule.clone()]);
    // A preset someone stored on the camera with the column up: looking 25 degrees down.
    rig.fr7.set_presets(vec![FakePreset { number: 1, name: "Down".into(), pan: 0.0, tilt: -25.0, zoom: 0.1, focus: 0.5 }]);
    rig.pod.place(0.0);

    api.sync_presets(ptz_core::FIRST_CAMERA).unwrap();
    let began = Instant::now();
    let mut lowest = 0.0f64;
    while began.elapsed() < Duration::from_secs(4) {
        lowest = lowest.min(rig.fr7.pose().1);
        std::thread::sleep(Duration::from_millis(10));
    }
    let at = Pose { tilt: lowest, elevation: rig.pod.height_mm(), ..Pose::home() };
    assert!(
        !rule.violated(&at),
        "preset sync tilted the head to {lowest:.1} degrees with the column at {:.0} mm, against \"{}\"; fault: {:?}",
        at.elevation,
        rule.describe(),
        api.status().unwrap().telemetry.fault
    );
}

#[test]
fn presets_cannot_be_changed_while_armed() {
    let t = TestServer::start("audit-presets-armed");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let first = api.presets().unwrap()[0].clone();
    do_act(&api, Action::Arm);
    wait_for(&api, |s| s.transport.output == Output::Armed);

    let stored: Result<Preset, _> =
        api.post("/presets", &StorePreset { name: "While armed".into(), pose: Some(Pose::home()), camera: None });
    let deleted: Result<Vec<Preset>, _> = api.delete(&format!("/presets/{}", first.id));
    let synced = api.sync_presets(ptz_core::FIRST_CAMERA);
    assert!(
        stored.is_err() && deleted.is_err() && synced.is_err(),
        "while armed: store -> {:?}, delete -> {:?}, sync -> {:?}",
        stored.as_ref().map(|p| &p.id),
        deleted.as_ref().map(Vec::len),
        synced.as_ref().map(Vec::len)
    );
}

#[test]
fn estop_can_be_reset_when_no_song_is_loaded() {
    let t = TestServer::start("audit-reset");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    for song in api.songs().unwrap() {
        api.delete_song(&song.id).unwrap();
    }
    let st = api.post_empty::<Status>("/estop").unwrap();
    assert!(st.transport.estop);
    let reset = api.post_empty::<Status>("/estop/reset");
    assert!(reset.is_ok(), "E-Stop is latched and Reset answers: {reset:?}");
}

#[test]
fn a_refusal_while_idle_does_not_block_arm_for_good() {
    let t = TestServer::start("audit-refused");
    let api = t.client("a");
    api.take_lease("Luke-MacBook", false).unwrap();
    let rig = Rig::start();
    let project = rig.connect(&api);
    let mark = store(&api, "Up", Pose { elevation: 60.0, ..Pose::home() });
    song_with_move(&api, &project, "Refused", &mark);

    // For a moment the leg box refuses the calibration question (ER1), then answers again.
    rig.pod.refuse(Some("#RER"));
    wait_for(&api, |s| s.guidance.reason(Action::Arm).is_some_and(|r| r.contains("refused")));
    rig.pod.refuse(None);
    std::thread::sleep(Duration::from_secs(3));

    let arm = api.act(Action::Arm, &TransportRequest::default());
    assert!(
        arm.is_ok(),
        "3 s after the leg box answers every question again, Arm is still refused: {arm:?}; adapter: {:?}",
        api.status().unwrap().first().adapters[1]
    );
}
