//! Control of the show from a SKAARHOJ panel speaking the Raw Panel protocol.
//!
//! Three layers: `codec` and `link` speak to the panel, `topology` and `map`
//! say which control plays which role, and `controller` is a station of the
//! show server like the app, holding the edit lease under its own name.
//! `fake` stands in for the panel itself.

pub mod codec;
pub mod controller;
pub mod fake;
pub mod link;
pub mod map;
pub mod tool;
pub mod topology;

use link::{Event, PanelLink, Shared};
use map::PanelMap;
use ptz_core::client::Api;
use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

pub const DEFAULT_STATION: &str = "Panel";
/// The port a SKAARHOJ panel listens on in Raw Panel mode.
pub const RAW_PANEL_PORT: u16 = 9923;

#[derive(Clone)]
pub struct Options {
    /// The show server: `http://127.0.0.1:8787`.
    pub server: String,
    pub access_key: Option<String>,
    /// The name the panel holds the edit lease under.
    pub station: String,
    /// A panel to connect to: `192.168.10.99:9923`.
    pub panel: Option<String>,
    /// An address to wait on for a panel that connects by itself.
    pub listen: Option<String>,
    pub map: PanelMap,
    /// How long the edit lease is kept after the panel is lost.
    pub lease_grace: Duration,
    /// Say nothing on standard error.
    pub quiet: bool,
}

impl Options {
    pub fn new(server: &str) -> Options {
        Options {
            server: server.to_string(),
            access_key: None,
            station: DEFAULT_STATION.to_string(),
            panel: None,
            listen: None,
            map: PanelMap::ptz_pro(),
            lease_grace: Duration::from_secs(30),
            quiet: false,
        }
    }
}

/// The address with the Raw Panel port when none was given.
pub fn with_port(addr: &str) -> String {
    if addr.parse::<std::net::SocketAddr>().is_ok() {
        return addr.to_string();
    }
    match addr.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') && port.parse::<u16>().is_ok() => addr.to_string(),
        Some(_) if addr.matches(':').count() > 1 => format!("[{addr}]:{RAW_PANEL_PORT}"),
        _ => format!("{addr}:{RAW_PANEL_PORT}"),
    }
}

/// A running controller. Dropping it stops it.
pub struct Running {
    shared: Arc<Shared>,
    threads: Vec<std::thread::JoinHandle<()>>,
    /// Where a panel can connect, when the controller listens.
    pub listening: Option<SocketAddr>,
}

impl Running {
    pub fn stop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop();
    }
}

fn client_id() -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    format!("panel-{:x}-{nanos:x}", std::process::id())
}

/// Start the controller. Fails only on what will never work: no panel address
/// at all, or a listen address that cannot be bound. A panel or a server that is
/// not there yet is waited for.
pub fn start(options: Options) -> Result<Running, String> {
    if options.panel.is_none() && options.listen.is_none() {
        return Err("Give the panel's address with --panel, or an address to wait on with --listen".into());
    }
    if options.station.trim().is_empty() {
        return Err("The station name cannot be empty".into());
    }
    let listener = match &options.listen {
        Some(addr) => Some(TcpListener::bind(addr).map_err(|e| format!("Cannot listen for a panel on {addr}: {e}"))?),
        None => None,
    };
    let listening = listener.as_ref().and_then(|l| l.local_addr().ok());
    let id = client_id();
    let api = Api::new(&options.server, &id, options.access_key.as_deref());
    let shared = Shared::new();
    let (events, queue) = std::sync::mpsc::channel::<Event>();
    let (presses, pressed) = std::sync::mpsc::channel::<()>();
    let mut threads = Vec::new();
    let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| {
        std::thread::Builder::new()
            .name(format!("panel-{name}"))
            .spawn(f)
            .map_err(|e| format!("Cannot start the panel {name} thread: {e}"))
    };

    {
        let (api, shared, events) = (api.clone(), shared.clone(), events.clone());
        threads.push(spawn("estop", Box::new(move || link::run_estop(api, pressed, shared, events)))?);
    }
    {
        let (api, shared, events) = (api.clone(), shared.clone(), events.clone());
        threads.push(spawn("hand", Box::new(move || link::run_hand(api, shared, events)))?);
    }
    {
        let (api, shared, events) = (api.clone(), shared.clone(), events.clone());
        threads.push(spawn("status", Box::new(move || link::run_status(api, shared, events)))?);
    }
    {
        let link = PanelLink { dial: options.panel.clone(), listener, map: options.map.clone() };
        let (shared, events) = (shared.clone(), events.clone());
        threads.push(spawn("link", Box::new(move || link::run_panel(link, shared, events, presses)))?);
    }
    {
        let settings = controller::Settings {
            api,
            station: options.station.trim().to_string(),
            map: options.map,
            lease_grace: options.lease_grace,
            quiet: options.quiet,
        };
        let shared = shared.clone();
        threads.push(spawn("controller", Box::new(move || controller::Controller::new(settings, shared).run(queue)))?);
    }
    Ok(Running { shared, threads, listening })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_raw_panel_port_is_added_only_when_none_is_given() {
        assert_eq!(with_port("192.168.10.99"), "192.168.10.99:9923");
        assert_eq!(with_port("192.168.10.99:9924"), "192.168.10.99:9924");
        assert_eq!(with_port("panel.local"), "panel.local:9923");
        assert_eq!(with_port("panel.local:9000"), "panel.local:9000");
        assert_eq!(with_port("fe80::1"), "[fe80::1]:9923");
        assert_eq!(with_port("[fe80::1]:9000"), "[fe80::1]:9000");
    }
}
