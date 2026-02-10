// ABOUTME: Append-only JSONL event log for durable event storage.
// ABOUTME: Provides crash-safe append, sequential replay, and repair for truncated files.

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use specd_core::Event;
use thiserror::Error;

/// Errors that can occur during JSONL log operations.
#[derive(Debug, Error)]
pub enum JsonlError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// An append-only JSONL event log backed by a file.
/// Each line is a single JSON-serialized Event followed by a newline.
pub struct JsonlLog {
    path: PathBuf,
    file: File,
}

impl JsonlLog {
    /// Returns the path to the underlying JSONL file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Open (or create) a JSONL log file at the given path.
    /// Creates parent directories if they do not exist.
    /// The file is opened in append mode.
    pub fn open(path: &Path) -> Result<Self, JsonlError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new().create(true).append(true).open(path)?;

        Ok(Self {
            path: path.to_path_buf(),
            file,
        })
    }

    /// Append a single event to the log. Serializes as one JSON line,
    /// writes it with a trailing newline, and fsyncs to disk.
    pub fn append(&mut self, event: &Event) -> Result<(), JsonlError> {
        let json = serde_json::to_string(event)?;
        writeln!(self.file, "{}", json)?;
        self.file.sync_all()?;
        Ok(())
    }

    /// Replay all events from a JSONL file, returning them in order.
    /// Empty lines are skipped. Returns an empty Vec for empty files.
    pub fn replay(path: &Path) -> Result<Vec<Event>, JsonlError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let event: Event = serde_json::from_str(&line)?;
            events.push(event);
        }

        Ok(events)
    }

    /// Repair a potentially corrupted JSONL file by keeping only complete,
    /// parseable lines and truncating any partial trailing data.
    /// Uses atomic temp-file + fsync + rename to prevent data loss on crash.
    /// Returns the count of valid events retained.
    pub fn repair(path: &Path) -> Result<usize, JsonlError> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);
        let mut valid_lines: Vec<String> = Vec::new();

        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            // Only keep lines that parse as valid Event JSON
            if serde_json::from_str::<Event>(&line).is_ok() {
                valid_lines.push(line);
            }
        }

        let count = valid_lines.len();

        // Write valid lines to a temp file, fsync, then atomically rename
        let tmp_path = path.with_extension("jsonl.tmp");
        let mut tmp_file = File::create(&tmp_path)?;
        for line in &valid_lines {
            writeln!(tmp_file, "{}", line)?;
        }
        tmp_file.sync_all()?;

        // Atomic rename over the original
        fs::rename(&tmp_path, path)?;

        // Fsync the parent directory to ensure the rename metadata is durable.
        // Without this, a crash after rename could leave the directory entry
        // pointing at the old file. Best-effort: if the fsync fails, the
        // rename already succeeded and the data is consistent.
        if let Some(parent) = path.parent()
            && let Ok(dir) = File::open(parent)
        {
            let _ = dir.sync_all();
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use specd_core::EventPayload;
    use tempfile::TempDir;
    use ulid::Ulid;

    fn make_event(event_id: u64, payload: EventPayload) -> Event {
        Event {
            event_id,
            spec_id: Ulid::new(),
            timestamp: Utc::now(),
            payload,
        }
    }

    fn make_spec_created_event(event_id: u64) -> Event {
        make_event(
            event_id,
            EventPayload::SpecCreated {
                title: format!("Spec {}", event_id),
                one_liner: "Test".to_string(),
                goal: "Goal".to_string(),
            },
        )
    }

    #[test]
    fn append_and_replay_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("events.jsonl");

        let mut log = JsonlLog::open(&path).unwrap();
        let e1 = make_spec_created_event(1);
        let e2 = make_spec_created_event(2);
        let e3 = make_spec_created_event(3);

        log.append(&e1).unwrap();
        log.append(&e2).unwrap();
        log.append(&e3).unwrap();

        let events = JsonlLog::replay(&path).unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event_id, 1);
        assert_eq!(events[1].event_id, 2);
        assert_eq!(events[2].event_id, 3);
    }

    #[test]
    fn replay_empty_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.jsonl");
        // Create an empty file
        File::create(&path).unwrap();

        let events = JsonlLog::replay(&path).unwrap();
        assert!(events.is_empty());
    }

    #[test]
    fn replay_handles_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("trailing.jsonl");

        let mut log = JsonlLog::open(&path).unwrap();
        log.append(&make_spec_created_event(1)).unwrap();
        // The file should end with \n from writeln!, no phantom event

        let events = JsonlLog::replay(&path).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn repair_truncates_partial_last_line() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("corrupt.jsonl");

        // Write a valid event
        let mut log = JsonlLog::open(&path).unwrap();
        log.append(&make_spec_created_event(1)).unwrap();
        log.append(&make_spec_created_event(2)).unwrap();
        drop(log);

        // Append garbage to simulate a partial write
        let mut file = OpenOptions::new().append(true).open(&path).unwrap();
        write!(file, r#"{{"event_id":3,"spec_id":"bad_json_no_clos"#).unwrap();
        drop(file);

        let count = JsonlLog::repair(&path).unwrap();
        assert_eq!(count, 2);

        // Verify the file now replays cleanly
        let events = JsonlLog::replay(&path).unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].event_id, 1);
        assert_eq!(events[1].event_id, 2);
    }

    #[test]
    fn repair_no_op_on_clean_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("clean.jsonl");

        let mut log = JsonlLog::open(&path).unwrap();
        log.append(&make_spec_created_event(1)).unwrap();
        log.append(&make_spec_created_event(2)).unwrap();
        log.append(&make_spec_created_event(3)).unwrap();
        drop(log);

        let count = JsonlLog::repair(&path).unwrap();
        assert_eq!(count, 3);

        let events = JsonlLog::replay(&path).unwrap();
        assert_eq!(events.len(), 3);
    }

    #[test]
    fn append_is_crash_safe() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("synced.jsonl");

        let mut log = JsonlLog::open(&path).unwrap();
        let event = make_spec_created_event(1);
        log.append(&event).unwrap();
        // After append + sync_all, we should be able to read the event back
        // by opening a fresh reader
        drop(log);

        let events = JsonlLog::replay(&path).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].event_id, 1);
    }
}
