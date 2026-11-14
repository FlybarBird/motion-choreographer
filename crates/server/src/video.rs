//! Video files. The server never decodes video itself: `ffmpeg` pulls the
//! audio out and writes the small proxy frames the reference monitor shows.

use crate::media::{decode_file, Decoded};
use ptz_core::api::MediaInfo;
use ptz_core::time::FrameRate;
use std::path::Path;
use std::process::Command;

pub const NEEDS_FFMPEG: &str = "video files need ffmpeg, which was not found. Install it with: brew install ffmpeg";

pub fn tool(name: &str) -> Command {
    // Homebrew's folder is not on the PATH of an app started from Finder.
    for dir in ["/opt/homebrew/bin", "/usr/local/bin"] {
        let path = Path::new(dir).join(name);
        if path.exists() {
            return Command::new(path);
        }
    }
    Command::new(name)
}

/// The file's audio, decoded through a temporary WAV.
pub fn extract_audio(original: &Path, dir: &Path) -> Result<Decoded, String> {
    let wav = dir.join("extracted.wav");
    let out = tool("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-i"])
        .arg(original)
        .args(["-vn", "-map", "0:a:0", "-c:a", "pcm_s16le"])
        .arg(&wav)
        .output()
        .map_err(|_| NEEDS_FFMPEG.to_string())?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.lines().last().unwrap_or("ffmpeg could not read it").trim().to_string();
        return Err(format!("the video has no usable audio track ({why})"));
    }
    let decoded = decode_file(&wav, "wav");
    let _ = std::fs::remove_file(&wav);
    decoded
}

/// Work done once the audio is in place: start timecode and proxy frames.
pub fn after_ingest(_original: &Path, _dir: &Path, _decoded: &Decoded, _rate: FrameRate, _info: &mut MediaInfo) {}
