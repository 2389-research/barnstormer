// ABOUTME: Crash recovery and self-healing for spec state reconstruction.
// ABOUTME: Combines snapshots, JSONL repair, event replay, and SQLite integrity checks.

use std::path::Path;

use specd_core::state::SpecState;
use thiserror::Error;
use tracing;

use crate::jsonl::JsonlLog;
use crate::snapshot::load_latest_snapshot;
use crate::sqlite::SqliteIndex;

/// Errors that can occur during recovery.
#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("jsonl error: {0}")]
    Jsonl(#[from] crate::jsonl::JsonlError),

    #[error("snapshot error: {0}")]
    Snapshot(#[from] crate::snapshot::SnapshotError),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] crate::sqlite::SqliteError),
}

/// Recover a spec's state from its storage directory.
///
/// Recovery sequence:
/// 1. Try to load the latest snapshot
/// 2. Repair the JSONL event log (truncate partial last line)
/// 3. Replay events from the snapshot's last_event_id (or from beginning)
/// 4. Build SpecState from the events
/// 5. Check SQLite integrity (compare last_event_id)
/// 6. If mismatch: rebuild SQLite from all events
/// 7. Return recovered state and last_event_id
pub fn recover_spec(spec_dir: &Path) -> Result<(SpecState, u64), RecoveryError> {
    let events_path = spec_dir.join("events.jsonl");
    let snapshots_dir = spec_dir.join("snapshots");
    let index_path = spec_dir.join("index.db");

    // Step 1: Try to load latest snapshot
    let snapshot = load_latest_snapshot(&snapshots_dir)?;

    let (mut state, snapshot_event_id) = match &snapshot {
        Some(snap) => {
            tracing::info!("loaded snapshot at event {}", snap.last_event_id);
            (snap.state.clone(), snap.last_event_id)
        }
        None => {
            tracing::info!("no snapshot found, starting from empty state");
            (SpecState::new(), 0)
        }
    };

    // Step 2: Repair JSONL if it exists
    if events_path.exists() {
        let repaired_count = JsonlLog::repair(&events_path)?;
        tracing::info!("repaired JSONL: {} valid events", repaired_count);
    }

    // Step 3: Replay events from the JSONL log
    let all_events = if events_path.exists() {
        JsonlLog::replay(&events_path)?
    } else {
        Vec::new()
    };

    // Step 4: Apply events that are newer than the snapshot
    let tail_events: Vec<_> = all_events
        .iter()
        .filter(|e| e.event_id > snapshot_event_id)
        .collect();

    tracing::info!(
        "replaying {} events after snapshot (total {} events on disk)",
        tail_events.len(),
        all_events.len()
    );

    for event in &tail_events {
        state.apply(event);
    }

    let last_event_id = state.last_event_id;

    // Step 5 & 6: Check SQLite integrity and rebuild if needed
    let index = SqliteIndex::open(&index_path)?;
    let sqlite_last = index.get_last_event_id()?;

    match sqlite_last {
        Some(sqlite_id) if sqlite_id == last_event_id => {
            tracing::info!("SQLite index is up to date at event {}", sqlite_id);
        }
        Some(sqlite_id) => {
            tracing::warn!(
                "SQLite index stale (at event {}, expected {}), rebuilding",
                sqlite_id,
                last_event_id
            );
            index.rebuild_from_events(&all_events)?;
        }
        None => {
            tracing::info!("SQLite index empty, building from events");
            index.rebuild_from_events(&all_events)?;
        }
    }

    Ok((state, last_event_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonl::JsonlLog;
    use crate::snapshot::{SnapshotData, save_snapshot};
    use chrono::Utc;
    use specd_core::card::Card;
    use specd_core::event::{Event, EventPayload};
    use std::collections::HashMap;
    use std::fs::{self, OpenOptions};
    use std::io::Write;
    use tempfile::TempDir;
    use ulid::Ulid;

    fn make_event(event_id: u64, spec_id: Ulid, payload: EventPayload) -> Event {
        Event {
            event_id,
            spec_id,
            timestamp: Utc::now(),
            payload,
        }
    }

    fn make_spec_dir(dir: &TempDir) -> std::path::PathBuf {
        let spec_dir = dir.path().join("test_spec");
        fs::create_dir_all(spec_dir.join("snapshots")).unwrap();
        fs::create_dir_all(spec_dir.join("exports")).unwrap();
        spec_dir
    }

    fn write_events(spec_dir: &Path, events: &[Event]) {
        let events_path = spec_dir.join("events.jsonl");
        let mut log = JsonlLog::open(&events_path).unwrap();
        for event in events {
            log.append(event).unwrap();
        }
    }

    #[test]
    fn recover_from_clean_state() {
        let dir = TempDir::new().unwrap();
        let spec_dir = make_spec_dir(&dir);
        let spec_id = Ulid::new();

        let events = vec![
            make_event(
                1,
                spec_id,
                EventPayload::SpecCreated {
                    title: "Recovery Test".to_string(),
                    one_liner: "Test".to_string(),
                    goal: "Verify recovery".to_string(),
                },
            ),
            make_event(
                2,
                spec_id,
                EventPayload::CardCreated {
                    card: Card::new(
                        "idea".to_string(),
                        "Test Card".to_string(),
                        "human".to_string(),
                    ),
                },
            ),
        ];

        write_events(&spec_dir, &events);

        let (state, last_id) = recover_spec(&spec_dir).unwrap();

        assert_eq!(last_id, 2);
        assert!(state.core.is_some());
        assert_eq!(state.core.as_ref().unwrap().title, "Recovery Test");
        assert_eq!(state.cards.len(), 1);
    }

    #[test]
    fn recover_from_snapshot_plus_tail() {
        let dir = TempDir::new().unwrap();
        let spec_dir = make_spec_dir(&dir);
        let spec_id = Ulid::new();

        // Create 20 events
        let mut all_events = Vec::new();
        all_events.push(make_event(
            1,
            spec_id,
            EventPayload::SpecCreated {
                title: "Snapshot Test".to_string(),
                one_liner: "Test".to_string(),
                goal: "Verify snapshot + tail".to_string(),
            },
        ));
        for i in 2..=20 {
            all_events.push(make_event(
                i,
                spec_id,
                EventPayload::CardCreated {
                    card: Card::new(
                        "idea".to_string(),
                        format!("Card {}", i),
                        "human".to_string(),
                    ),
                },
            ));
        }

        // Write all events to JSONL
        write_events(&spec_dir, &all_events);

        // Create snapshot at event 10 (replay first 10 events to build state)
        let mut snap_state = SpecState::new();
        for event in &all_events[..10] {
            snap_state.apply(event);
        }

        let snap_data = SnapshotData {
            state: snap_state,
            last_event_id: 10,
            agent_contexts: HashMap::new(),
            saved_at: Utc::now(),
        };
        save_snapshot(&spec_dir.join("snapshots"), &snap_data).unwrap();

        // Recover: should load snapshot at 10, replay events 11-20
        let (state, last_id) = recover_spec(&spec_dir).unwrap();

        assert_eq!(last_id, 20);
        assert_eq!(state.core.as_ref().unwrap().title, "Snapshot Test");
        // 19 cards (events 2-20)
        assert_eq!(state.cards.len(), 19);
    }

    #[test]
    fn recover_repairs_partial_jsonl() {
        let dir = TempDir::new().unwrap();
        let spec_dir = make_spec_dir(&dir);
        let spec_id = Ulid::new();

        let events = vec![
            make_event(
                1,
                spec_id,
                EventPayload::SpecCreated {
                    title: "Repair Test".to_string(),
                    one_liner: "Test".to_string(),
                    goal: "Verify repair".to_string(),
                },
            ),
            make_event(
                2,
                spec_id,
                EventPayload::CardCreated {
                    card: Card::new(
                        "idea".to_string(),
                        "Good Card".to_string(),
                        "human".to_string(),
                    ),
                },
            ),
        ];

        write_events(&spec_dir, &events);

        // Append garbage to simulate a partial write / crash
        let events_path = spec_dir.join("events.jsonl");
        let mut file = OpenOptions::new().append(true).open(&events_path).unwrap();
        write!(file, r#"{{"event_id":3,"corrupt_data"#).unwrap();
        drop(file);

        // Recovery should repair and still get 2 valid events
        let (state, last_id) = recover_spec(&spec_dir).unwrap();

        assert_eq!(last_id, 2);
        assert!(state.core.is_some());
        assert_eq!(state.core.as_ref().unwrap().title, "Repair Test");
        assert_eq!(state.cards.len(), 1);
    }

    #[test]
    fn recover_rebuilds_stale_sqlite() {
        let dir = TempDir::new().unwrap();
        let spec_dir = make_spec_dir(&dir);
        let spec_id = Ulid::new();

        let card = Card::new(
            "idea".to_string(),
            "Stale Card".to_string(),
            "human".to_string(),
        );

        let events = vec![
            make_event(
                1,
                spec_id,
                EventPayload::SpecCreated {
                    title: "Stale Test".to_string(),
                    one_liner: "Test".to_string(),
                    goal: "Verify rebuild".to_string(),
                },
            ),
            make_event(2, spec_id, EventPayload::CardCreated { card: card.clone() }),
        ];

        write_events(&spec_dir, &events[..1]);

        // Create SQLite index with only event 1 (simulate it being behind)
        let index_path = spec_dir.join("index.db");
        let idx = SqliteIndex::open(&index_path).unwrap();
        idx.apply_event(&events[0]).unwrap();
        // Now SQLite says last_event_id = 1
        drop(idx);

        // Write remaining events to JSONL
        {
            let events_path = spec_dir.join("events.jsonl");
            let mut log = JsonlLog::open(&events_path).unwrap();
            for event in &events[1..] {
                log.append(event).unwrap();
            }
        }

        // Recovery should detect the mismatch and rebuild SQLite
        let (state, last_id) = recover_spec(&spec_dir).unwrap();

        assert_eq!(last_id, 2);
        assert_eq!(state.cards.len(), 1);

        // Verify SQLite was rebuilt
        let idx = SqliteIndex::open(&index_path).unwrap();
        let sqlite_last = idx.get_last_event_id().unwrap();
        assert_eq!(sqlite_last, Some(2));

        let cards = idx.list_cards(&spec_id).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Stale Card");
    }
}
