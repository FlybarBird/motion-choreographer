//! REST for commands and edits, WebSocket for status. All under `/api/v1`.

use crate::engine::{halt_for_exit, spawn_tick, Config, Fail, MediaTicket, Push, Shared, Show, StageUpload, Stamp};
use crate::media::{self, Ingested};
use crate::video;
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use ptz_core::api::*;
use ptz_core::pose::{Preset, Project};
use ptz_core::transport::Action;
use serde::Deserialize;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::RecvError;

#[derive(Clone)]
pub struct App {
    pub show: Shared,
    pub access_key: Option<Arc<String>>,
}

pub fn lock(show: &Shared) -> MutexGuard<'_, Show> {
    show.lock().unwrap_or_else(|e| e.into_inner())
}

impl IntoResponse for Fail {
    fn into_response(self) -> Response {
        let code = StatusCode::from_u16(self.code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (code, Json(ApiError { error: self.msg })).into_response()
    }
}

type Reply<T> = Result<Json<T>, Fail>;

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok());
                match hex {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query_value(query: Option<&str>, key: &str) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (k == key).then(|| percent_decode(v))
    })
}

fn request_value(parts_headers: &axum::http::HeaderMap, query: Option<&str>, header: &str, key: &str) -> Option<String> {
    parts_headers.get(header).and_then(|v| v.to_str().ok()).map(|s| s.to_string()).or_else(|| query_value(query, key))
}

/// Who is asking. Sent as the `x-client-id` header, or `?client=` on the WebSocket.
pub struct Client(pub String);

impl<S: Send + Sync> FromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Client(request_value(&parts.headers, parts.uri.query(), HEADER_CLIENT, "client").unwrap_or_default()))
    }
}

async fn gate(State(app): State<App>, req: Request, next: Next) -> Response {
    let query = req.uri().query().map(|s| s.to_string());
    if let Some(key) = &app.access_key {
        let given = request_value(req.headers(), query.as_deref(), HEADER_KEY, "key");
        if given.as_deref() != Some(key.as_str()) {
            return Fail { code: 401, msg: "The access key is missing or wrong".into() }.into_response();
        }
    }
    if let Some(client) = request_value(req.headers(), query.as_deref(), HEADER_CLIENT, "client") {
        lock(&app.show).touch(&client);
    }
    next.run(req).await
}

fn body_json<T: for<'de> Deserialize<'de> + Default>(body: &Bytes) -> Result<T, Fail> {
    if body.iter().all(|b| b.is_ascii_whitespace()) {
        return Ok(T::default());
    }
    serde_json::from_slice(body).map_err(|e| Fail::bad(format!("The request body is not valid: {e}")))
}

async fn get_status(State(app): State<App>, Client(c): Client) -> Json<Status> {
    Json(lock(&app.show).status(&c))
}

async fn post_lease(State(app): State<App>, Client(c): Client, Json(req): Json<LeaseRequest>) -> Reply<Status> {
    let mut s = lock(&app.show);
    s.take_lease(&c, &req)?;
    Ok(Json(s.status(&c)))
}

async fn put_panel(State(app): State<App>, Client(c): Client, Json(state): Json<PanelState>) -> Reply<PanelState> {
    Ok(Json(lock(&app.show).set_panel(&c, state)?))
}

async fn delete_lease(State(app): State<App>, Client(c): Client) -> Reply<Status> {
    let mut s = lock(&app.show);
    s.release_lease(&c)?;
    Ok(Json(s.status(&c)))
}

async fn list_songs(State(app): State<App>) -> Json<Vec<SongSummary>> {
    Json(lock(&app.show).list_songs())
}

async fn create_song(State(app): State<App>, Client(c): Client, Json(req): Json<NewSong>) -> Reply<WorkingSong> {
    Ok(Json(lock(&app.show).create_song(&c, &req)?))
}

async fn delete_song(State(app): State<App>, Client(c): Client, Path(id): Path<String>) -> Reply<Vec<SongSummary>> {
    let mut s = lock(&app.show);
    s.delete_song(&c, &id)?;
    Ok(Json(s.list_songs()))
}

async fn load_song(State(app): State<App>, Client(c): Client, Path(id): Path<String>) -> Reply<WorkingSong> {
    Ok(Json(lock(&app.show).load_song(&c, &id)?))
}

async fn get_working(State(app): State<App>, Path(id): Path<String>) -> Reply<WorkingSong> {
    Ok(Json(lock(&app.show).working(&id)?))
}

async fn put_working(
    State(app): State<App>,
    Client(c): Client,
    Path(id): Path<String>,
    Json(put): Json<PutWorking>,
) -> Reply<WorkingSong> {
    Ok(Json(lock(&app.show).put_working(&c, &id, put)?))
}

async fn save_song(State(app): State<App>, Client(c): Client, Path(id): Path<String>) -> Reply<WorkingSong> {
    Ok(Json(lock(&app.show).save_song(&c, &id)?))
}

async fn revert_song(State(app): State<App>, Client(c): Client, Path(id): Path<String>) -> Reply<WorkingSong> {
    Ok(Json(lock(&app.show).revert_song(&c, &id)?))
}

async fn copy_song(
    State(app): State<App>,
    Client(c): Client,
    Path(id): Path<String>,
    Json(req): Json<NewSong>,
) -> Reply<WorkingSong> {
    Ok(Json(lock(&app.show).copy_song(&c, &id, &req)?))
}

async fn commit_song(State(app): State<App>, Client(c): Client, Path(id): Path<String>) -> Reply<RevisionInfo> {
    Ok(Json(lock(&app.show).commit_song(&c, &id)?))
}

async fn get_revisions(State(app): State<App>, Path(id): Path<String>) -> Reply<Vec<RevisionInfo>> {
    Ok(Json(lock(&app.show).revisions(&id)?))
}

async fn transport(State(app): State<App>, Client(c): Client, Path(action): Path<String>, body: Bytes) -> Reply<Status> {
    let Some(action) = Action::from_key(&action).filter(|a| !matches!(a, Action::Estop | Action::ResetEstop)) else {
        return Err(Fail::not_found(format!("There is no transport action called {action}")));
    };
    let req: TransportRequest = body_json(&body)?;
    let mut s = lock(&app.show);
    s.act(&c, action, &req)?;
    Ok(Json(s.status(&c)))
}

async fn estop(State(app): State<App>, Client(c): Client) -> Reply<Status> {
    let mut s = lock(&app.show);
    s.act(&c, Action::Estop, &TransportRequest::default())?;
    Ok(Json(s.status(&c)))
}

async fn estop_reset(State(app): State<App>, Client(c): Client) -> Reply<Status> {
    let mut s = lock(&app.show);
    s.act(&c, Action::ResetEstop, &TransportRequest::default())?;
    Ok(Json(s.status(&c)))
}

async fn refit(
    State(app): State<App>,
    Client(c): Client,
    Path(clip): Path<String>,
    Json(req): Json<Refit>,
) -> Reply<WorkingSong> {
    Ok(Json(lock(&app.show).refit(&c, &clip, &req)?))
}

async fn get_project(State(app): State<App>) -> Json<Project> {
    Json(lock(&app.show).project.clone())
}

async fn put_project(State(app): State<App>, Client(c): Client, Json(project): Json<Project>) -> Reply<Project> {
    Ok(Json(lock(&app.show).put_project(&c, project)?))
}

#[derive(Deserialize)]
struct StageQuery {
    #[serde(default)]
    name: String,
    unit: Option<ptz_core::camera::Unit>,
    up: Option<ptz_core::camera::UpAxis>,
}

/// The largest mesh file accepted: the triangle limit with every vertex its own, and room to spare.
const STAGE_MODEL_LIMIT: usize = 96 * 1024 * 1024;

/// Check an uploaded mesh and write it beside the stage model, off the async threads and outside the show's lock.
fn check_stage_model(bytes: &[u8], q: StageQuery, into: std::path::PathBuf) -> Result<StageUpload, String> {
    let mesh = ptz_scene::mesh::Mesh::from_bytes(bytes)?;
    if mesh.triangles.len() > ptz_scene::MAX_TRIANGLES {
        return Err(format!(
            "it has {} triangles; the most that can be shown is {}",
            mesh.triangles.len(),
            ptz_scene::MAX_TRIANGLES
        ));
    }
    let bounds = mesh.bounds().ok_or("it has no triangles")?;
    let (min, max) = (bounds.min.map(f64::from), bounds.max.map(f64::from));
    let largest = (0..3).map(|i| max[i] - min[i]).fold(0.0, f64::max);
    if !(largest.is_finite() && largest > 0.0) {
        return Err("it has no size".into());
    }
    let name = q.name.rsplit(['/', '\\']).next().unwrap_or("").trim().to_string();
    let name: String = name.chars().filter(|c| !c.is_control()).take(120).collect();
    let dir = into.parent().ok_or("the stage folder could not be found")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("the stage folder could not be made: {e}"))?;
    let file = dir.join(format!("upload-{}.tmp", std::process::id()));
    std::fs::write(&file, bytes).map_err(|e| format!("it could not be written: {e}"))?;
    Ok(StageUpload {
        name: if name.is_empty() { "Stage model".into() } else { name },
        unit: q.unit.unwrap_or_else(|| ptz_core::camera::Unit::guess(largest)),
        up: q.up.unwrap_or_default(),
        triangles: mesh.triangles.len() as u64,
        min,
        max,
        file,
    })
}

async fn post_stage_model(State(app): State<App>, Client(c): Client, Query(q): Query<StageQuery>, body: Bytes) -> Reply<Project> {
    let into = {
        let s = lock(&app.show);
        s.may_change_stage(&c)?;
        s.storage.stage_model_path()
    };
    let checked = tokio::task::spawn_blocking(move || check_stage_model(&body, q, into))
        .await
        .map_err(|_| Fail::invalid("The stage model was not imported: the importer stopped unexpectedly"))?
        .map_err(|why| Fail::invalid(format!("The stage model was not imported: {why}")))?;
    Ok(Json(lock(&app.show).set_stage_model(&c, checked)?))
}

async fn get_stage_model(State(app): State<App>) -> Result<Response, Fail> {
    let path = lock(&app.show).stage_model_file()?;
    let bytes = tokio::fs::read(&path).await.map_err(Fail::io)?;
    Ok(([(axum::http::header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response())
}

async fn delete_stage_model(State(app): State<App>, Client(c): Client) -> Reply<Project> {
    Ok(Json(lock(&app.show).delete_stage_model(&c)?))
}

async fn get_inputs() -> Json<InputChoices> {
    Json(tokio::task::spawn_blocking(crate::lock::choices).await.unwrap_or_default())
}

async fn get_presets(State(app): State<App>) -> Json<Vec<Preset>> {
    Json(lock(&app.show).presets.clone())
}

async fn post_preset(State(app): State<App>, Client(c): Client, Json(req): Json<StorePreset>) -> Reply<Preset> {
    Ok(Json(lock(&app.show).store_preset(&c, &req)?))
}

async fn delete_preset(State(app): State<App>, Client(c): Client, Path(id): Path<String>) -> Reply<Vec<Preset>> {
    let mut s = lock(&app.show);
    s.delete_preset(&c, &id)?;
    Ok(Json(s.presets.clone()))
}

#[derive(Deserialize, Default)]
struct OfCamera {
    #[serde(default)]
    camera: Option<String>,
}

async fn sync_presets(State(app): State<App>, Client(c): Client, Query(q): Query<OfCamera>) -> Reply<Vec<Preset>> {
    Ok(Json(lock(&app.show).sync_presets(&c, q.camera.as_deref())?))
}

async fn preset_thumb(State(app): State<App>, Path(id): Path<String>) -> Result<Response, Fail> {
    let path = lock(&app.show).preset_thumbnail(&id)?;
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok(bytes_reply("image/jpeg", bytes)),
        Err(_) => Err(Fail::not_found("That preset has no picture")),
    }
}

async fn rig_jog(State(app): State<App>, Client(c): Client, Json(req): Json<JogRequest>) -> Reply<Status> {
    let mut s = lock(&app.show);
    s.jog(&c, req.camera.as_deref(), req.jog)?;
    Ok(Json(s.status(&c)))
}

async fn list_media(State(app): State<App>) -> Json<Vec<MediaInfo>> {
    Json(lock(&app.show).media.clone())
}

#[derive(Deserialize)]
struct UploadQuery {
    #[serde(default)]
    name: String,
}

/// Write the upload to disk as it arrives, then decode it off the async threads.
async fn receive_media(ticket: MediaTicket, body: axum::body::Body) -> Result<(Ingested, Option<Vec<f32>>), String> {
    use futures_util::StreamExt;
    use tokio::io::AsyncWriteExt;
    let tmp = ticket.incoming.join(format!(".upload-{}.tmp", ticket.id));
    let io = |e: std::io::Error| format!("could not write to the media folder: {e}");
    let mut file = tokio::fs::File::create(&tmp).await.map_err(io)?;
    let mut stream = body.into_data_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            let _ = tokio::fs::remove_file(&tmp).await;
            return Err("the upload was interrupted".into());
        };
        total += chunk.len() as u64;
        file.write_all(&chunk).await.map_err(io)?;
    }
    file.flush().await.map_err(io)?;
    drop(file);
    if total == 0 {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err("the file is empty".into());
    }
    tokio::task::spawn_blocking(move || {
        let ingested = media::ingest(&tmp, &ticket.dir, &ticket.id, &ticket.name, ticket.rate)?;
        let exclude = ingested.info.ltc_channel;
        let playable = crate::audio::resample(&ingested.decoded.mono(exclude), ingested.decoded.rate, ticket.device_rate).ok();
        Ok((ingested, playable))
    })
    .await
    .map_err(|_| "the importer stopped unexpectedly".to_string())?
}

async fn upload_media(
    State(app): State<App>,
    Client(c): Client,
    Query(q): Query<UploadQuery>,
    body: axum::body::Body,
) -> Reply<MediaInfo> {
    let reserved = lock(&app.show).reserve_media(&c, &q.name);
    let ticket = match reserved {
        Ok(t) => t,
        Err(fail) => {
            // Take the upload anyway, so the sender reads the refusal instead of a broken connection.
            use futures_util::StreamExt;
            let mut stream = body.into_data_stream();
            while let Some(Ok(_)) = stream.next().await {}
            return Err(fail);
        }
    };
    let (id, name) = (ticket.id.clone(), ticket.name.clone());
    let result = receive_media(ticket, body).await;
    let mut s = lock(&app.show);
    match result {
        Ok((ingested, playable)) => Ok(Json(s.register_media(ingested, playable))),
        Err(msg) => {
            s.cancel_media(&id);
            Err(Fail::invalid(format!("{name} was not imported: {msg}")))
        }
    }
}

async fn delete_media(State(app): State<App>, Client(c): Client, Path(id): Path<String>) -> Reply<Vec<MediaInfo>> {
    let mut s = lock(&app.show);
    s.delete_media(&c, &id)?;
    Ok(Json(s.media.clone()))
}

#[derive(Deserialize)]
struct PeaksQuery {
    /// Samples per pixel at the app's zoom. The reply is the coarsest level that is at least that fine.
    #[serde(default)]
    spp: u32,
}

async fn media_peaks(State(app): State<App>, Path(id): Path<String>, Query(q): Query<PeaksQuery>) -> Result<Response, Fail> {
    let (path, rate) = {
        let s = lock(&app.show);
        (s.media_file(&id, "peaks.bin")?, s.media_info(&id)?.sample_rate)
    };
    let levels = media::read_peaks(&path).ok_or_else(|| Fail::not_found("This file has no waveform"))?;
    let level = media::pick_level(&levels, q.spp.max(1)).ok_or_else(|| Fail::not_found("This file has no waveform"))?;
    Ok(bytes_reply("application/octet-stream", level.level(rate).to_bytes()))
}

/// Set, decode or clear a file's start timecode. Reading the file happens off the show lock.
async fn media_start_tc(
    State(app): State<App>,
    Client(c): Client,
    Path(id): Path<String>,
    Json(req): Json<StoreTc>,
) -> Reply<MediaInfo> {
    let text = req.start_tc.trim().to_string();
    let (dir, rate, mut info) = {
        let s = lock(&app.show);
        s.require_lease(&c)?;
        (s.media_file(&id, "")?, s.rate(), s.media_info(&id)?.clone())
    };
    let name = info.name.clone();
    let stopped = |_| Fail { code: 500, msg: "The file reader stopped unexpectedly".into() };
    let stamp = if text.is_empty() {
        tokio::task::spawn_blocking(move || {
            info.start_tc = None;
            info.start_tc_source = None;
            if let Some(original) = media::original_path(&dir) {
                video::read_stamp(&original, rate, &mut info);
            }
            Stamp::FileOwn { start_tc: info.start_tc, source: info.start_tc_source }
        })
        .await
        .map_err(stopped)?
    } else if let Some(n) = text.strip_prefix("ltc:") {
        let Some(channel) = n.trim().parse::<u32>().ok().filter(|n| *n >= 1) else {
            return Err(Fail::bad("Name the channel as ltc:<number>, counting from 1"));
        };
        let start = tokio::task::spawn_blocking(move || media::ltc_start_of_file(&dir.join("audio.f32"), channel - 1, rate))
            .await
            .map_err(stopped)?
            .map_err(|why| Fail::invalid(format!("{name}: {why}")))?;
        Stamp::Ltc { channel: channel - 1, start }
    } else {
        match ptz_core::Timecode::parse_at(&text, rate) {
            Some(tc) => Stamp::Typed(tc),
            None => return Err(Fail::invalid(format!("{text} is not a timecode at {} fps. Use HH:MM:SS:FF", rate.label()))),
        }
    };
    Ok(Json(lock(&app.show).stamp_media(&c, &id, stamp)?))
}

/// One proxy frame of a video, addressed by the timeline frame it shows.
async fn media_frame(State(app): State<App>, Path((id, file)): Path<(String, String)>) -> Result<Response, Fail> {
    let Some(frame) = file.strip_suffix(".jpg").and_then(|n| n.parse::<i64>().ok()) else {
        return Err(Fail::not_found("Frames are addressed as <frame number>.jpg"));
    };
    let path = {
        let s = lock(&app.show);
        let index =
            video::proxy_index(s.media_info(&id)?, frame, s.rate()).ok_or_else(|| Fail::not_found("This file has no picture"))?;
        video::frame_path(&s.media_file(&id, "")?, index)
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => Ok(bytes_reply("image/jpeg", bytes)),
        Err(_) => Err(Fail::not_found("That frame is missing from the media folder")),
    }
}

fn bytes_reply(content_type: &'static str, bytes: Vec<u8>) -> Response {
    ([(axum::http::header::CONTENT_TYPE, content_type)], bytes).into_response()
}

#[derive(Deserialize)]
struct Since {
    #[serde(default)]
    since: u64,
}

async fn get_log(State(app): State<App>, Query(q): Query<Since>) -> Json<Vec<LogRecord>> {
    Json(lock(&app.show).log_since(q.since))
}

async fn ws(State(app): State<App>, Client(c): Client, upgrade: WebSocketUpgrade) -> Response {
    upgrade.on_upgrade(move |socket| ws_loop(socket, app, c))
}

async fn send_msg(socket: &mut WebSocket, msg: &WsMsg) -> bool {
    match serde_json::to_string(msg) {
        Ok(text) => socket.send(Message::text(text)).await.is_ok(),
        Err(_) => true,
    }
}

/// Status at 30 Hz while anything moves and 5 Hz when idle, plus events as they happen.
async fn ws_loop(mut socket: WebSocket, app: App, client: String) {
    let mut rx = lock(&app.show).subscribe();
    let mut timer = tokio::time::interval(Duration::from_millis(33));
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_sent: Option<Instant> = None;
    let mut poke = true;
    loop {
        tokio::select! {
            _ = timer.tick() => {
                let status = {
                    let s = lock(&app.show);
                    let due = poke || s.is_playing() || last_sent.is_none_or(|t| t.elapsed() >= Duration::from_millis(200));
                    due.then(|| s.status(&client))
                };
                if let Some(status) = status {
                    poke = false;
                    last_sent = Some(Instant::now());
                    if !send_msg(&mut socket, &WsMsg::Status(Box::new(status))).await {
                        break;
                    }
                }
            }
            pushed = rx.recv() => match pushed {
                Ok(Push::Poke) | Err(RecvError::Lagged(_)) => poke = true,
                Ok(Push::Msg(msg)) => {
                    if !send_msg(&mut socket, &msg).await {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                // Any text from the client is a heartbeat.
                Some(Ok(Message::Text(_))) => lock(&app.show).touch(&client),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

pub fn router(app: App) -> Router {
    let api = Router::new()
        .route("/status", get(get_status))
        .route("/ws", get(ws))
        .route("/lease", post(post_lease).delete(delete_lease))
        .route("/panel", put(put_panel))
        .route("/songs", get(list_songs).post(create_song))
        .route("/songs/{id}", delete(delete_song))
        .route("/songs/{id}/load", post(load_song))
        .route("/songs/{id}/working", get(get_working).put(put_working))
        .route("/songs/{id}/save", post(save_song))
        .route("/songs/{id}/revert", post(revert_song))
        .route("/songs/{id}/copy", post(copy_song))
        .route("/songs/{id}/commit", post(commit_song))
        .route("/songs/{id}/revisions", get(get_revisions))
        .route("/transport/{action}", post(transport))
        .route("/estop", post(estop))
        .route("/estop/reset", post(estop_reset))
        .route("/project", get(get_project).put(put_project))
        .route("/inputs", get(get_inputs))
        .route("/presets", get(get_presets).post(post_preset))
        .route("/presets/sync", post(sync_presets))
        .route("/presets/{id}", delete(delete_preset))
        .route("/presets/{id}/thumb.jpg", get(preset_thumb))
        .route("/rig/jog", post(rig_jog))
        .route("/record/{clip}/refit", post(refit))
        .route("/media", get(list_media).post(upload_media))
        .route("/media/{id}", delete(delete_media))
        .route("/media/{id}/peaks", get(media_peaks))
        .route("/media/{id}/start_tc", post(media_start_tc))
        .route("/media/{id}/frames/{file}", get(media_frame))
        .route(
            "/stage/model",
            get(get_stage_model)
                .post(post_stage_model)
                .delete(delete_stage_model)
                .layer(axum::extract::DefaultBodyLimit::max(STAGE_MODEL_LIMIT)),
        )
        .route("/log", get(get_log));
    Router::new().nest(API, api).layer(axum::middleware::from_fn_with_state(app.clone(), gate)).with_state(app)
}

/// A server running on its own thread.
pub struct Running {
    pub addr: SocketAddr,
    pub show: Shared,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Block until the server stops (Ctrl-C).
    pub fn wait(mut self) {
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// How long a stopping server waits for the rigs to report standing still.
const HALT_ON_EXIT: Duration = Duration::from_secs(3);

/// Resolves when the server is told to stop: by its owner, Ctrl-C, or the signals
/// a terminal closing, `kill` and a system shutdown send.
async fn asked_to_stop(owner: tokio::sync::oneshot::Receiver<()>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let (mut term, mut hup) = match (signal(SignalKind::terminate()), signal(SignalKind::hangup())) {
            (Ok(term), Ok(hup)) => (term, hup),
            _ => {
                tokio::select! {
                    _ = owner => {}
                    _ = tokio::signal::ctrl_c() => {}
                }
                return;
            }
        };
        tokio::select! {
            _ = owner => {}
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
            _ = hup.recv() => {}
        }
    }
    #[cfg(not(unix))]
    tokio::select! {
        _ = owner => {}
        _ = tokio::signal::ctrl_c() => {}
    }
}

/// Open the data folder and serve it on `bind`.
pub fn start(cfg: &Config, bind: &str, access_key: Option<String>) -> Result<Running, String> {
    let addr: SocketAddr = bind.parse().map_err(|_| format!("{bind} is not an address like 127.0.0.1:8787"))?;
    if !addr.ip().is_loopback() && access_key.as_deref().unwrap_or("").is_empty() {
        return Err(format!("Refusing to listen on {addr} without --access-key. Anyone on the network could drive the rig."));
    }
    let listener = std::net::TcpListener::bind(addr).map_err(|e| format!("Could not listen on {addr}: {e}"))?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    let (show, _rx) = Show::open(cfg).map_err(|e| format!("Could not open {}: {e}", cfg.data.display()))?;
    let show: Shared = Arc::new(Mutex::new(show));
    spawn_tick(&show);
    let app = App { show: show.clone(), access_key: access_key.filter(|k| !k.is_empty()).map(Arc::new) };
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let halting = show.clone();
    let thread = std::thread::Builder::new()
        .name("http".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
                let shutdown = async move {
                    asked_to_stop(stop_rx).await;
                    // The rigs are stopped before anything else is wound down.
                    let stopped = tokio::task::spawn_blocking(move || halt_for_exit(&halting, HALT_ON_EXIT)).await;
                    if !matches!(stopped, Ok(true)) {
                        eprintln!("ptz-server: a rig did not confirm that it stopped. Use its own stop control");
                    }
                };
                let service = router(app).into_make_service();
                if let Err(e) = axum::serve(listener, service).with_graceful_shutdown(shutdown).await {
                    eprintln!("server error: {e}");
                }
            });
            rt.shutdown_timeout(Duration::from_millis(200));
        })
        .map_err(|e| e.to_string())?;
    Ok(Running { addr, show, stop: Some(stop_tx), thread: Some(thread) })
}
