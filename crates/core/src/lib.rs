//! Shared model for Motion Choreographer. Pure functions: no async, no file or
//! network access, no clock reads. The app links it for instant feedback while
//! dragging; the server links it to decide what is legal.

pub mod api;
pub mod camera;
pub mod chase;
#[cfg(feature = "client")]
pub mod client;
pub mod codes;
pub mod compile;
pub mod edit;
pub mod eval;
pub mod fit;
pub mod lens;
pub mod ltc;
pub mod pose;
pub mod song;
pub mod time;
pub mod transport;

pub use camera::{CameraConfig, FIRST_CAMERA};
pub use pose::{Axis, Pose, Preset, Project, Units};
pub use song::Song;
pub use time::{FrameRate, Timecode};
