//! Video files. The server never decodes video itself: `ffmpeg` pulls the
//! audio out and writes the small proxy frames the reference monitor shows,
//! and `ffprobe` reads the timecode track.

use crate::media::{self, decode_file, Decoded};
use ptz_core::api::{MediaInfo, MediaKind};
use ptz_core::time::{FrameRate, Timecode};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const NEEDS_FFMPEG: &str = "video files need ffmpeg, which was not found. Install it with: brew install ffmpeg";

/// Proxy frames are this wide; the height follows the picture's shape.
pub const PROXY_WIDTH: u32 = 480;

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

pub fn have_tools() -> bool {
    ["ffmpeg", "ffprobe"].iter().all(|t| tool(t).arg("-version").output().is_ok_and(|o| o.status.success()))
}

/// One value from `ffprobe`, or nothing when it is absent or the tool is missing.
fn probe(original: &Path, entries: &str, streams: Option<&str>) -> Option<String> {
    let mut cmd = tool("ffprobe");
    cmd.args(["-v", "error"]);
    if let Some(s) = streams {
        cmd.args(["-select_streams", s]);
    }
    let out = cmd.args(["-show_entries", entries, "-of", "default=nw=1:nk=1"]).arg(original).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.lines().map(str::trim).find(|l| !l.is_empty()).map(str::to_string)
}

fn seconds_of(original: &Path) -> Option<f64> {
    probe(original, "format=duration", None)?.parse::<f64>().ok().filter(|d| d.is_finite() && *d > 0.0)
}

/// Frames per second of the picture, from a fraction such as `30000/1001`.
fn picture_fps(original: &Path) -> Option<f64> {
    let text = probe(original, "stream=r_frame_rate", Some("v:0"))?;
    let (n, d) = text.split_once('/').unwrap_or((text.as_str(), "1"));
    let fps = n.parse::<f64>().ok()? / d.parse::<f64>().ok()?;
    (fps.is_finite() && fps > 0.0).then_some(fps)
}

/// The file's audio, decoded through a temporary WAV. A file with a picture
/// and no sound gets a silent track of the picture's length.
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
        let _ = std::fs::remove_file(&wav);
        if let (Some(seconds), Some(_)) = (seconds_of(original), picture_fps(original)) {
            let rate = 48_000u32;
            return Ok(Decoded { channels: 1, rate, samples: vec![0.0; (seconds * rate as f64).round() as usize] });
        }
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.lines().last().unwrap_or("ffmpeg could not read it").trim().to_string();
        return Err(format!("the file has no picture or sound that ffmpeg can read ({why})"));
    }
    let decoded = decode_file(&wav, "wav");
    let _ = std::fs::remove_file(&wav);
    decoded
}

/// The start timecode on the file's timecode track or in its tags.
pub fn container_timecode(original: &Path) -> Option<String> {
    probe(original, "stream_tags=timecode:format_tags=timecode", None)
}

/// A container's timecode restated at the project rate. The label is kept when
/// it exists at that rate; a frame number the rate does not have is scaled.
pub fn timecode_at(tag: &str, picture_fps: Option<f64>, rate: FrameRate) -> Option<Timecode> {
    if let Some(tc) = Timecode::parse_at(tag, rate) {
        return Some(tc);
    }
    let mut tc = Timecode::parse(tag)?;
    let from = picture_fps.map(|f| f.round()).filter(|f| *f >= 1.0)?;
    tc.f = ((tc.f as f64 * rate.nominal() as f64 / from).floor() as u8).min(rate.nominal() as u8 - 1);
    tc.drop_frame = rate.drop_frame();
    if rate.drop_frame() && tc.s == 0 && tc.f < 2 && !tc.m.is_multiple_of(10) {
        tc.f = 2;
    }
    tc.is_valid(rate).then_some(tc)
}

pub fn frames_dir(dir: &Path) -> PathBuf {
    dir.join("frames")
}

pub fn frame_path(dir: &Path, index: i64) -> PathBuf {
    frames_dir(dir).join(format!("{index:06}.jpg"))
}

/// Write one small JPEG per timeline frame. Returns how many were written.
pub fn make_proxies(original: &Path, dir: &Path, rate: FrameRate) -> Result<i64, String> {
    let frames = frames_dir(dir);
    let _ = std::fs::remove_dir_all(&frames);
    std::fs::create_dir_all(&frames).map_err(|e| format!("could not write to the media folder: {e}"))?;
    let filter = format!("fps={}/{},scale={PROXY_WIDTH}:-2", rate.num(), rate.den());
    let out = tool("ffmpeg")
        .args(["-v", "error", "-nostdin", "-y", "-i"])
        .arg(original)
        .args(["-an", "-map", "0:v:0", "-vf", &filter, "-q:v", "6", "-start_number", "0"])
        .arg(frames.join("%06d.jpg"))
        .output()
        .map_err(|_| NEEDS_FFMPEG.to_string())?;
    let count = std::fs::read_dir(&frames).map(|d| d.flatten().count() as i64).unwrap_or(0);
    if count == 0 {
        let _ = std::fs::remove_dir_all(&frames);
        let why = String::from_utf8_lossy(&out.stderr);
        return Err(why.lines().last().unwrap_or("ffmpeg wrote no frames").trim().to_string());
    }
    Ok(count)
}

/// The proxy that shows timeline frame `frame` of the media, at the current project rate.
pub fn proxy_index(info: &MediaInfo, frame: i64, rate: FrameRate) -> Option<i64> {
    if info.proxy_frames <= 0 {
        return None;
    }
    let made_at = info.proxy_rate.unwrap_or(rate);
    let index = if made_at == rate { frame } else { (frame as f64 * made_at.fps() / rate.fps()).floor() as i64 };
    Some(index.clamp(0, info.proxy_frames - 1))
}

/// The start timecode the file itself carries: a broadcast WAV's time
/// reference, or a video's timecode track.
pub fn read_stamp(original: &Path, rate: FrameRate, info: &mut MediaInfo) {
    if let Some(samples) = media::bext_time_reference(original) {
        info.start_samples = Some(samples);
        info.start_tc_source = Some(media::STAMP_BEXT.into());
        media::restate(info, rate);
    }
    if info.kind != MediaKind::Video {
        return;
    }
    if let Some(tc) = container_timecode(original).and_then(|tag| timecode_at(&tag, picture_fps(original), rate)) {
        info.start_tc = Some(tc.to_string());
        info.start_tc_source = Some(media::STAMP_QUICKTIME.into());
    }
}

/// Work done once the audio is in place: start timecode and proxy frames.
pub fn after_ingest(original: &Path, dir: &Path, rate: FrameRate, info: &mut MediaInfo) {
    read_stamp(original, rate, info);
    if info.kind != MediaKind::Video {
        return;
    }
    // A file whose picture cannot be read still imports for its sound.
    if let Ok(count) = make_proxies(original, dir, rate) {
        info.proxy_frames = count;
        info.proxy_rate = Some(rate);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_container_timecode_is_kept_or_scaled_to_the_project_rate() {
        let at = |tag: &str, fps: Option<f64>, rate| timecode_at(tag, fps, rate).map(|t| t.to_string());
        assert_eq!(at("01:00:10:00", Some(25.0), FrameRate::F25).as_deref(), Some("01:00:10:00"));
        assert_eq!(at("01:00:10;00", Some(29.97), FrameRate::F2997Df).as_deref(), Some("01:00:10;00"));
        // Frame 28 of 30 does not exist at 25: it becomes the same point in the second.
        assert_eq!(at("01:00:10:28", Some(30.0), FrameRate::F25).as_deref(), Some("01:00:10:23"));
        assert_eq!(at("01:00:10:28", None, FrameRate::F25), None);
        assert_eq!(at("soon", Some(25.0), FrameRate::F25), None);
    }

    #[test]
    fn a_proxy_is_addressed_by_timeline_frame_whatever_rate_it_was_made_at() {
        let mut info = media::info_for(
            "v",
            "v.mov",
            MediaKind::Video,
            &Decoded { channels: 1, rate: 48_000, samples: vec![0.0; 48_000] },
            FrameRate::F25,
        );
        assert_eq!(proxy_index(&info, 3, FrameRate::F25), None);
        info.proxy_frames = 25;
        info.proxy_rate = Some(FrameRate::F25);
        assert_eq!(proxy_index(&info, 3, FrameRate::F25), Some(3));
        assert_eq!(proxy_index(&info, 400, FrameRate::F25), Some(24));
        assert_eq!(proxy_index(&info, -2, FrameRate::F25), Some(0));
        // The project moved to 30 fps: timeline frame 15 is half a second in, proxy 12.
        assert_eq!(proxy_index(&info, 15, FrameRate::F30), Some(12));
    }
}
