//! Pictures of the whole window for a person to look at: every surface, and the states that
//! have to be told apart at a glance. Nothing is written unless `PTZ_DESIGN_SHOTS` names a folder:
//!
//! `PTZ_DESIGN_SHOTS=$PWD/design/after cargo test -p ptz-app --offline design_pictures`

use super::*;
use crate::app::Prompt;
use egui_kittest::kittest::Queryable as _;
use ptz_core::api::{AdapterStatus, LogRecord, SongStatus, SongSummary};
use ptz_core::transport::{Run, TimeMode, TransportState};

const AUDIO: &str = "Opening number stereo mix.wav";

/// A show with three cameras and a song that has something on every lane: preset moves for each
/// camera, a keys clip, an audio bed, two markers and one move squeezed until it is too fast.
fn show() -> (Project, Vec<Preset>, Song, String) {
    let (project, mut presets) = three_cameras();
    let more = [
        ("keys", "Keys", "cam2", -20.0, -2.0, 0.55, 260.0),
        ("crowd", "Crowd wide", "cam3", 4.0, -6.0, 0.08, 400.0),
        ("singer", "Singer close", "cam3", -9.0, -3.5, 0.7, 380.0),
    ];
    for (id, name, camera, pan, tilt, zoom, elevation) in more {
        let mut preset = Preset::new(id, name, Pose { pan, tilt, zoom, focus: 0.5, elevation });
        preset.camera = camera.into();
        presets.push(preset);
    }
    let mut song = Song::new("s", "Opening Number", "01:00:00:00", 1500);
    let mut first = String::new();
    let mut squeezed = String::new();
    let moves =
        [("lectern", 80), ("awards", 520), ("wide", 980), ("drums", 240), ("keys", 760), ("crowd", 120), ("singer", 1100)];
    for (preset, frame) in moves {
        let preset = presets.iter().find(|p| p.id == preset).expect("the preset is in the show");
        let (next, id) = ptz_core::edit::insert_preset_move(&song, &project, preset, frame).expect("the move fits");
        song = next;
        match preset.id.as_str() {
            "lectern" => first = id,
            "awards" => squeezed = id,
            _ => {}
        }
    }
    song = ptz_core::edit::set_custom_duration(&song, &project, &squeezed, 14).expect("a move can be made too short");
    let mut curves = std::collections::BTreeMap::new();
    curves.insert(
        Axis::Pan,
        vec![Key::new(0, 4.0, Interp::Bezier), Key::new(120, -14.0, Interp::Bezier), Key::new(260, -2.0, Interp::Bezier)],
    );
    curves.insert(Axis::Tilt, vec![Key::new(0, -6.0, Interp::Bezier), Key::new(260, -1.0, Interp::Bezier)]);
    curves.insert(Axis::Zoom, vec![Key::new(0, 0.08, Interp::Linear), Key::new(260, 0.5, Interp::Linear)]);
    song = ptz_core::edit::punch_keys(&song, &project, "cam3", 640, 260, curves, None).expect("the keys fit").0;
    song = ptz_core::edit::add_media_clip(&song, &project, "bed", AUDIO, ptz_core::song::Track::Audio, 1400, 0)
        .expect("the audio fits")
        .0;
    for (frame, name) in [(240, "Verse"), (760, "Chorus")] {
        song = ptz_core::edit::add_marker(&song, &project, frame, name).expect("a marker can be added");
    }
    (project, presets, song, first)
}

fn bed() -> ptz_core::api::MediaInfo {
    ptz_core::api::MediaInfo {
        id: "bed".into(),
        name: AUDIO.into(),
        kind: ptz_core::api::MediaKind::Audio,
        channels: 2,
        sample_rate: 48_000,
        samples: 2_688_000,
        duration_s: 56.0,
        duration_frames: 1400,
        start_tc: Some("01:00:00:00".into()),
        start_tc_source: Some("bext".into()),
        proxy_frames: 0,
        ltc_channel: None,
        start_samples: None,
        proxy_rate: None,
    }
}

/// A waveform that swells and falls, so the audio lane has a shape to judge.
fn bed_level() -> ptz_core::api::PeakLevel {
    let samples_per_bucket = 2048u32;
    let buckets = 2_688_000usize.div_ceil(samples_per_bucket as usize);
    let pairs = (0..buckets)
        .map(|i| {
            let t = i as f32 / buckets as f32;
            let swell = 0.35 + 0.55 * (t * 9.0).sin().abs();
            let beat = 0.6 + 0.4 * ((i % 23) as f32 / 23.0);
            let peak = (swell * beat * 120.0) as i8;
            (-peak, peak)
        })
        .collect();
    ptz_core::api::PeakLevel { samples_per_bucket, sample_rate: 48_000, pairs }
}

fn log_records() -> Vec<LogRecord> {
    let record = |seq: u64, time: &str, kind: &str, data: serde_json::Value| LogRecord {
        seq,
        time: format!("2026-11-15T{time}Z"),
        kind: kind.into(),
        data,
        prev: String::new(),
        hash: String::new(),
    };
    vec![
        record(
            41,
            "19:02:11",
            "adapter",
            serde_json::json!({"camera": "cam1", "name": "FR7", "connected": true, "detail": "192.168.10.21"}),
        ),
        record(42, "19:02:14", "lease", serde_json::json!({"holder": "Front of house"})),
        record(
            43,
            "19:03:40",
            "arm",
            serde_json::json!({"song": "Opening Number", "revision": 3, "sha256": "9f2c41d07ab3e6aa", "by": "Front of house"}),
        ),
        record(44, "19:03:52", "transport", serde_json::json!({"action": "play", "frame": 0, "by": "Front of house"})),
        record(
            45,
            "19:04:09",
            "refused",
            serde_json::json!({"action": "record_on", "reason": "Disarm to record", "by": "Balcony"}),
        ),
        record(
            46,
            "19:04:31",
            "fault",
            serde_json::json!({"camera": "cam2", "reason": "Tracking fault: pan is 3.2° from the timeline"}),
        ),
        record(47, "19:04:31", "camera_held", serde_json::json!({"camera": "cam2", "still_running": 2})),
        record(48, "19:04:40", "transport", serde_json::json!({"action": "stop", "frame": 1012, "by": "Front of house"})),
        record(49, "19:05:02", "estop", serde_json::json!({"by": "Balcony"})),
        record(50, "19:05:02", "estop_halted", serde_json::json!({"after_ms": 180})),
        record(51, "19:06:15", "issue_accepted", serde_json::json!({"by": "Front of house", "clip": "c2", "issues": [1]})),
    ]
}

/// The window on the show, as it is with a server behind it: a committed song, an audio output, a song list and a log.
fn window(size: egui::Vec2) -> Rig {
    let (project, presets, song, first) = show();
    let mut r = rig_sized(size, project, presets, song);
    {
        let s = &mut r.harness.state_mut().session;
        s.status.song = Some(SongStatus {
            id: "s".into(),
            name: "Opening Number".into(),
            start_tc: "01:00:00:00".into(),
            length_frames: 1500,
            revision: Some(3),
            dirty: false,
            committed_clean: true,
            edit_counter: 12,
            issues: 1,
            blocking: 1,
        });
        s.status.audio.output = Some("MacBook Pro Speakers".into());
        s.status.audio.device_rate = 48_000;
        s.status.audio.peak_left = 0.32;
        s.status.audio.peak_right = 0.21;
        s.status.frame = 430;
        s.status.frame_f = 430.0;
        s.media = vec![bed()];
        s.peaks.insert("bed".into(), bed_level());
        s.songs = vec![
            SongSummary {
                id: "s".into(),
                name: "Opening Number".into(),
                start_tc: "01:00:00:00".into(),
                length_frames: 1500,
                revision: Some(3),
                dirty: false,
                loaded: true,
            },
            SongSummary {
                id: "t".into(),
                name: "Ballad".into(),
                start_tc: "01:10:00:00".into(),
                length_frames: 5200,
                revision: Some(7),
                dirty: true,
                loaded: false,
            },
            SongSummary {
                id: "u".into(),
                name: "Encore".into(),
                start_tc: "01:20:00:00".into(),
                length_frames: 4100,
                revision: None,
                dirty: false,
                loaded: false,
            },
        ];
        s.log = log_records();
        s.timeline.selection.insert(first);
    }
    r.harness.run_steps(3);
    r
}

/// Change the transport, and let the guidance and the live flag follow it as the server's would.
fn transport(r: &mut Rig, change: impl FnOnce(&mut TransportState)) {
    let s = &mut r.harness.state_mut().session;
    change(&mut s.status.transport);
    s.status.guidance = ptz_core::transport::guidance(&s.status.transport, &ptz_core::transport::Context::ready());
    s.status.hardware_live = s.status.transport.hardware_live();
}

/// Every camera on a real rig that answers.
fn on_hardware(r: &mut Rig) {
    let s = &mut r.harness.state_mut().session;
    s.status.telemetry.hardware = true;
    for (i, camera) in s.status.cameras.iter_mut().enumerate() {
        camera.hardware = true;
        camera.rig_pose = Pose { pan: -12.4 + i as f64 * 9.0, tilt: -1.5, zoom: 0.42, focus: 0.5, elevation: 310.0 };
        camera.error = Pose { pan: 0.12, tilt: -0.04, zoom: 0.0, focus: 0.0, elevation: 1.5 };
        camera.adapters = vec![
            AdapterStatus {
                name: "FR7".into(),
                connected: true,
                detail: format!("192.168.10.{}", 21 + i),
                calibrated: None,
                round_trip_ms: Some(3.4),
                raw_position: None,
            },
            AdapterStatus {
                name: "AutoPod+".into(),
                connected: true,
                detail: format!("192.168.10.{}:5000", 41 + i),
                calibrated: Some(true),
                round_trip_ms: Some(6.1),
                raw_position: Some(15250.0),
            },
        ];
    }
}

/// The pictures taken of the smallest window as well: the ones where room is shortest.
const AT_THE_SMALLEST: [&str; 12] = [
    "00-connect",
    "01-main",
    "02-timeline-axis-lanes",
    "03-graph",
    "05-settings-rig",
    "06-dialog-new-song",
    "06-dialog-stop-not-sent",
    "07-armed-playing",
    "10-fault-held",
    "11-estop-latched",
    "14-media-stage-rig-log",
    "20-twelve-cameras",
];

fn wanted(name: &str) -> bool {
    !name.ends_with("1100x700") || AT_THE_SMALLEST.iter().any(|kept| name.starts_with(kept))
}

/// Write the window as it is now. The stage pictures take a few frames to be drawn in turn.
fn save(r: &mut Rig, dir: &std::path::Path, name: &str) {
    if !wanted(name) {
        return;
    }
    for _ in 0..10 {
        r.harness.run_steps(1);
    }
    let window = r.harness.ctx.content_rect();
    r.picture().save(window, &dir.join(format!("{name}.png")));
}

/// The connect screen, which is drawn before there is a session: with a recent server and a refusal on show.
fn connect_screen(size: egui::Vec2, dir: &std::path::Path, name: &str) {
    let settings = crate::app::Settings {
        server: "http://127.0.0.1:8787".into(),
        station: "Front of house".into(),
        recent: vec!["http://127.0.0.1:8787".into(), "http://show-server.local:8787".into()],
        ..Default::default()
    };
    let textures = picture::Textures::default();
    let mut themed = false;
    let mut harness = Harness::builder().with_size(size).renderer(textures.clone()).build_ui_state(
        move |ui, settings: &mut crate::app::Settings| {
            if !themed {
                crate::theme::apply(ui.ctx());
                themed = true;
                return;
            }
            let error = "Could not reach http://show-server.local:8787: connection refused";
            crate::app::connect_form(ui, settings, Some(error), false, true);
        },
        settings,
    );
    harness.run_steps(4);
    let window = harness.ctx.content_rect();
    picture::Picture::of(&harness.ctx, harness.output(), &textures).save(window, &dir.join(format!("{name}.png")));
}

#[test]
fn design_pictures() {
    let Some(dir) = std::env::var_os("PTZ_DESIGN_SHOTS").map(std::path::PathBuf::from) else { return };
    let sizes = [(DEFAULT_WINDOW, "1500x920"), (SMALLEST_WINDOW, "1100x700")];

    for (size, tag) in sizes {
        connect_screen(size, &dir, &format!("00-connect-{tag}"));

        // The window at rest: presets, Program monitor, Inspector on a selected move, timeline.
        let mut r = window(size);
        save(&mut r, &dir, &format!("01-main-{tag}"));

        // The timeline with a camera's axis lanes open and an axis chosen for Record.
        let mut r = window(size);
        {
            let s = &mut r.harness.state_mut().session;
            s.timeline.view.set_open(0, true);
            s.toggle_record_axis(Axis::Pan);
        }
        save(&mut r, &dir, &format!("02-timeline-axis-lanes-{tag}"));

        // The graph editor on the keys clip, with a key selected so its handles show.
        let mut r = window(size);
        {
            let shell = r.harness.state_mut();
            let s = &mut shell.session;
            let keys = s.song.as_ref().and_then(|song| song.motion.iter().find(|c| c.is_keys())).map(|c| c.id.clone());
            let keys = keys.expect("the show has a keys clip");
            s.select_camera("cam3");
            s.timeline.selection.clear();
            s.timeline.selection.insert(keys.clone());
            s.timeline.key_selection.insert((keys, Axis::Pan, 1));
            shell.show_tab(Tab::Graph);
        }
        save(&mut r, &dir, &format!("03-graph-{tag}"));
        r.harness.state_mut().session.graph.speed = true;
        save(&mut r, &dir, &format!("04-graph-speed-{tag}"));

        // Show settings.
        for (page, name) in [(Page::Cameras, "cameras"), (Page::Rig, "rig"), (Page::Limits, "limits"), (Page::Lens, "lens")] {
            let mut r = window(size);
            if page == Page::Rig {
                let s = &mut r.harness.state_mut().session;
                s.project.cameras[0].rig.kind = ptz_core::pose::RigKind::Hardware;
                s.project.cameras[0].rig.fr7_host = "192.168.10.21".into();
                s.project.cameras[0].rig.autopod_host = "192.168.10.41".into();
            }
            r.settings(page);
            save(&mut r, &dir, &format!("05-settings-{name}-{tag}"));
        }

        // Dialogs.
        for (prompt, name) in [
            (Prompt::NewSong, "new-song"),
            (Prompt::LoadSong, "load-song"),
            (Prompt::Revert, "revert"),
            (Prompt::SyncPresets, "sync-presets"),
            (Prompt::StopNotSent, "stop-not-sent"),
        ] {
            let mut r = window(size);
            safety::open(&mut r, prompt);
            save(&mut r, &dir, &format!("06-dialog-{name}-{tag}"));
        }

        // Armed and playing on real rigs, with the Rig controller in front for its tracking bars.
        let mut r = window(size);
        on_hardware(&mut r);
        transport(&mut r, |t| {
            t.output = Output::Armed;
            t.run = Run::Playing;
        });
        r.harness.state_mut().show_tab(Tab::Rig);
        save(&mut r, &dir, &format!("07-armed-playing-{tag}"));

        // Positioning: armed, and the rig is travelling to the playhead.
        let mut r = window(size);
        on_hardware(&mut r);
        transport(&mut r, |t| t.output = Output::Positioning);
        save(&mut r, &dir, &format!("08-positioning-{tag}"));

        // Record on, locked to LTC, with a pass under way.
        let mut r = window(size);
        on_hardware(&mut r);
        transport(&mut r, |t| {
            t.record = true;
            t.run = Run::Playing;
            t.time = TimeMode::LockedExternal;
        });
        {
            let s = &mut r.harness.state_mut().session;
            s.status.record.axes = vec![Axis::Pan, Axis::Tilt];
            s.status.record.active = true;
            s.status.record.frames = 212;
            s.status.lock.source = Some("ltc".into());
            s.status.lock.chase = ptz_core::chase::ChaseState::Locked;
            s.status.lock.last_code = Some("01:00:17:05".into());
            s.timeline.view.set_open(0, true);
        }
        r.harness.state_mut().show_tab(Tab::Rig);
        save(&mut r, &dir, &format!("09-recording-locked-{tag}"));

        // A camera faulted and held while the others carry on, then E-Stop latched.
        let mut r = window(size);
        on_hardware(&mut r);
        transport(&mut r, |t| {
            t.output = Output::Armed;
            t.run = Run::Playing;
        });
        {
            let shell = r.harness.state_mut();
            let s = &mut shell.session;
            let fault = "2 Centre: Tracking fault: pan is 3.2° from the timeline. The rig is held";
            s.status.telemetry.fault = Some(fault.into());
            s.status.cameras[1].fault = Some("Tracking fault: pan is 3.2° from the timeline".into());
            s.status.cameras[1].held = true;
            s.status.cameras[2].adapters[0].connected = false;
            shell.show_tab(Tab::Multiview);
        }
        save(&mut r, &dir, &format!("10-fault-held-{tag}"));
        transport(&mut r, |t| t.estop = true);
        r.harness.state_mut().show_tab(Tab::Program);
        save(&mut r, &dir, &format!("11-estop-latched-{tag}"));

        // Watching: another station holds the lease, and the server has been lost.
        let mut r = window(size);
        {
            let s = &mut r.harness.state_mut().session;
            s.status.lease =
                Some(ptz_core::api::LeaseStatus { holder: "Balcony".into(), client_id: "balcony".into(), expires_in_ms: 5000 });
            s.status.you_hold_lease = false;
            s.toast("Balcony took the edit lease", true);
            s.toast("Saved Opening Number", false);
        }
        save(&mut r, &dir, &format!("12-watching-toasts-{tag}"));
        r.harness.state_mut().session.lost = Some("Lost the connection to the server. Reconnecting".into());
        save(&mut r, &dir, &format!("13-server-lost-{tag}"));

        // The other tabs: Media, Stage, Rig controller and Log; then Multiview and Reference.
        let mut r = window(size);
        for tab in [Tab::Media, Tab::Stage, Tab::Rig, Tab::Log] {
            r.harness.state_mut().show_tab(tab);
        }
        save(&mut r, &dir, &format!("14-media-stage-rig-log-{tag}"));
        let mut r = window(size);
        r.harness.state_mut().show_tab(Tab::Multiview);
        r.harness.state_mut().session.timeline.selection.clear();
        save(&mut r, &dir, &format!("15-multiview-song-inspector-{tag}"));
        r.harness.state_mut().show_tab(Tab::Reference);
        save(&mut r, &dir, &format!("16-reference-{tag}"));

        // The View menu, open beside E-STOP.
        let mut r = window(size);
        let view = safety::rect_of(&r, "View").center();
        r.click(view);
        save(&mut r, &dir, &format!("17-view-menu-{tag}"));

        // The Lock menu, with the file on the timeline in it.
        let mut r = window(size);
        let lock = safety::rect_of(&r, "Lock").center();
        r.click(lock);
        save(&mut r, &dir, &format!("18-lock-menu-{tag}"));

        // The Inspector on the audio clip: its file timecode and the lock to it.
        let mut r = window(size);
        {
            let s = &mut r.harness.state_mut().session;
            let audio = s.song.as_ref().and_then(|song| song.media.first()).map(|c| c.id.clone()).expect("the show has audio");
            s.timeline.selection.clear();
            s.timeline.selection.insert(audio);
        }
        save(&mut r, &dir, &format!("19-inspector-audio-{tag}"));

        // Twelve cameras: the camera bar at its fullest, and a tile for each.
        let mut project = Project::default();
        for n in 2..=12 {
            let mut camera = ptz_core::CameraConfig::new(&format!("cam{n}"), &format!("Camera {n}"));
            camera.mount.x = (n as f64 - 6.5) * 2.0;
            project.cameras.push(camera);
        }
        let mut r = rig_sized(size, project, presets(), Song::new("s", "Opening Number", "01:00:00:00", 1500));
        r.harness.state_mut().show_tab(Tab::Multiview);
        save(&mut r, &dir, &format!("20-twelve-cameras-{tag}"));

        // The set list, as it is in a show and while it is being arranged.
        let mut r = window(size);
        r.harness.state_mut().session.project.set_list = vec!["s".into(), "t".into()];
        r.harness.state_mut().show_tab(Tab::SetList);
        r.harness.run_steps(3);
        save(&mut r, &dir, &format!("22-set-list-{tag}"));
        r.press("Arrange");
        save(&mut r, &dir, &format!("23-set-list-arranging-{tag}"));

        // A tip, under the pointer on the timeline's Fit button.
        let mut r = window(size);
        // The timeline's Fit: the lowest of them, now the stage viewer has one too.
        let fit = r
            .harness
            .query_all_by_label("Fit")
            .map(|n| n.rect())
            .max_by(|a, b| a.top().total_cmp(&b.top()))
            .expect("the timeline has a Fit button")
            .center();
        r.harness.hover_at(fit);
        r.harness.run_steps(90);
        save(&mut r, &dir, &format!("24-tip-{tag}"));

        // No song open: what the panels say when they have nothing to show.
        let mut r = rig_sized(size, Project::default(), Vec::new(), Song::new("s", "Opening Number", "01:00:00:00", 1500));
        r.harness.state_mut().session.song = None;
        r.harness.state_mut().show_tab(Tab::Media);
        save(&mut r, &dir, &format!("21-nothing-open-{tag}"));
    }
}
