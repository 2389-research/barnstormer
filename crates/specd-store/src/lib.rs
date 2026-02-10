// ABOUTME: Persistence layer for specd, handling event storage and state reconstruction.
// ABOUTME: Provides JSONL event log, snapshot management, SQLite index, crash recovery, and storage management.

pub mod jsonl;
pub mod manager;
pub mod recovery;
pub mod snapshot;
pub mod sqlite;

pub use jsonl::{JsonlError, JsonlLog};
pub use manager::{ManagerError, StorageManager};
pub use recovery::{RecoveryError, recover_spec};
pub use snapshot::{SnapshotData, SnapshotError, load_latest_snapshot, save_snapshot};
pub use sqlite::{SqliteError, SqliteIndex};
