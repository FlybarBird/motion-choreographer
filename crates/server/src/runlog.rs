//! Append-only run log. Each record carries the SHA-256 of the previous one,
//! so an edited or deleted line is detectable.

use crate::storage::{now_rfc3339, sha256_hex, today};
use ptz_core::api::LogRecord;
use serde_json::Value;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

fn record_hash(seq: u64, time: &str, kind: &str, data: &Value, prev: &str) -> String {
    let body = format!("{seq}|{time}|{kind}|{data}|{prev}");
    sha256_hex(body.as_bytes())
}

pub struct RunLog {
    dir: PathBuf,
    file: Option<File>,
    day: String,
    seq: u64,
    prev: String,
    recent: Vec<LogRecord>,
    /// Files with records the disk may not hold yet, on their way to the thread that sees to it.
    unflushed: Sender<File>,
}

/// What puts a file's records on the disk itself.
pub type Flush = Box<dyn Fn(&File) + Send>;

/// The show loop writes to the log while it is steering the rigs, and must never wait for a disk.
/// A record is handed to the operating system as it is written, which is enough to outlive the
/// server; this thread then has it put on the disk, which is what outlives the power.
fn start_flusher(flush: Flush) -> Sender<File> {
    let (unflushed, files) = channel::<File>();
    let run = move || {
        while let Ok(mut file) = files.recv() {
            // One flush covers every record written before it. Only the newest handle of each
            // file is needed, and a file changes only at midnight.
            while let Ok(newer) = files.try_recv() {
                let same = match (file.metadata(), newer.metadata()) {
                    (Ok(a), Ok(b)) => same_file(&a, &b),
                    _ => false,
                };
                if !same {
                    flush(&file);
                }
                file = newer;
            }
            flush(&file);
        }
    };
    let _ = std::thread::Builder::new().name("run-log".into()).spawn(run);
    unflushed
}

#[cfg(unix)]
fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    a.dev() == b.dev() && a.ino() == b.ino()
}

#[cfg(not(unix))]
fn same_file(_: &fs::Metadata, _: &fs::Metadata) -> bool {
    false
}

fn log_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .collect();
    files.sort();
    files
}

fn read_records(path: &Path) -> Vec<Result<LogRecord, String>> {
    let Ok(f) = File::open(path) else { return Vec::new() };
    BufReader::new(f)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<LogRecord>(&l).map_err(|e| e.to_string()))
        .collect()
}

impl RunLog {
    /// Open the log, continuing the chain from the last record on disk.
    pub fn open(dir: &Path) -> RunLog {
        RunLog::open_flushing_with(
            dir,
            Box::new(|file| {
                let _ = file.sync_data();
            }),
        )
    }

    /// As `open`, with the step that puts records on the disk given by the caller.
    pub fn open_flushing_with(dir: &Path, flush: Flush) -> RunLog {
        let _ = fs::create_dir_all(dir);
        // A crash can leave half a line at the end of the newest file. Drop it.
        if let Some(last) = log_files(dir).pop() {
            if let Ok(text) = fs::read(&last) {
                if !text.is_empty() && !text.ends_with(b"\n") {
                    let keep = text.iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
                    let _ = fs::write(&last, &text[..keep]);
                }
            }
        }
        let mut seq = 0;
        let mut prev = GENESIS.to_string();
        let mut recent = Vec::new();
        for path in log_files(dir) {
            for r in read_records(&path).into_iter().flatten() {
                seq = r.seq;
                prev = r.hash.clone();
                recent.push(r);
                if recent.len() > 500 {
                    recent.remove(0);
                }
            }
        }
        RunLog { dir: dir.to_path_buf(), file: None, day: String::new(), seq, prev, recent, unflushed: start_flusher(flush) }
    }

    /// Append a record. It is with the operating system before this returns, and on the disk
    /// as soon as the disk allows, without the caller waiting for it.
    pub fn append(&mut self, kind: &str, data: Value) -> LogRecord {
        let time = now_rfc3339();
        let day = today();
        if self.file.is_none() || self.day != day {
            self.day = day.clone();
            self.file = OpenOptions::new().create(true).append(true).open(self.dir.join(format!("{day}.jsonl"))).ok();
        }
        self.seq += 1;
        let hash = record_hash(self.seq, &time, kind, &data, &self.prev);
        let record = LogRecord { seq: self.seq, time, kind: kind.to_string(), data, prev: self.prev.clone(), hash };
        if let Some(f) = &mut self.file {
            if let Ok(line) = serde_json::to_string(&record) {
                let _ = writeln!(f, "{line}");
                match f.try_clone() {
                    Ok(handle) => {
                        let _ = self.unflushed.send(handle);
                    }
                    // Without a second handle to pass on, the record is still put on the disk.
                    Err(_) => {
                        let _ = f.sync_data();
                    }
                }
            }
        }
        self.prev = record.hash.clone();
        self.recent.push(record.clone());
        if self.recent.len() > 500 {
            self.recent.remove(0);
        }
        record
    }

    pub fn since(&self, seq: u64) -> Vec<LogRecord> {
        self.recent.iter().filter(|r| r.seq > seq).cloned().collect()
    }
}

/// Walk every log file in order and check the hash chain.
/// Returns the number of records, or a description of the first break.
pub fn verify(dir: &Path) -> Result<u64, String> {
    let mut prev = GENESIS.to_string();
    let mut expect_seq = 1u64;
    let mut count = 0;
    for path in log_files(dir) {
        let name = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        for (line, r) in read_records(&path).into_iter().enumerate() {
            let r = r.map_err(|e| format!("{name} line {}: not a log record ({e})", line + 1))?;
            if r.seq != expect_seq {
                return Err(format!(
                    "{name} line {}: record {} follows record {}; a line is missing",
                    line + 1,
                    r.seq,
                    expect_seq - 1
                ));
            }
            if r.prev != prev {
                return Err(format!("{name} line {}: record {} does not chain to the one before it", line + 1, r.seq));
            }
            if record_hash(r.seq, &r.time, &r.kind, &r.data, &r.prev) != r.hash {
                return Err(format!("{name} line {}: record {} was modified", line + 1, r.seq));
            }
            prev = r.hash;
            expect_seq += 1;
            count += 1;
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ptz-log-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn writing_a_record_does_not_wait_for_a_slow_disk() {
        let dir = temp("slow-disk");
        let flushes = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counted = flushes.clone();
        // A disk that takes a fifth of a second over every flush.
        let mut log = RunLog::open_flushing_with(
            &dir,
            Box::new(move |file| {
                std::thread::sleep(std::time::Duration::from_millis(200));
                let _ = file.sync_data();
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }),
        );
        // What twelve cameras write in one pass of the show loop.
        let began = std::time::Instant::now();
        for camera in 0..12 {
            log.append("pose", json!({ "camera": camera }));
        }
        let took = began.elapsed();
        assert!(took < std::time::Duration::from_millis(150), "twelve records held the show loop for {took:?}");
        // Every record is already where a reader, and a restarted server, finds it.
        assert_eq!(verify(&dir), Ok(12));
        // And the disk is asked to hold them, without one flush for each.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while flushes.load(std::sync::atomic::Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        let flushed = flushes.load(std::sync::atomic::Ordering::SeqCst);
        assert!((1..=2).contains(&flushed), "{flushed} flushes for one burst of records");
        drop(log);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn chain_verifies_and_continues_across_restarts() {
        let dir = temp("chain");
        let mut log = RunLog::open(&dir);
        log.append("server_start", json!({}));
        log.append("transport", json!({"action": "play", "by": "luke"}));
        drop(log);
        let mut log = RunLog::open(&dir);
        let r = log.append("estop", json!({"by": "watcher"}));
        assert_eq!(r.seq, 3);
        assert_eq!(verify(&dir), Ok(3));
        assert_eq!(log.since(1).len(), 2);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn numbers_read_back_as_they_were_hashed() {
        let dir = temp("floats");
        let mut log = RunLog::open(&dir);
        let mut x = 0.123_456_789_012_345_67_f64;
        for _ in 0..400 {
            x = (x * 997.0 + 0.1).fract() * 1e-3;
            log.append("pose", json!({ "pan": x * 1e5, "error": -x, "tiny": x * 1e-12, "big": x * 1e15 }));
        }
        drop(log);
        assert_eq!(verify(&dir), Ok(400));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_modified_or_deleted_line_is_detected() {
        let dir = temp("tamper");
        let mut log = RunLog::open(&dir);
        for i in 0..5 {
            log.append("transport", json!({"action": "play", "n": i}));
        }
        drop(log);
        let path = log_files(&dir).pop().unwrap();
        let original = fs::read_to_string(&path).unwrap();

        let edited = original.replace("\"n\":2", "\"n\":9");
        assert_ne!(edited, original);
        fs::write(&path, &edited).unwrap();
        assert!(verify(&dir).unwrap_err().contains("record 3 was modified"));

        let lines: Vec<&str> = original.lines().collect();
        let removed: Vec<&str> = lines.iter().enumerate().filter(|(i, _)| *i != 1).map(|(_, l)| *l).collect();
        fs::write(&path, removed.join("\n")).unwrap();
        assert!(verify(&dir).unwrap_err().contains("a line is missing"));

        fs::write(&path, &original).unwrap();
        assert_eq!(verify(&dir), Ok(5));
        let _ = fs::remove_dir_all(&dir);
    }
}
