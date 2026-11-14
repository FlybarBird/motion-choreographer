//! Plain files under the data directory. Every write goes to a temporary file
//! and is renamed into place, so a crash never leaves half a song.

use ptz_core::api::RevisionInfo;
use ptz_core::pose::{Preset, Project};
use ptz_core::song::Song;
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.{}.{n}.tmp", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
    write_atomic(path, &bytes)
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// UTC time as `2026-11-14T02:44:00Z`, without a date library.
pub fn now_rfc3339() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    rfc3339(secs)
}

pub fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // Civil-from-days (Howard Hinnant).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, (rem % 3600) / 60, rem % 60)
}

pub fn today() -> String {
    now_rfc3339()[..10].to_string()
}

/// Lowercase letters, digits and dashes, for ids and folder names.
pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "song".into()
    } else {
        out
    }
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Clone)]
pub struct Storage {
    root: PathBuf,
}

impl Storage {
    pub fn open(root: &Path) -> std::io::Result<Storage> {
        fs::create_dir_all(root.join("songs"))?;
        fs::create_dir_all(root.join("media"))?;
        fs::create_dir_all(root.join("log"))?;
        Ok(Storage { root: root.to_path_buf() })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn is_fresh(&self) -> bool {
        !self.root.join("project.json").exists()
    }

    pub fn load_project(&self) -> Project {
        read_json(&self.root.join("project.json")).unwrap_or_default()
    }

    /// The project as it is on disk: nothing when there is no file yet, and the reason when
    /// there is one that cannot be read.
    pub fn read_project(&self) -> Result<Option<Project>, String> {
        let bytes = match fs::read(self.root.join("project.json")) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.to_string()),
        };
        serde_json::from_slice(&bytes).map(Some).map_err(|e| e.to_string())
    }

    pub fn save_project(&self, project: &Project) -> std::io::Result<()> {
        write_json(&self.root.join("project.json"), project)
    }

    /// The song that was loaded when the server last ran.
    pub fn load_current(&self) -> Option<String> {
        read_json::<serde_json::Value>(&self.root.join("state.json"))?.get("current")?.as_str().map(|s| s.to_string())
    }

    pub fn save_current(&self, id: Option<&str>) -> std::io::Result<()> {
        write_json(&self.root.join("state.json"), &serde_json::json!({ "current": id }))
    }

    pub fn load_presets(&self) -> Vec<Preset> {
        read_json(&self.root.join("presets.json")).unwrap_or_default()
    }

    pub fn save_presets(&self, presets: &[Preset]) -> std::io::Result<()> {
        write_json(&self.root.join("presets.json"), &presets)
    }

    pub fn preset_thumbnail_path(&self, id: &str) -> Option<PathBuf> {
        safe_id(id).then(|| self.root.join("presets").join(format!("{id}.jpg")))
    }

    fn song_dir(&self, id: &str) -> Option<PathBuf> {
        safe_id(id).then(|| self.root.join("songs").join(id))
    }

    pub fn song_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = fs::read_dir(self.root.join("songs"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().join("working.json").exists())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        ids.sort();
        ids
    }

    pub fn unique_song_id(&self, name: &str) -> String {
        let base = slug(name);
        let existing = self.song_ids();
        if !existing.contains(&base) {
            return base;
        }
        (2..).map(|n| format!("{base}-{n}")).find(|id| !existing.contains(id)).unwrap_or(base)
    }

    pub fn load_working(&self, id: &str) -> Option<Song> {
        read_json(&self.song_dir(id)?.join("working.json"))
    }

    pub fn save_working(&self, song: &Song) -> std::io::Result<()> {
        let dir = self.song_dir(&song.id).ok_or_else(|| std::io::Error::other("bad song id"))?;
        write_json(&dir.join("working.json"), song)
    }

    pub fn load_draft(&self, id: &str) -> Option<Song> {
        read_json(&self.song_dir(id)?.join("draft.json"))
    }

    pub fn save_draft(&self, song: &Song) -> std::io::Result<()> {
        let dir = self.song_dir(&song.id).ok_or_else(|| std::io::Error::other("bad song id"))?;
        write_json(&dir.join("draft.json"), song)
    }

    pub fn revisions(&self, id: &str) -> Vec<RevisionInfo> {
        let Some(dir) = self.song_dir(id) else { return Vec::new() };
        let mut out: Vec<RevisionInfo> = fs::read_dir(dir.join("rev"))
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".meta.json"))
            .filter_map(|e| read_json(&e.path()))
            .collect();
        out.sort_by_key(|r| r.number);
        out
    }

    pub fn load_revision(&self, id: &str, number: u32) -> Option<Song> {
        read_json(&self.song_dir(id)?.join("rev").join(format!("{number:04}.json")))
    }

    pub fn latest_revision(&self, id: &str) -> Option<(RevisionInfo, Song)> {
        let info = self.revisions(id).pop()?;
        let song = self.load_revision(id, info.number)?;
        Some((info, song))
    }

    /// Write the next numbered revision. Existing revisions are never rewritten.
    pub fn commit(&self, song: &Song, by: &str) -> std::io::Result<RevisionInfo> {
        let dir = self.song_dir(&song.id).ok_or_else(|| std::io::Error::other("bad song id"))?.join("rev");
        fs::create_dir_all(&dir)?;
        // A commit that was cut short leaves a revision file without its record. Its number is
        // taken: the next commit goes after it, and the file is left as it is.
        let on_disk = fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_string_lossy().split('.').next().and_then(|n| n.parse::<u32>().ok()))
            .max();
        let recorded = self.revisions(&song.id).last().map(|r| r.number);
        let number = on_disk.max(recorded).map(|n| n + 1).unwrap_or(1);
        let path = dir.join(format!("{number:04}.json"));
        if path.exists() {
            return Err(std::io::Error::other("revision already exists"));
        }
        let bytes = serde_json::to_vec_pretty(song).map_err(std::io::Error::other)?;
        let info = RevisionInfo { number, sha256: sha256_hex(&bytes), committed_at: now_rfc3339(), committed_by: by.to_string() };
        write_atomic(&path, &bytes)?;
        write_json(&dir.join(format!("{number:04}.meta.json")), &info)?;
        Ok(info)
    }

    pub fn delete_song(&self, id: &str) -> std::io::Result<()> {
        match self.song_dir(id) {
            Some(dir) if dir.exists() => fs::remove_dir_all(dir),
            _ => Ok(()),
        }
    }

    pub fn pass_path(&self, song_id: &str, pass: &str) -> Option<PathBuf> {
        (safe_id(pass)).then(|| self.song_dir(song_id).map(|d| d.join("passes").join(format!("{pass}.json"))))?
    }

    pub fn media_root(&self) -> PathBuf {
        self.root.join("media")
    }

    pub fn media_dir(&self, id: &str) -> Option<PathBuf> {
        safe_id(id).then(|| self.root.join("media").join(id))
    }

    pub fn log_dir(&self) -> PathBuf {
        self.root.join("log")
    }

    /// The imported stage model, in the compact mesh form the app uploads.
    pub fn stage_model_path(&self) -> PathBuf {
        self.root.join("stage").join("model.ptzmesh")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_and_slug() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_794_624_240), "2026-11-14T02:44:00Z");
        assert_eq!(rfc3339(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(slug("Walk-In"), "walk-in");
        assert_eq!(slug("  Opening Number!  "), "opening-number");
        assert_eq!(slug("///"), "song");
        assert!(!safe_id("../etc"));
    }

    /// A crash or a full disk between the two files of a commit leaves the revision without its record.
    #[test]
    fn a_revision_left_without_its_record_does_not_block_the_next_commit() {
        let dir = std::env::temp_dir().join(format!("ptz-storage-orphan-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let st = Storage::open(&dir).unwrap();
        let mut song = Song::new("finale", "Finale", "01:00:00:00", 100);
        assert_eq!(st.commit(&song, "luke").unwrap().number, 1);
        let rev = dir.join("songs/finale/rev");
        fs::copy(rev.join("0001.json"), rev.join("0002.json")).unwrap();
        let orphan = fs::read(rev.join("0002.json")).unwrap();

        song.length_frames = 200;
        let next = st.commit(&song, "luke").unwrap();
        assert_eq!(next.number, 3, "the number the orphan holds is not used again");
        assert_eq!(fs::read(rev.join("0002.json")).unwrap(), orphan, "and the orphan is not rewritten");
        assert_eq!(st.latest_revision("finale").map(|(info, s)| (info.number, s.length_frames)), Some((3, 200)));
        assert_eq!(st.revisions("finale").iter().map(|r| r.number).collect::<Vec<_>>(), [1, 3]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn revisions_are_numbered_and_never_rewritten() {
        let dir = std::env::temp_dir().join(format!("ptz-storage-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let st = Storage::open(&dir).unwrap();
        let mut song = Song::new("walk-in", "Walk-In", "01:00:00:00", 100);
        st.save_working(&song).unwrap();
        let r1 = st.commit(&song, "luke").unwrap();
        song.length_frames = 200;
        let r2 = st.commit(&song, "luke").unwrap();
        assert_eq!((r1.number, r2.number), (1, 2));
        assert_ne!(r1.sha256, r2.sha256);
        assert_eq!(st.load_revision("walk-in", 1).unwrap().length_frames, 100);
        let bytes = fs::read(dir.join("songs/walk-in/rev/0001.json")).unwrap();
        assert_eq!(sha256_hex(&bytes), r1.sha256);
        assert_eq!(st.latest_revision("walk-in").unwrap().0.number, 2);
        assert_eq!(st.song_ids(), vec!["walk-in".to_string()]);
        assert_eq!(st.unique_song_id("Walk In"), "walk-in-2");
        let _ = fs::remove_dir_all(&dir);
    }
}
