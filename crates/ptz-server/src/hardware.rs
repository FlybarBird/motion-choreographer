//! Hardware adapter traits + simulator implementations.
//!
//! Real FR7 (VISCA over IP) and AutoPod adapters are intentionally stubbed.
//! Do not invent unverified device protocol claims — wire them behind these
//! traits once hardware is available.

use async_trait::async_trait;
use parking_lot::Mutex;
use ptz_core::RigPose;
use std::sync::Arc;
use tracing::{info, warn};

/// Sony ILME-FR7 pan/tilt/zoom/focus control.
#[async_trait]
pub trait Fr7Device: Send + Sync {
    async fn connect(&self) -> anyhow::Result<()>;
    async fn goto_ptzf(&self, pose: RigPose, live: bool) -> anyhow::Result<()>;
    async fn estop(&self) -> anyhow::Result<()>;
    fn name(&self) -> &str;
    fn is_simulator(&self) -> bool;
}

/// Polecam AutoPod pedestal elevation.
#[async_trait]
pub trait AutoPodDevice: Send + Sync {
    async fn connect(&self) -> anyhow::Result<()>;
    async fn goto_elevation(&self, meters: f64, live: bool) -> anyhow::Result<()>;
    async fn estop(&self) -> anyhow::Result<()>;
    fn name(&self) -> &str;
    fn is_simulator(&self) -> bool;
}

#[derive(Default)]
struct SimState {
    pose: RigPose,
    connected: bool,
}

/// In-process FR7 simulator — records commanded poses, never talks to a camera.
pub struct Fr7Simulator {
    state: Mutex<SimState>,
}

impl Fr7Simulator {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(SimState::default()),
        }
    }

    pub fn current_pose(&self) -> RigPose {
        self.state.lock().pose
    }
}

#[async_trait]
impl Fr7Device for Fr7Simulator {
    async fn connect(&self) -> anyhow::Result<()> {
        self.state.lock().connected = true;
        info!("FR7 simulator connected");
        Ok(())
    }

    async fn goto_ptzf(&self, pose: RigPose, live: bool) -> anyhow::Result<()> {
        if !live {
            // Preview path must never move physical equipment; sim still updates
            // internal pose so the program monitor can render.
            self.state.lock().pose = pose;
            return Ok(());
        }
        self.state.lock().pose = pose;
        Ok(())
    }

    async fn estop(&self) -> anyhow::Result<()> {
        warn!("FR7 simulator E-Stop");
        Ok(())
    }

    fn name(&self) -> &str {
        "FR7 Simulator"
    }

    fn is_simulator(&self) -> bool {
        true
    }
}

pub struct AutoPodSimulator {
    elevation_m: Mutex<f64>,
}

impl AutoPodSimulator {
    pub fn new() -> Self {
        Self {
            elevation_m: Mutex::new(1.4),
        }
    }

    pub fn elevation(&self) -> f64 {
        *self.elevation_m.lock()
    }
}

#[async_trait]
impl AutoPodDevice for AutoPodSimulator {
    async fn connect(&self) -> anyhow::Result<()> {
        info!("AutoPod simulator connected");
        Ok(())
    }

    async fn goto_elevation(&self, meters: f64, live: bool) -> anyhow::Result<()> {
        let _ = live; // sim always tracks; live flag reserved for real adapter
        *self.elevation_m.lock() = meters;
        Ok(())
    }

    async fn estop(&self) -> anyhow::Result<()> {
        warn!("AutoPod simulator E-Stop");
        Ok(())
    }

    fn name(&self) -> &str {
        "AutoPod Simulator"
    }

    fn is_simulator(&self) -> bool {
        true
    }
}

/// Placeholder for a real VISCA-over-IP FR7 adapter.
///
/// TODO(hardware): implement Sony ILME-FR7 VISCA over IP after capturing
/// verified command sequences on a physical unit. Do not guess opcode tables.
pub struct Fr7ViscaTodo {
    _endpoint: String,
}

impl Fr7ViscaTodo {
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            _endpoint: endpoint.into(),
        }
    }
}

#[async_trait]
impl Fr7Device for Fr7ViscaTodo {
    async fn connect(&self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "TODO(hardware): FR7 VISCA adapter not implemented — needs physical ILME-FR7"
        ))
    }

    async fn goto_ptzf(&self, _pose: RigPose, _live: bool) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("TODO(hardware): FR7 VISCA goto_ptzf"))
    }

    async fn estop(&self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("TODO(hardware): FR7 VISCA estop"))
    }

    fn name(&self) -> &str {
        "FR7 VISCA (TODO)"
    }

    fn is_simulator(&self) -> bool {
        false
    }
}

/// Placeholder for a real AutoPod HTTP/OSC adapter.
///
/// TODO(hardware): integrate with verified AutoPod RX/TX protocol (see
/// FlybarBird/autopod-osc-bridge for TX pedal bridging reference) once pedestal
/// absolute positioning commands are confirmed on hardware.
pub struct AutoPodRealTodo;

#[async_trait]
impl AutoPodDevice for AutoPodRealTodo {
    async fn connect(&self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!(
            "TODO(hardware): AutoPod real adapter not implemented"
        ))
    }

    async fn goto_elevation(&self, _meters: f64, _live: bool) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("TODO(hardware): AutoPod goto_elevation"))
    }

    async fn estop(&self) -> anyhow::Result<()> {
        Err(anyhow::anyhow!("TODO(hardware): AutoPod estop"))
    }

    fn name(&self) -> &str {
        "AutoPod Real (TODO)"
    }

    fn is_simulator(&self) -> bool {
        false
    }
}

pub type SharedFr7 = Arc<dyn Fr7Device>;
pub type SharedAutoPod = Arc<dyn AutoPodDevice>;
