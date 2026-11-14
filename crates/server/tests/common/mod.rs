#![allow(dead_code)]

use ptz_core::api::{MediaInfo, Status, TransportRequest};
use ptz_core::client::{Api, ApiErr};
use ptz_core::transport::Action;
use ptz_server::engine::Config;
use ptz_server::http::{self, Running};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static COUNTER: AtomicU32 = AtomicU32::new(0);

pub fn temp_dir(name: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("ptz-test-{name}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

pub struct TestServer {
    pub dir: PathBuf,
    pub url: String,
    pub key: Option<String>,
    pub running: Option<Running>,
    keep: bool,
}

impl TestServer {
    pub fn start(name: &str) -> TestServer {
        Self::start_with(name, |_| {})
    }

    pub fn start_with(name: &str, tweak: impl FnOnce(&mut Config)) -> TestServer {
        Self::launch(temp_dir(name), None, tweak)
    }

    pub fn start_keyed(name: &str, key: &str) -> TestServer {
        Self::launch(temp_dir(name), Some(key.to_string()), |_| {})
    }

    pub fn reopen(dir: &Path) -> TestServer {
        Self::launch(dir.to_path_buf(), None, |_| {})
    }

    fn launch(dir: PathBuf, key: Option<String>, tweak: impl FnOnce(&mut Config)) -> TestServer {
        let mut cfg = Config::new(&dir);
        // Tests never open a sound card: the renderer runs on the monotonic clock.
        cfg.audio = ptz_server::engine::AudioChoice::Off;
        tweak(&mut cfg);
        let running = http::start(&cfg, "127.0.0.1:0", key.clone()).expect("server starts");
        TestServer { dir, url: running.url(), key, running: Some(running), keep: false }
    }

    pub fn client(&self, id: &str) -> Api {
        Api::new(&self.url, id, self.key.as_deref())
    }
}

impl TestServer {
    /// Stop the server and keep its data folder for a restart.
    pub fn stop_keeping_data(mut self) -> PathBuf {
        self.keep = true;
        self.dir.clone()
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.running.take();
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

pub fn wait_for(api: &Api, mut done: impl FnMut(&Status) -> bool) -> Status {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let st = api.status().expect("status");
        if done(&st) {
            return st;
        }
        assert!(Instant::now() < deadline, "timed out waiting; last status: {st:?}");
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// The message of a refusal with the given HTTP status; anything else fails the test.
pub fn refused(result: Result<impl std::fmt::Debug, ApiErr>, code: u16) -> String {
    match result {
        Err(ApiErr::Refused { code: got, msg }) if got == code => msg,
        other => panic!("expected a {code} refusal, got {other:?}"),
    }
}

pub fn do_act(api: &Api, action: Action) -> Status {
    api.act(action, &TransportRequest::default()).unwrap_or_else(|e| panic!("{action:?}: {e:?}"))
}

/// Write `bytes` to a fixture file and import it under `name`.
pub fn import(api: &Api, t: &TestServer, name: &str, bytes: &[u8]) -> Result<MediaInfo, ApiErr> {
    let dir = t.dir.join("fixtures");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    api.upload_media(name, std::fs::File::open(path).unwrap())
}
