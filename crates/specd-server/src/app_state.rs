// ABOUTME: Shared application state for the specd HTTP server.
// ABOUTME: Contains actor handles, SPECD_HOME path, and provides constructors for prod and test use.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use specd_agent::SwarmOrchestrator;
use specd_core::SpecActorHandle;
use tokio::sync::{Mutex, RwLock};
use ulid::Ulid;

use crate::providers::ProviderStatus;

/// Shared application state accessible by all Axum handlers.
/// Stores a map of spec actors keyed by their ULID and the SPECD_HOME directory.
pub struct AppState {
    pub actors: Arc<RwLock<HashMap<Ulid, SpecActorHandle>>>,
    pub swarms: Arc<RwLock<HashMap<Ulid, Arc<Mutex<SwarmOrchestrator>>>>>,
    pub specd_home: PathBuf,
    pub provider_status: ProviderStatus,
}

/// Type alias for the Arc-wrapped state used with Axum's State extractor.
pub type SharedState = Arc<AppState>;

impl AppState {
    /// Create a new AppState with the given home directory, provider status, and an empty actor map.
    pub fn new(specd_home: PathBuf, provider_status: ProviderStatus) -> Self {
        Self {
            actors: Arc::new(RwLock::new(HashMap::new())),
            swarms: Arc::new(RwLock::new(HashMap::new())),
            specd_home,
            provider_status,
        }
    }
}
