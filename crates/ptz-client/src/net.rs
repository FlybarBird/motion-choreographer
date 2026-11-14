//! Blocking HTTP + background WebSocket live feed.

use anyhow::{anyhow, Context, Result};
use parking_lot::Mutex;
use ptz_core::{ServerEvent, ShowStatus, Song, API_VERSION};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

#[derive(Clone)]
pub struct Backend {
    base: String,
    agent: ureq::Agent,
}

impl Backend {
    pub fn new(base_url: &str) -> Self {
        let base = base_url.trim_end_matches('/').to_string();
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .build()
            .into();
        Self { base, agent }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    pub fn status(&self) -> Result<ShowStatus> {
        let r = self.agent.get(&self.url("/api/v1/status")).call()?;
        Ok(r.into_body().read_json()?)
    }

    pub fn songs(&self) -> Result<Vec<Song>> {
        let r = self.agent.get(&self.url("/api/v1/songs")).call()?;
        Ok(r.into_body().read_json()?)
    }

    pub fn presets(&self) -> Result<Vec<ptz_core::RigPreset>> {
        let r = self.agent.get(&self.url("/api/v1/presets")).call()?;
        Ok(r.into_body().read_json()?)
    }

    pub fn select_song(&self, id: &str) -> Result<Song> {
        let r = self
            .agent
            .post(&self.url(&format!("/api/v1/songs/{id}/select")))
            .send_empty()?;
        Ok(r.into_body().read_json()?)
    }

    pub fn put_song(&self, song: &Song) -> Result<Song> {
        let r = self
            .agent
            .put(&self.url(&format!("/api/v1/songs/{}", song.id)))
            .send_json(song)?;
        Ok(r.into_body().read_json()?)
    }

    pub fn add_move(
        &self,
        song_id: &str,
        preset_id: &str,
        start_frame: i64,
        duration_frames: i64,
    ) -> Result<Song> {
        let r = self
            .agent
            .post(&self.url(&format!("/api/v1/songs/{song_id}/moves")))
            .send_json(serde_json::json!({
                "preset_id": preset_id,
                "start_frame": start_frame,
                "duration_frames": duration_frames,
            }))?;
        Ok(r.into_body().read_json()?)
    }

    pub fn transport_with_body(&self, action: &str) -> Result<ShowStatus, TransportReject> {
        let url = self.url(&format!("/api/v1/transport/{action}"));
        match self.agent.post(&url).send_empty() {
            Ok(r) => Ok(r.into_body().read_json().map_err(|e| TransportReject {
                status: 500,
                message: e.to_string(),
            })?),
            Err(ureq::Error::StatusCode(code)) => Err(TransportReject {
                status: code,
                message: format!("HTTP {code} — action rejected by server"),
            }),
            Err(e) => Err(TransportReject {
                status: 0,
                message: e.to_string(),
            }),
        }
    }

    pub fn upload_audio(&self, song_id: &str, path: &std::path::Path) -> Result<Song> {
        let data = std::fs::read(path).context("read audio file")?;
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("audio.wav");
        let boundary = "----ptzdirectorboundary";
        let mut body = Vec::new();
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(&data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        let r = self
            .agent
            .post(&self.url(&format!("/api/v1/songs/{song_id}/audio")))
            .header(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .send(&body)?;
        Ok(r.into_body().read_json()?)
    }
}

#[derive(Debug)]
pub struct TransportReject {
    pub status: u16,
    pub message: String,
}

impl std::fmt::Display for TransportReject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.status)
    }
}

pub struct LiveFeed {
    pub latest: Arc<Mutex<Option<ShowStatus>>>,
    pub connected: Arc<Mutex<bool>>,
    pub last_error: Arc<Mutex<Option<String>>>,
}

impl LiveFeed {
    pub fn start(base_url: &str) -> Self {
        let latest = Arc::new(Mutex::new(None));
        let connected = Arc::new(Mutex::new(false));
        let last_error = Arc::new(Mutex::new(None));
        let base = base_url.trim_end_matches('/').to_string();
        let latest_c = latest.clone();
        let connected_c = connected.clone();
        let err_c = last_error.clone();
        thread::spawn(move || loop {
            let ws_url = base
                .replacen("http://", "ws://", 1)
                .replacen("https://", "wss://", 1)
                + "/api/v1/ws";
            match tungstenite::client::connect(&ws_url) {
                Ok((mut socket, _)) => {
                    *connected_c.lock() = true;
                    *err_c.lock() = None;
                    while let Ok(msg) = socket.read() {
                        if let Ok(text) = msg.to_text() {
                            if let Ok(ev) = serde_json::from_str::<ServerEvent>(text) {
                                if let ServerEvent::Status(status) = ev {
                                    if status.api_version != API_VERSION {
                                        *err_c.lock() = Some(format!(
                                            "API mismatch: server {} client {API_VERSION}",
                                            status.api_version
                                        ));
                                    }
                                    *latest_c.lock() = Some(status);
                                }
                            }
                        }
                    }
                    *connected_c.lock() = false;
                }
                Err(e) => {
                    *connected_c.lock() = false;
                    *err_c.lock() = Some(e.to_string());
                    thread::sleep(Duration::from_millis(750));
                }
            }
            thread::sleep(Duration::from_millis(500));
        });
        Self {
            latest,
            connected,
            last_error,
        }
    }
}

pub fn remember_backend_url(url: &str) -> Result<()> {
    let dir = dirs::config_dir()
        .ok_or_else(|| anyhow!("no config dir"))?
        .join("ptz-director");
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("backend_url.txt"), url)?;
    Ok(())
}

pub fn load_backend_url() -> String {
    dirs::config_dir()
        .and_then(|d| std::fs::read_to_string(d.join("ptz-director/backend_url.txt")).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| ptz_core::DEFAULT_BACKEND_URL.to_string())
}
