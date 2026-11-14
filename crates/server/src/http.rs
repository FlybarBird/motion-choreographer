//! REST for commands and edits, WebSocket for status. All under `/api/v1`.

use crate::engine::{spawn_tick, Config, Fail, Push, Shared, Show};
use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Path, Query, Request, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
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

async fn get_project(State(app): State<App>) -> Json<Project> {
    Json(lock(&app.show).project.clone())
}

async fn put_project(State(app): State<App>, Client(c): Client, Json(project): Json<Project>) -> Reply<Project> {
    Ok(Json(lock(&app.show).put_project(&c, project)?))
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

async fn sync_presets(State(app): State<App>, Client(c): Client) -> Reply<Vec<Preset>> {
    Ok(Json(lock(&app.show).sync_presets(&c)?))
}

async fn rig_jog(State(app): State<App>, Client(c): Client, Json(jog): Json<Jog>) -> Reply<Status> {
    let mut s = lock(&app.show);
    s.jog(&c, jog)?;
    Ok(Json(s.status(&c)))
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
        .route("/presets", get(get_presets).post(post_preset))
        .route("/presets/sync", post(sync_presets))
        .route("/presets/{id}", delete(delete_preset))
        .route("/rig/jog", post(rig_jog))
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
    let thread = std::thread::Builder::new()
        .name("http".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("runtime");
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
                let shutdown = async move {
                    tokio::select! {
                        _ = stop_rx => {}
                        _ = tokio::signal::ctrl_c() => {}
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
