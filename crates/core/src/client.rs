//! Blocking HTTP client for the show server, shared by the app, the probe and
//! the tests so they all speak the API the same way.

use crate::api::*;
use crate::pose::{Preset, Project};
use crate::transport::Action;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// Shared by every connection in a station, including the panel's dedicated jog link.
static CAMERA_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, PartialEq, Debug)]
pub enum ApiErr {
    /// The server answered and said no. `code` 409 is a policy refusal.
    Refused { code: u16, msg: String },
    /// The server could not be reached, or the answer made no sense.
    Network(String),
}

impl ApiErr {
    pub fn message(&self) -> &str {
        match self {
            ApiErr::Refused { msg, .. } | ApiErr::Network(msg) => msg,
        }
    }

    pub fn code(&self) -> Option<u16> {
        match self {
            ApiErr::Refused { code, .. } => Some(*code),
            ApiErr::Network(_) => None,
        }
    }
}

impl std::fmt::Display for ApiErr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

pub type ApiResult<T> = Result<T, ApiErr>;

#[derive(Clone)]
pub struct Api {
    base: String,
    pub client_id: String,
    key: Option<String>,
    agent: ureq::Agent,
}

/// `127.0.0.1:8787` or `http://host:8787/` to `http://host:8787`.
pub fn normalize_base(input: &str) -> String {
    let s = input.trim().trim_end_matches('/');
    if s.starts_with("http://") || s.starts_with("https://") {
        s.to_string()
    } else {
        format!("http://{s}")
    }
}

impl Api {
    pub fn new(base: &str, client_id: &str, key: Option<&str>) -> Api {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_connect(Some(Duration::from_secs(3)))
            .timeout_global(Some(Duration::from_secs(120)))
            .build();
        Api {
            base: normalize_base(base),
            client_id: client_id.to_string(),
            key: key.filter(|k| !k.is_empty()).map(|k| k.to_string()),
            agent: ureq::Agent::new_with_config(config),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{API}{path}", self.base)
    }

    /// WebSocket address for the status stream.
    pub fn ws_url(&self) -> String {
        let host = self.base.trim_start_matches("http://").trim_start_matches("https://");
        let mut url = format!("ws://{host}{API}/ws?client={}", self.client_id);
        if let Some(k) = &self.key {
            url.push_str("&key=");
            url.push_str(k);
        }
        url
    }

    fn finish(result: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> ApiResult<ureq::http::Response<ureq::Body>> {
        let mut resp = result.map_err(|e| ApiErr::Network(format!("Cannot reach the server: {e}")))?;
        let code = resp.status().as_u16();
        if (200..300).contains(&code) {
            return Ok(resp);
        }
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        let msg = serde_json::from_str::<ApiError>(&text).map(|e| e.error).unwrap_or_else(|_| {
            if text.trim().is_empty() {
                format!("The server answered {code}")
            } else {
                text.trim().to_string()
            }
        });
        Err(ApiErr::Refused { code, msg })
    }

    fn json<T: DeserializeOwned>(resp: ApiResult<ureq::http::Response<ureq::Body>>) -> ApiResult<T> {
        let mut resp = resp?;
        resp.body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_json::<T>()
            .map_err(|e| ApiErr::Network(format!("The server's answer was not understood: {e}")))
    }

    fn with_headers<B>(&self, mut req: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
        req = req.header(HEADER_CLIENT, &self.client_id);
        if let Some(k) = &self.key {
            req = req.header(HEADER_KEY, k);
        }
        req
    }

    pub fn get<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        Self::json(Self::finish(self.with_headers(self.agent.get(self.url(path))).call()))
    }

    pub fn get_bytes(&self, path: &str) -> ApiResult<Vec<u8>> {
        let mut resp = Self::finish(self.with_headers(self.agent.get(self.url(path))).call())?;
        resp.body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_to_vec()
            .map_err(|e| ApiErr::Network(format!("Download failed: {e}")))
    }

    pub fn post<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> ApiResult<T> {
        Self::json(Self::finish(self.with_headers(self.agent.post(self.url(path))).send_json(body)))
    }

    pub fn post_empty<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        Self::json(Self::finish(self.with_headers(self.agent.post(self.url(path))).send_empty()))
    }

    pub fn put<B: Serialize, T: DeserializeOwned>(&self, path: &str, body: &B) -> ApiResult<T> {
        Self::json(Self::finish(self.with_headers(self.agent.put(self.url(path))).send_json(body)))
    }

    pub fn delete<T: DeserializeOwned>(&self, path: &str) -> ApiResult<T> {
        Self::json(Self::finish(self.with_headers(self.agent.delete(self.url(path))).call()))
    }

    /// Upload a file as the raw request body.
    pub fn upload<T: DeserializeOwned>(&self, path: &str, file: std::fs::File) -> ApiResult<T> {
        let req = self.with_headers(self.agent.post(self.url(path))).content_type("application/octet-stream");
        Self::json(Self::finish(req.send(file)))
    }

    pub fn post_bytes<T: DeserializeOwned>(&self, path: &str, bytes: &[u8]) -> ApiResult<T> {
        let req = self.with_headers(self.agent.post(self.url(path))).content_type("application/octet-stream");
        Self::json(Self::finish(req.send(bytes)))
    }

    // ---- typed calls ----

    pub fn status(&self) -> ApiResult<Status> {
        self.get("/status")
    }

    pub fn take_lease(&self, name: &str, force: bool) -> ApiResult<Status> {
        self.post("/lease", &LeaseRequest { name: name.to_string(), force })
    }

    pub fn release_lease(&self) -> ApiResult<Status> {
        self.delete("/lease")
    }

    pub fn act(&self, action: Action, req: &TransportRequest) -> ApiResult<Status> {
        match action {
            Action::Estop => self.post_empty("/estop"),
            Action::ResetEstop => self.post_empty("/estop/reset"),
            other => self.post(&format!("/transport/{}", other.key()), req),
        }
    }

    pub fn songs(&self) -> ApiResult<Vec<SongSummary>> {
        self.get("/songs")
    }

    pub fn new_song(&self, name: &str, start_tc: Option<&str>) -> ApiResult<WorkingSong> {
        self.post("/songs", &NewSong { name: name.to_string(), start_tc: start_tc.map(|s| s.to_string()) })
    }

    pub fn load_song(&self, id: &str) -> ApiResult<WorkingSong> {
        self.post_empty(&format!("/songs/{id}/load"))
    }

    pub fn working(&self, id: &str) -> ApiResult<WorkingSong> {
        self.get(&format!("/songs/{id}/working"))
    }

    pub fn put_working(&self, put: &PutWorking) -> ApiResult<WorkingSong> {
        self.put(&format!("/songs/{}/working", put.song.id), put)
    }

    pub fn save(&self, id: &str) -> ApiResult<WorkingSong> {
        self.post_empty(&format!("/songs/{id}/save"))
    }

    /// Save the draft of every song that has unsaved changes.
    pub fn save_all(&self) -> ApiResult<SavedAll> {
        self.post_empty("/save")
    }

    pub fn revert(&self, id: &str) -> ApiResult<WorkingSong> {
        self.post_empty(&format!("/songs/{id}/revert"))
    }

    pub fn copy_song(&self, id: &str, name: &str) -> ApiResult<WorkingSong> {
        self.post(&format!("/songs/{id}/copy"), &NewSong { name: name.to_string(), start_tc: None })
    }

    pub fn commit(&self, id: &str) -> ApiResult<RevisionInfo> {
        self.post_empty(&format!("/songs/{id}/commit"))
    }

    /// Confirm editing the loaded song while the show is live. Opening a page does not do this.
    pub fn confirm_live_edit(&self, id: &str) -> ApiResult<Status> {
        self.post_empty(&format!("/songs/{id}/live_edit"))
    }

    /// Leave live edit. The loaded song is refused again while the show stays live.
    pub fn cancel_live_edit(&self, id: &str) -> ApiResult<Status> {
        self.delete(&format!("/songs/{id}/live_edit"))
    }

    pub fn delete_song(&self, id: &str) -> ApiResult<Vec<SongSummary>> {
        self.delete(&format!("/songs/{id}"))
    }

    pub fn project(&self) -> ApiResult<Project> {
        self.get("/project")
    }

    pub fn put_project(&self, project: &Project) -> ApiResult<Project> {
        self.put("/project", project)
    }

    /// Store a camera's position and zoom as the pose a new song starts from.
    pub fn store_default(&self, req: &StoreDefault) -> ApiResult<Project> {
        self.post("/cameras/default", req)
    }

    pub fn presets(&self) -> ApiResult<Vec<Preset>> {
        self.get("/presets")
    }

    /// Read one camera's stored presets into the preset list.
    pub fn sync_presets(&self, camera: &str) -> ApiResult<Vec<Preset>> {
        self.post_empty(&format!("/presets/sync?camera={}", encode_query(camera)))
    }

    pub fn set_camera_armed(&self, camera: &str, armed: bool) -> ApiResult<Status> {
        self.put(
            &format!("/cameras/{}/armed", encode_query(camera)),
            &CameraArmRequest { armed, sequence: Some(CAMERA_SEQUENCE.fetch_add(1, Ordering::Relaxed)) },
        )
    }

    pub fn jog(&self, camera: &str, jog: &Jog) -> ApiResult<Status> {
        self.post(
            "/rig/jog",
            &JogRequest {
                camera: Some(camera.to_string()),
                sequence: Some(CAMERA_SEQUENCE.fetch_add(1, Ordering::Relaxed)),
                jog: *jog,
            },
        )
    }

    /// Report what a hardware panel controller is doing. Needs no lease.
    pub fn put_panel(&self, state: &PanelState) -> ApiResult<PanelState> {
        self.put("/panel", state)
    }

    pub fn media(&self) -> ApiResult<Vec<MediaInfo>> {
        self.get("/media")
    }

    /// Import a file. `name` is the file name shown in the Media tab; its extension picks the decoder.
    pub fn upload_media(&self, name: &str, file: std::fs::File) -> ApiResult<MediaInfo> {
        self.upload(&format!("/media?name={}", encode_query(name)), file)
    }

    pub fn delete_media(&self, id: &str) -> ApiResult<Vec<MediaInfo>> {
        self.delete(&format!("/media/{id}"))
    }

    /// Replace the stage model with a mesh file. `name` is the file it was read from.
    pub fn upload_stage_model(
        &self,
        name: &str,
        unit: crate::camera::Unit,
        up: crate::camera::UpAxis,
        mesh_file: &[u8],
    ) -> ApiResult<Project> {
        let word = |v: serde_json::Value| v.as_str().unwrap_or_default().to_string();
        let (unit, up) = (word(serde_json::json!(unit)), word(serde_json::json!(up)));
        self.post_bytes(&format!("/stage/model?name={}&unit={unit}&up={up}", encode_query(name)), mesh_file)
    }

    /// The stage model's mesh file.
    pub fn stage_model(&self) -> ApiResult<Vec<u8>> {
        self.get_bytes("/stage/model")
    }

    pub fn delete_stage_model(&self) -> ApiResult<Project> {
        self.delete("/stage/model")
    }

    /// The waveform overview level that suits `samples_per_pixel`.
    pub fn peaks(&self, id: &str, samples_per_pixel: u32) -> ApiResult<PeakLevel> {
        let bytes = self.get_bytes(&format!("/media/{id}/peaks?spp={samples_per_pixel}"))?;
        PeakLevel::from_bytes(&bytes).ok_or_else(|| ApiErr::Network("The server's waveform was not understood".into()))
    }
}

/// Percent-encode a value for a query string.
pub fn encode_query(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
            other => format!("%{other:02X}"),
        })
        .collect()
}
