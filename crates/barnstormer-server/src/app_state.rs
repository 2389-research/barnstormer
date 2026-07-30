// ABOUTME: Shared application state for the barnstormer HTTP server.
// ABOUTME: Contains actor handles, BARNSTORMER_HOME path, and provides constructors for prod and test use.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use barnstormer_agent::SwarmOrchestrator;
use barnstormer_core::SpecActorHandle;
use tokio::sync::{Mutex, RwLock};
use ulid::Ulid;

use crate::providers::ProviderStatus;

/// Google Drive Picker configuration. Present only when both env vars are set,
/// so the Drive button stays hidden (feature-flag off) until configured.
#[derive(Clone, Debug)]
pub struct DriveConfig {
    pub client_id: String,
    pub api_key: String,
}

impl DriveConfig {
    /// Read `GOOGLE_DRIVE_CLIENT_ID` + `GOOGLE_DRIVE_API_KEY`. Returns `None`
    /// unless both are present and non-empty.
    pub fn from_env() -> Option<Self> {
        let client_id = std::env::var("GOOGLE_DRIVE_CLIENT_ID")
            .ok()
            .filter(|s| !s.trim().is_empty())?;
        let api_key = std::env::var("GOOGLE_DRIVE_API_KEY")
            .ok()
            .filter(|s| !s.trim().is_empty())?;
        Some(Self { client_id, api_key })
    }
}

/// Bundles a SwarmOrchestrator with its background task handle so
/// the agent loop can be cancelled on cleanup.
pub struct SwarmHandle {
    pub swarm: Arc<Mutex<SwarmOrchestrator>>,
    pub task: tokio::task::JoinHandle<()>,
}

/// Shared application state accessible by all Axum handlers.
/// Stores a map of spec actors keyed by their ULID and the BARNSTORMER_HOME directory.
pub struct AppState {
    pub actors: Arc<RwLock<HashMap<Ulid, SpecActorHandle>>>,
    pub swarms: Arc<RwLock<HashMap<Ulid, SwarmHandle>>>,
    /// Background tasks that subscribe to actor broadcast channels and persist
    /// every event to JSONL. Keyed by spec ULID for cleanup on shutdown.
    pub event_persisters: Arc<RwLock<HashMap<Ulid, tokio::task::JoinHandle<()>>>>,
    pub barnstormer_home: PathBuf,
    pub provider_status: ProviderStatus,
    /// Google Drive Picker config, or `None` when not configured.
    pub drive: Option<DriveConfig>,
}

/// Type alias for the Arc-wrapped state used with Axum's State extractor.
pub type SharedState = Arc<AppState>;

impl AppState {
    /// Create a new AppState with the given home directory, provider status, and an empty actor map.
    /// Google Drive config is read from the environment (off unless both vars are set).
    pub fn new(barnstormer_home: PathBuf, provider_status: ProviderStatus) -> Self {
        Self {
            actors: Arc::new(RwLock::new(HashMap::new())),
            swarms: Arc::new(RwLock::new(HashMap::new())),
            event_persisters: Arc::new(RwLock::new(HashMap::new())),
            barnstormer_home,
            provider_status,
            drive: DriveConfig::from_env(),
        }
    }
}
