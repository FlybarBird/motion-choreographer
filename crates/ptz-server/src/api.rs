//! Versioned HTTP + WebSocket API (`/api/v1/...`).

use crate::show::ShowEngine;
use axum::extract::multipart::Multipart;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use ptz_core::transport::TransportAction;
use ptz_core::{ProjectSettings, ServerEvent, Song, API_VERSION};
use serde::Deserialize;
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tracing::info;

pub type AppState = Arc<ShowEngine>;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/status", get(get_status))
        .route("/api/v1/settings", get(get_settings).put(put_settings))
        .route("/api/v1/songs", get(list_songs).post(create_song))
        .route("/api/v1/songs/{id}", get(get_song).put(put_song))
        .route("/api/v1/songs/{id}/select", post(select_song))
        .route("/api/v1/songs/{id}/commit", post(commit_song))
        .route("/api/v1/songs/{id}/moves", post(add_move))
        .route("/api/v1/songs/{id}/audio", post(upload_audio))
        .route("/api/v1/presets", get(list_presets))
        .route("/api/v1/transport/{action}", post(transport))
        .route("/api/v1/ws", get(ws_handler))
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "ok": true,
        "api_version": API_VERSION,
        "service": "ptz-director-server"
    }))
}

async fn get_status(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(state.status()).unwrap())
}

async fn get_settings(State(state): State<AppState>) -> Json<ProjectSettings> {
    Json(state.settings.lock().clone())
}

async fn put_settings(
    State(state): State<AppState>,
    Json(settings): Json<ProjectSettings>,
) -> Json<ProjectSettings> {
    Json(state.update_settings(settings))
}

async fn list_songs(State(state): State<AppState>) -> Json<Vec<Song>> {
    Json(state.list_songs())
}

#[derive(Deserialize)]
struct CreateSongBody {
    name: String,
}

async fn create_song(
    State(state): State<AppState>,
    Json(body): Json<CreateSongBody>,
) -> Json<Song> {
    Json(state.create_song(body.name))
}

async fn get_song(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Song>, (StatusCode, String)> {
    state
        .get_song(&id)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "song not found".into()))
}

async fn put_song(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(mut song): Json<Song>,
) -> Result<Json<Song>, (StatusCode, String)> {
    song.id = id;
    state
        .update_song_draft(song)
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

async fn select_song(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Song>, (StatusCode, String)> {
    state
        .select_song(&id)
        .map(Json)
        .map_err(|e| (StatusCode::NOT_FOUND, e))
}

async fn commit_song(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<Song>, (StatusCode, String)> {
    let song = state
        .get_song(&id)
        .ok_or((StatusCode::NOT_FOUND, "song not found".into()))?;
    state
        .save_song(song)
        .map(Json)
        .map_err(|e| (StatusCode::CONFLICT, e))
}

#[derive(Deserialize)]
struct AddMoveBody {
    preset_id: String,
    start_frame: i64,
    duration_frames: i64,
}

async fn add_move(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AddMoveBody>,
) -> Result<Json<Song>, (StatusCode, String)> {
    state
        .add_move_from_preset(&id, &body.preset_id, body.start_frame, body.duration_frames)
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

async fn upload_audio(
    State(state): State<AppState>,
    Path(id): Path<String>,
    mut multipart: Multipart,
) -> Result<Json<Song>, (StatusCode, String)> {
    let mut filename = String::from("audio.wav");
    let mut bytes: Option<Vec<u8>> = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    {
        if let Some(name) = field.file_name().map(|s| s.to_string()) {
            filename = name;
        }
        let data = field
            .bytes()
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        bytes = Some(data.to_vec());
    }
    let bytes = bytes.ok_or((StatusCode::BAD_REQUEST, "missing file".into()))?;
    state
        .import_audio(&id, &filename, &bytes)
        .map(Json)
        .map_err(|e| (StatusCode::BAD_REQUEST, e))
}

async fn list_presets(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(state.presets()).unwrap())
}

async fn transport(
    State(state): State<AppState>,
    Path(action): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let action = parse_action(&action).ok_or((StatusCode::BAD_REQUEST, "unknown action".into()))?;
    match state.transport_action(action) {
        Ok(status) => Ok(Json(serde_json::to_value(status).unwrap())),
        Err(e) => Err((StatusCode::CONFLICT, e)),
    }
}

fn parse_action(s: &str) -> Option<TransportAction> {
    Some(match s {
        "save_commit" | "commit" => TransportAction::SaveCommit,
        "prepare" => TransportAction::Prepare,
        "arm" => TransportAction::Arm,
        "play" => TransportAction::Play,
        "preview" => TransportAction::Preview,
        "stop" => TransportAction::Stop,
        "cue" => TransportAction::Cue,
        "estop" => TransportAction::Estop,
        "clear_estop" => TransportAction::ClearEstop,
        _ => return None,
    })
}

async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| client_ws(socket, state))
}

async fn client_ws(socket: WebSocket, state: AppState) {
    let (mut tx, mut rx) = socket.split();
    let mut events = state.subscribe();
    // Push initial status
    if let Ok(text) = serde_json::to_string(&ServerEvent::Status(state.status())) {
        let _ = tx.send(Message::Text(text.into())).await;
    }
    info!("websocket client connected");

    loop {
        tokio::select! {
            ev = events.recv() => {
                match ev {
                    Ok(ev) => {
                        if let Ok(text) = serde_json::to_string(&ev) {
                            if tx.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
            msg = rx.next() => {
                match msg {
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(Message::Ping(p))) => {
                        let _ = tx.send(Message::Pong(p)).await;
                    }
                    Some(Ok(Message::Text(t))) => {
                        // Client may send {"action":"play"} etc.
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t) {
                            if let Some(action) = v.get("action").and_then(|a| a.as_str()).and_then(parse_action) {
                                let _ = state.transport_action(action);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    info!("websocket client disconnected (show continues under server control)");
}
