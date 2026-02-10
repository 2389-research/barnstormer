// ABOUTME: Atomic snapshot save and load for SpecState persistence.
// ABOUTME: Writes snapshots with atomic rename for crash safety and loads the latest by event ID.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use specd_core::SpecState;
use thiserror::Error;

/// Errors that can occur during snapshot operations.
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// A full snapshot of spec state at a given event, including optional
/// agent context for restoring agent-specific working memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotData {
    pub state: SpecState,
    pub last_event_id: u64,
    pub agent_contexts: HashMap<String, serde_json::Value>,
    pub saved_at: DateTime<Utc>,
}

/// Save a snapshot to disk using atomic write (write to .tmp, fsync, rename).
/// Creates the target directory if it does not exist.
pub fn save_snapshot(dir: &Path, data: &SnapshotData) -> Result<(), SnapshotError> {
    fs::create_dir_all(dir)?;

    let tmp_path = dir.join(format!("state_{}.tmp", data.last_event_id));
    let final_path = dir.join(format!("state_{}.json", data.last_event_id));

    let json = serde_json::to_string_pretty(data)?;

    let mut file = File::create(&tmp_path)?;
    file.write_all(json.as_bytes())?;
    file.sync_all()?;
    drop(file);

    fs::rename(&tmp_path, &final_path)?;

    Ok(())
}

/// Load the snapshot with the highest event ID from the given directory.
/// Returns None if the directory is empty or does not exist.
pub fn load_latest_snapshot(dir: &Path) -> Result<Option<SnapshotData>, SnapshotError> {
    if !dir.exists() {
        return Ok(None);
    }

    let mut best: Option<(u64, std::path::PathBuf)> = None;

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Match pattern: state_<event_id>.json
        if let Some(rest) = name_str.strip_prefix("state_")
            && let Some(id_str) = rest.strip_suffix(".json")
            && let Ok(event_id) = id_str.parse::<u64>()
        {
            match &best {
                Some((current_best, _)) if event_id > *current_best => {
                    best = Some((event_id, entry.path()));
                }
                None => {
                    best = Some((event_id, entry.path()));
                }
                _ => {}
            }
        }
    }

    match best {
        Some((_, path)) => {
            let contents = fs::read_to_string(&path)?;
            let data: SnapshotData = serde_json::from_str(&contents)?;
            Ok(Some(data))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use specd_core::SpecState;
    use tempfile::TempDir;

    fn make_snapshot(event_id: u64) -> SnapshotData {
        let mut state = SpecState::new();
        state.last_event_id = event_id;

        let mut agent_contexts = HashMap::new();
        agent_contexts.insert(
            "explorer".to_string(),
            serde_json::json!({"step": 3, "notes": "found patterns"}),
        );

        SnapshotData {
            state,
            last_event_id: event_id,
            agent_contexts,
            saved_at: Utc::now(),
        }
    }

    #[test]
    fn snapshot_round_trip() {
        let dir = TempDir::new().unwrap();
        let snap = make_snapshot(42);

        save_snapshot(dir.path(), &snap).unwrap();

        let loaded = load_latest_snapshot(dir.path())
            .unwrap()
            .expect("should find snapshot");

        assert_eq!(loaded.last_event_id, 42);
        assert_eq!(loaded.state.last_event_id, 42);
        assert!(loaded.agent_contexts.contains_key("explorer"));
        assert_eq!(
            loaded.agent_contexts["explorer"]["step"],
            serde_json::json!(3)
        );
    }

    #[test]
    fn load_latest_picks_highest() {
        let dir = TempDir::new().unwrap();

        save_snapshot(dir.path(), &make_snapshot(10)).unwrap();
        save_snapshot(dir.path(), &make_snapshot(20)).unwrap();

        let loaded = load_latest_snapshot(dir.path())
            .unwrap()
            .expect("should find snapshot");

        assert_eq!(loaded.last_event_id, 20);
    }

    #[test]
    fn load_returns_none_for_empty_dir() {
        let dir = TempDir::new().unwrap();

        let result = load_latest_snapshot(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn save_creates_directory() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("deep").join("nested").join("snapshots");

        save_snapshot(&nested, &make_snapshot(5)).unwrap();

        let loaded = load_latest_snapshot(&nested)
            .unwrap()
            .expect("should find snapshot");

        assert_eq!(loaded.last_event_id, 5);
    }
}
