//! PTZ Director shared domain.
//!
//! The authoritative show clock and transport live on the server. This crate
//! holds the pure rules both sides share so the operator UI never offers an
//! action the server would reject.

pub mod audio;
pub mod easing;
pub mod model;
pub mod timecode;
pub mod transport;

pub use audio::{decode_audio, waveform_peaks, DecodedAudio, Waveform};
pub use easing::{ease, EaseKind};
pub use model::*;
pub use timecode::{FrameRate, Timecode, TimecodeError};
pub use transport::{
    TransportAction, TransportButtons, TransportError, TransportGuidance, TransportMode,
    TransportPhase, TransportPolicy, TransportSnapshot,
};

pub const API_VERSION: &str = "v1";
pub const DEFAULT_BACKEND_URL: &str = "http://127.0.0.1:8787";
