// ABOUTME: Persistence layer for specd, handling event storage and state reconstruction.
// ABOUTME: Provides JSONL event log, snapshot management, and state recovery.

pub mod jsonl;

pub use jsonl::{JsonlLog, JsonlError};
