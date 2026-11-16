//! Talking to a panel directly, without a show: for `ptz-panel map` and
//! `ptz-panel learn`.

use crate::codec::{self, Color, FromPanel, Lit, Text, Trigger};
use crate::link::{resolve, Conn};
use crate::topology::Topology;
use std::collections::VecDeque;
use std::net::TcpStream;
use std::time::{Duration, Instant};

pub struct Session {
    conn: Conn,
    pub model: String,
    pub name: String,
    pub topology: Option<Topology>,
    events: VecDeque<(u32, Trigger)>,
    heard: bool,
}

impl Session {
    /// Dial the panel and ask what it is. Waits up to `wait` for it to describe itself.
    pub fn open(addr: &str, wait: Duration) -> Result<Session, String> {
        let to = resolve(addr).ok_or_else(|| format!("{addr} is not an address this computer can find"))?;
        let stream = TcpStream::connect_timeout(&to, Duration::from_secs(3)).map_err(|e| {
            format!("No panel answers at {addr}: {e}. Check the address, and that the panel is in Raw Panel mode")
        })?;
        let conn = Conn::new(stream).map_err(|e| format!("The connection to {addr} could not be set up: {e}"))?;
        let mut s =
            Session { conn, model: String::new(), name: String::new(), topology: None, events: VecDeque::new(), heard: false };
        s.send(&["list".to_string()])?;
        let opened = Instant::now();
        let mut asked = false;
        while s.topology.is_none() && opened.elapsed() < wait {
            s.pump()?;
            if !asked && (s.heard || opened.elapsed() > Duration::from_millis(500)) {
                asked = true;
                s.send(&["SleepTimer=0".to_string(), "WakeUp!".to_string(), "PanelTopology?".to_string(), "map".to_string()])?;
            }
        }
        if !s.heard {
            return Err(format!(
                "{addr} took the connection but said nothing. Check that the panel is in Raw Panel mode and that nothing else is connected to it"
            ));
        }
        Ok(s)
    }

    fn send(&mut self, lines: &[String]) -> Result<(), String> {
        self.conn.send(lines).map_err(|e| format!("The panel connection closed: {e}"))
    }

    fn pump(&mut self) -> Result<(), String> {
        let lines = self.conn.lines().map_err(|e| format!("The panel connection closed: {e}"))?;
        let mut out = Vec::new();
        for line in lines {
            self.heard = true;
            match codec::parse(&line) {
                FromPanel::Event { hwc, trigger, .. } => self.events.push_back((hwc, trigger)),
                FromPanel::Ping => out.push("ack".to_string()),
                FromPanel::List => out.push("\nActivePanel=1".to_string()),
                FromPanel::Sleeping(true) => out.extend(["SleepTimer=0".to_string(), "WakeUp!".to_string()]),
                FromPanel::Model(m) => self.model = m,
                FromPanel::Name(n) => self.name = n,
                FromPanel::Topology(json) => match Topology::parse(&json) {
                    Ok(t) => self.topology = Some(t),
                    Err(e) => return Err(format!("The panel's description of itself was not understood: {e}")),
                },
                _ => {}
            }
        }
        self.send(&out)
    }

    /// The next thing done on the panel, or nothing within `wait`.
    pub fn next_event(&mut self, wait: Duration) -> Result<Option<(u32, Trigger)>, String> {
        let until = Instant::now() + wait;
        loop {
            if let Some(event) = self.events.pop_front() {
                return Ok(Some(event));
            }
            if Instant::now() >= until {
                return Ok(None);
            }
            self.pump()?;
        }
    }

    /// Forget anything done on the panel so far.
    pub fn drain(&mut self) -> Result<(), String> {
        self.pump()?;
        self.events.clear();
        Ok(())
    }

    pub fn show(&mut self, hwc: u32, lit: Lit, color: Color, text: &Text) -> Result<(), String> {
        self.send(&[codec::hwc_color(hwc, color), codec::hwc_mode(hwc, lit), codec::hwc_text(hwc, text)])
    }

    pub fn clear(&mut self) -> Result<(), String> {
        self.send(&["Clear".to_string()])
    }
}
