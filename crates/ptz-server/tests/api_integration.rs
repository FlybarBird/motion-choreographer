//! API integration tests: transport rejections, preview vs live, audio upload.

use ptz_core::transport::{TransportAction, TransportPolicy};
use ptz_server::{spawn_ticker, ShowEngine};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

async fn boot() -> (SocketAddr, Arc<ShowEngine>) {
    let dir = tempfile::tempdir().unwrap();
    let engine = ShowEngine::new(dir.path().to_path_buf());
    // Leak tempdir for process lifetime of test
    std::mem::forget(dir);
    spawn_ticker(Arc::clone(&engine));
    let app = ptz_server::api::router(Arc::clone(&engine));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    (addr, engine)
}

#[tokio::test]
async fn play_while_editing_returns_409() {
    let (addr, engine) = boot().await;
    // Make song dirty
    let songs = engine.list_songs();
    let mut song = songs[0].clone();
    engine.select_song(&song.id).unwrap();
    song.dirty = true;
    engine.update_song_draft(song).unwrap();

    let url = format!("http://{addr}/api/v1/transport/play");
    let resp = reqwest::Client::new().post(&url).send().await.unwrap();
    assert_eq!(resp.status(), 409, "Play must not be accepted while Editing");
    let body = resp.text().await.unwrap();
    assert!(
        body.to_lowercase().contains("save") || body.to_lowercase().contains("commit"),
        "body={body}"
    );
}

#[tokio::test]
async fn preview_allowed_while_editing_and_not_hardware_live() {
    let (addr, engine) = boot().await;
    let song = engine.list_songs()[0].clone();
    engine.select_song(&song.id).unwrap();
    engine.update_song_draft(song).unwrap();

    let url = format!("http://{addr}/api/v1/transport/preview");
    let resp = reqwest::Client::new().post(&url).send().await.unwrap();
    assert!(resp.status().is_success(), "{}", resp.status());
    let status: ptz_core::ShowStatus = resp.json().await.unwrap();
    assert!(!status.transport.hardware_live);
    assert_eq!(status.transport.mode, ptz_core::TransportMode::Preview);
}

#[tokio::test]
async fn live_path_prepare_arm_play() {
    let (addr, engine) = boot().await;
    let song = engine.list_songs()[0].clone();
    engine.select_song(&song.id).unwrap();
    engine.save_song(song).unwrap();

    let client = reqwest::Client::new();
    for action in ["prepare", "arm", "play"] {
        let url = format!("http://{addr}/api/v1/transport/{action}");
        let resp = client.post(&url).send().await.unwrap();
        assert!(
            resp.status().is_success(),
            "{action} => {}",
            resp.status()
        );
    }
    let status = engine.status();
    assert!(status.transport.hardware_live);
    assert_eq!(status.transport.phase, ptz_core::TransportPhase::LivePlaying);
}

#[tokio::test]
async fn ui_guidance_matches_server_acceptance() {
    let (_addr, engine) = boot().await;
    let song = engine.list_songs()[0].clone();
    engine.select_song(&song.id).unwrap();

    // Editing
    engine.update_song_draft(engine.get_song(&song.id).unwrap()).unwrap();
    let st = engine.status();
    let g = &st.guidance;
    assert!(!g.buttons.play);
    assert!(TransportPolicy::can(&st.transport, TransportAction::Play).is_err());
    assert_eq!(
        g.buttons.play,
        TransportPolicy::can(&st.transport, TransportAction::Play).is_ok()
    );
    assert_eq!(
        g.buttons.preview,
        TransportPolicy::can(&st.transport, TransportAction::Preview).is_ok()
    );

    // After commit → prepare → arm
    engine.save_song(engine.get_song(&song.id).unwrap()).unwrap();
    engine.transport_action(TransportAction::Prepare).unwrap();
    engine.transport_action(TransportAction::Arm).unwrap();
    let st = engine.status();
    assert!(st.guidance.buttons.play);
    assert!(TransportPolicy::can(&st.transport, TransportAction::Play).is_ok());
}

#[tokio::test]
async fn status_reports_audio_output_fields() {
    let (addr, engine) = boot().await;
    let song = engine.list_songs()[0].clone();
    engine.select_song(&song.id).unwrap();

    let wav = fixture_wav_path();
    let data = std::fs::read(&wav).expect("fixture wav");
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(data)
            .file_name("test-click-12_4s.wav")
            .mime_str("audio/wav")
            .unwrap(),
    );
    let url = format!("http://{addr}/api/v1/songs/{}/audio", song.id);
    let resp = reqwest::Client::new()
        .post(&url)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let status: ptz_core::ShowStatus = reqwest::Client::new()
        .get(format!("http://{addr}/api/v1/status"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let a = &status.audio_output;
    assert!(!a.device.is_empty(), "device name or silent");
    assert!(a.sample_rate >= 8000);
    let dur = a.duration_ms.expect("duration_ms");
    assert!((dur as i64 - 12400).abs() < 80, "duration_ms={dur}");
    assert_eq!(a.hardware_live, status.transport.hardware_live);
    assert_eq!(a.drift_ms, a.position_ms as i64 - a.transport_position_ms as i64);
}

#[tokio::test]
async fn audio_upload_sets_duration_and_waveform() {
    let (addr, engine) = boot().await;
    let song = engine.list_songs()[0].clone();
    engine.select_song(&song.id).unwrap();

    let wav = fixture_wav_path();
    let data = std::fs::read(&wav).expect("fixture wav");
    let form = reqwest::multipart::Form::new().part(
        "file",
        reqwest::multipart::Part::bytes(data)
            .file_name("test-click-12_4s.wav")
            .mime_str("audio/wav")
            .unwrap(),
    );
    let url = format!("http://{addr}/api/v1/songs/{}/audio", song.id);
    let resp = reqwest::Client::new()
        .post(&url)
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.text().await.unwrap());
    let updated = engine.get_song(&song.id).unwrap();
    let audio = updated.audio.expect("audio meta");
    assert!(
        (audio.duration_secs - 12.4).abs() < 0.05,
        "duration {}",
        audio.duration_secs
    );
    assert!(!audio.waveform.is_empty());
    let max = audio.waveform.iter().cloned().fold(0.0f32, f32::max);
    assert!(max > 0.2, "waveform should capture click peak, max={max}");
}

fn fixture_wav_path() -> std::path::PathBuf {
    let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("../../fixtures/test-click-12_4s.wav");
    p
}
