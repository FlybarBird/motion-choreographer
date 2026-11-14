//! Real hardware: an FR7 for pan, tilt, zoom and focus, and an AutoPod+ for
//! elevation, presented to the show as one rig.

pub mod autopod;
pub mod cgi;
pub mod fake;
pub mod fr7;
pub mod visca;

use crate::rig::{DevicePreset, Drive, EstopLine, PoseReading, RigAdapter, Simulator};
use autopod::{AutoPod, PodConfig};
use fr7::{Fr7, Fr7Config};
use ptz_core::api::AdapterStatus;
use ptz_core::pose::{Axis, AxisTable, Pose, Preset, RigConfig, RigKind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

static STOPS_ONLY: AtomicBool = AtomicBool::new(false);

/// From here on, this process sends the devices questions and stops and nothing that could
/// move them. For a tool that is being ended in the middle of a move. There is no way back.
pub fn stops_only() {
    STOPS_ONLY.store(true, Ordering::SeqCst);
}

pub(crate) fn only_stops_now() -> bool {
    STOPS_ONLY.load(Ordering::SeqCst)
}

/// Each device is read and commanded twenty times a second.
pub const PERIOD: Duration = Duration::from_millis(50);

/// `host` with `port` added when it names none.
pub fn with_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:{port}")
    }
}

/// Sleeps to an even beat, without drifting when a cycle runs long.
pub struct Loop {
    period: Duration,
    next: Instant,
}

impl Loop {
    pub fn new(period: Duration) -> Loop {
        Loop { period, next: Instant::now() + period }
    }

    pub fn wait(&mut self) {
        let now = Instant::now();
        if self.next > now {
            std::thread::sleep(self.next - now);
        }
        self.next = (self.next + self.period).max(Instant::now());
    }
}

pub struct HardwareRig {
    fr7: Option<Fr7>,
    pod: Option<AutoPod>,
    query_delay_s: f64,
    /// Where an axis with no device was last told to be, so it reads as in place.
    told: Pose,
    sync_elevation: f64,
}

impl HardwareRig {
    pub fn start(rig: &RigConfig, axes: &AxisTable) -> HardwareRig {
        let fr7 = (!rig.fr7_host.trim().is_empty()).then(|| Fr7::start(Fr7Config::of(rig), *axes));
        let pod = (!rig.autopod_host.trim().is_empty()).then(|| AutoPod::start(PodConfig::of(rig), axes.elevation));
        HardwareRig { fr7, pod, query_delay_s: rig.query_delay_ms.max(0.0) / 1000.0, told: Pose::home(), sync_elevation: 0.0 }
    }
}

impl HardwareRig {
    /// Put both devices on a show's E-Stop line.
    pub fn on_line(self, line: &EstopLine) -> HardwareRig {
        if let Some(fr7) = &self.fr7 {
            fr7.on_line(line);
        }
        if let Some(pod) = &self.pod {
            pod.on_line(line);
        }
        self
    }
}

impl RigAdapter for HardwareRig {
    fn status(&self) -> Vec<AdapterStatus> {
        let mut out = Vec::new();
        out.extend(self.fr7.as_ref().map(|f| f.status()));
        out.extend(self.pod.as_ref().map(|p| p.status()));
        out
    }

    fn problem(&self) -> Option<String> {
        if self.fr7.is_none() && self.pod.is_none() {
            return Some("The rig is set to hardware but no FR7 or AutoPod+ address is entered".into());
        }
        self.fr7.as_ref().and_then(|f| f.problem()).or_else(|| self.pod.as_ref().and_then(|p| p.problem()))
    }

    fn pose(&self) -> Pose {
        self.reading().pose
    }

    fn reading(&self) -> PoseReading {
        let mut reading = PoseReading::all_at(self.told, Instant::now());
        if let Some(f) = &self.fr7 {
            let (head, times) = f.head();
            reading.pose = Pose { pan: head.pan, tilt: head.tilt, zoom: head.zoom, focus: head.focus, ..reading.pose };
            reading.at[Axis::Pan.index()] = times.pan_tilt;
            reading.at[Axis::Tilt.index()] = times.pan_tilt;
            reading.at[Axis::Zoom.index()] = times.zoom;
            reading.at[Axis::Focus.index()] = times.focus;
        }
        if let Some(p) = &self.pod {
            let (mm, read_at) = p.elevation();
            reading.pose.elevation = mm;
            reading.at[Axis::Elevation.index()] = read_at;
        }
        reading
    }

    fn tick(&mut self, _dt: f64, axes: &AxisTable) {
        if let Some(f) = &self.fr7 {
            f.set_axes(axes);
        }
        if let Some(p) = &self.pod {
            p.set_limits(&axes.elevation);
        }
    }

    fn drive(&mut self, cmd: &Drive) {
        self.told = cmd.target;
        if let Some(f) = &self.fr7 {
            f.drive(cmd);
        }
        if let Some(p) = &self.pod {
            p.drive(cmd);
        }
    }

    fn stop(&mut self) {
        if let Some(f) = &self.fr7 {
            f.stop();
        }
        if let Some(p) = &self.pod {
            p.stop();
        }
    }

    fn is_moving(&self) -> bool {
        self.fr7.as_ref().is_some_and(|f| f.is_moving()) || self.pod.as_ref().is_some_and(|p| p.is_moving())
    }

    fn presets(&mut self) -> Result<Vec<Preset>, String> {
        Err("The camera's presets are read by recalling each one. Use Sync from rig".into())
    }

    fn begin_preset_sync(&mut self) -> Result<(), String> {
        let Some(f) = &self.fr7 else {
            return Err("No FR7 is set up to read presets from".into());
        };
        self.sync_elevation = self.pod.as_ref().map(|p| p.elevation().0).unwrap_or(self.told.elevation);
        f.begin_sync()
    }

    fn take_preset_sync(&mut self) -> Option<Result<Vec<DevicePreset>, String>> {
        let mut result = self.fr7.as_ref()?.take_sync()?;
        if let Ok(list) = &mut result {
            // The camera knows nothing of the pedestal: its presets keep the height the rig is at.
            for p in list {
                p.preset.pose.elevation = self.sync_elevation;
            }
        }
        Some(result)
    }

    fn preset_sync_progress(&self) -> Option<String> {
        self.fr7.as_ref().and_then(|f| f.sync_progress())
    }

    fn query_delay_s(&self) -> f64 {
        if self.query_delay_s > 0.0 {
            return self.query_delay_s;
        }
        let fr7 = self.fr7.as_ref().map(|f| f.round_trip_s()).unwrap_or(0.0);
        let pod = self.pod.as_ref().map(|p| p.round_trip_s()).unwrap_or(0.0);
        fr7.max(pod) / 2.0
    }
}

/// The rig one camera's settings ask for.
pub fn build(rig: &RigConfig, axes: &AxisTable, line: &EstopLine) -> Box<dyn RigAdapter> {
    match rig.kind {
        RigKind::Simulator => Box::new(Simulator::new()),
        RigKind::Hardware => Box::new(HardwareRig::start(rig, axes).on_line(line)),
    }
}
