// ABOUTME: Persistence layer for specd, handling event storage and state reconstruction.
// ABOUTME: Provides JSONL event log, snapshot management, SQLite index, and state recovery.

pub mod jsonl;
pub mod snapshot;
pub mod sqlite;

pub use jsonl::{JsonlError, JsonlLog};
pub use snapshot::{SnapshotData, SnapshotError, load_latest_snapshot, save_snapshot};
pub use sqlite::{SqliteError, SqliteIndex};
