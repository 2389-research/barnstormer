// ABOUTME: Persistence layer for specd, handling event storage and state reconstruction.
// ABOUTME: Provides JSONL event log, snapshot management, and state recovery.

pub mod jsonl;
pub mod snapshot;

pub use jsonl::{JsonlError, JsonlLog};
pub use snapshot::{SnapshotData, SnapshotError, load_latest_snapshot, save_snapshot};
