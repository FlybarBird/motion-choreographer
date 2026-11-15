//! The control panel the server runs for itself. The panel is one more station of the
//! server, reached through its own API like any other; this only decides whether one
//! is running, and at which address, from the project's settings.

use crate::engine::Shared;
use crate::http::lock;
use ptz_core::api::PanelHost;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the project's panel settings are looked at.
const LOOK: Duration = Duration::from_millis(200);
/// How long before a panel that could not be started is tried again.
const TRY_AGAIN: Duration = Duration::from_secs(5);

/// What a controller needs to reach this server and to read its control map.
#[derive(Clone)]
pub struct Reach {
    /// This server, as a station on the same machine reaches it.
    pub server: String,
    pub access_key: Option<String>,
    /// The data folder: `panel.json` in it is the control map, when there is one.
    pub data: PathBuf,
}

impl Reach {
    pub fn new(addr: SocketAddr, access_key: Option<String>, data: PathBuf) -> Reach {
        Reach { server: local_url(addr), access_key: access_key.filter(|k| !k.is_empty()), data }
    }

    pub fn map_file(&self) -> PathBuf {
        self.data.join("panel.json")
    }

    /// Start a controller for the panel at `address`. A map that cannot be read is an error:
    /// the panel is not run on a layout its owner did not mean.
    pub fn start(&self, address: &str) -> Result<ptz_panel::Running, String> {
        let mut options = ptz_panel::Options::new(&self.server);
        options.access_key = self.access_key.clone();
        options.panel = Some(ptz_panel::with_port(address));
        options.map = ptz_panel::map::PanelMap::load(&self.map_file())?;
        ptz_panel::start(options)
    }
}

/// The address a station on this machine uses for a server listening on `addr`.
pub fn local_url(addr: SocketAddr) -> String {
    let local: IpAddr = match addr.ip() {
        ip if !ip.is_unspecified() => ip,
        IpAddr::V4(_) => Ipv4Addr::LOCALHOST.into(),
        IpAddr::V6(_) => Ipv6Addr::LOCALHOST.into(),
    };
    format!("http://{}", SocketAddr::new(local, addr.port()))
}

/// The thread that keeps the running panel in step with the project.
pub struct Follower {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Follower {
    /// Stop following, and stop the controller if one is running.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Follower {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Run the panel the project asks for, and change with it: started when it is switched on,
/// stopped when it is switched off, restarted when its address changes.
pub fn follow(show: Shared, reach: Reach) -> Follower {
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let thread = std::thread::Builder::new()
        .name("panel-host".into())
        .spawn(move || {
            // The address a controller is running for, and the controller.
            let mut running: Option<(String, ptz_panel::Running)> = None;
            // The address that could not be started, and when to try it again.
            let mut failed: Option<(String, Instant)> = None;
            while !stopped.load(Ordering::SeqCst) {
                let wanted = lock(&show).project.panel.wanted().map(ptz_panel::with_port);
                let have = running.as_ref().map(|(address, _)| address.clone());
                let waiting =
                    failed.as_ref().is_some_and(|(address, again)| Some(address) == wanted.as_ref() && Instant::now() < *again);
                if wanted != have && !waiting {
                    // The lock is not held while the controller stops: it lets go of its lease through the API.
                    if let Some((_, mut controller)) = running.take() {
                        controller.stop();
                        lock(&show).forget_panel();
                    }
                    failed = None;
                    let host = wanted.map(|address| match reach.start(&address) {
                        Ok(controller) => {
                            running = Some((address.clone(), controller));
                            PanelHost { address, from_command_line: false, problem: None }
                        }
                        Err(why) => {
                            failed = Some((address.clone(), Instant::now() + TRY_AGAIN));
                            PanelHost { address, from_command_line: false, problem: Some(why) }
                        }
                    });
                    lock(&show).set_panel_host(host);
                }
                std::thread::sleep(LOOK);
            }
            if let Some((_, mut controller)) = running.take() {
                controller.stop();
            }
        })
        .ok();
    Follower { stop, thread }
}
