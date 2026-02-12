// ABOUTME: High-level storage manager for the barnstormer daemon's filesystem layout.
// ABOUTME: Handles directory creation, spec discovery, recovery orchestration, and export writing.

use std::fs;
use std::path::{Path, PathBuf};

use barnstormer_core::export::{export_dot, export_markdown, export_yaml};
use barnstormer_core::state::SpecState;
use thiserror::Error;
use ulid::Ulid;

use crate::recovery::{RecoveryError, recover_spec};

/// Errors that can occur during storage management operations.
#[derive(Debug, Error)]
pub enum ManagerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("recovery error: {0}")]
    Recovery(#[from] RecoveryError),

    #[error("yaml export error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("invalid spec directory name: {0}")]
    InvalidSpecDir(String),
}

/// Manages the barnstormer home directory layout and provides high-level operations
/// for spec storage, recovery, and export generation.
pub struct StorageManager {
    home: PathBuf,
}

impl StorageManager {
    /// Create a new StorageManager rooted at the given home directory.
    /// Creates the home and specs subdirectories if they do not exist.
    pub fn new(home: PathBuf) -> Result<Self, ManagerError> {
        let specs_dir = home.join("specs");
        fs::create_dir_all(&specs_dir)?;
        Ok(Self { home })
    }

    /// Return the home directory path.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Scan the specs directory and return all spec directories with their ULIDs.
    pub fn list_spec_dirs(&self) -> Result<Vec<(Ulid, PathBuf)>, ManagerError> {
        let specs_dir = self.home.join("specs");
        if !specs_dir.exists() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();
        for entry in fs::read_dir(&specs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            match name_str.parse::<Ulid>() {
                Ok(spec_id) => results.push((spec_id, path)),
                Err(_) => {
                    tracing::warn!("skipping non-ULID directory in specs/: {}", name_str);
                }
            }
        }

        Ok(results)
    }

    /// Create a spec directory with the required subdirectories.
    pub fn create_spec_dir(&self, spec_id: &Ulid) -> Result<PathBuf, ManagerError> {
        let spec_dir = self.home.join("specs").join(spec_id.to_string());
        fs::create_dir_all(spec_dir.join("snapshots"))?;
        fs::create_dir_all(spec_dir.join("exports"))?;
        Ok(spec_dir)
    }

    /// Get the path to a spec's directory (does not create it).
    pub fn get_spec_dir(&self, spec_id: &Ulid) -> PathBuf {
        self.home.join("specs").join(spec_id.to_string())
    }

    /// Recover all specs from their storage directories.
    /// Returns a list of (spec_id, recovered_state) pairs.
    /// Logs and skips specs that fail to recover.
    pub fn recover_all_specs(&self) -> Result<Vec<(Ulid, SpecState)>, ManagerError> {
        let spec_dirs = self.list_spec_dirs()?;
        let mut recovered = Vec::new();

        for (spec_id, spec_dir) in &spec_dirs {
            match recover_spec(spec_dir) {
                Ok((state, last_event_id)) => {
                    tracing::info!("recovered spec {} at event {}", spec_id, last_event_id);
                    recovered.push((*spec_id, state));
                }
                Err(e) => {
                    tracing::error!("failed to recover spec {}: {}", spec_id, e);
                }
            }
        }

        Ok(recovered)
    }

    /// Write export files (spec.md, spec.yaml, pipeline.dot) to the exports/ subdirectory.
    pub fn write_exports(spec_dir: &Path, state: &SpecState) -> Result<(), ManagerError> {
        let exports_dir = spec_dir.join("exports");
        fs::create_dir_all(&exports_dir)?;

        // Write Markdown export
        let md = export_markdown(state);
        fs::write(exports_dir.join("spec.md"), md)?;

        // Write YAML export (only if core exists)
        if state.core.is_some() {
            let yaml = export_yaml(state)?;
            fs::write(exports_dir.join("spec.yaml"), yaml)?;
        }

        // Write DOT export
        let dot = export_dot(state);
        fs::write(exports_dir.join("pipeline.dot"), dot)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use barnstormer_core::card::Card;
    use barnstormer_core::model::SpecCore;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn make_state_with_core() -> SpecState {
        let core = SpecCore {
            spec_id: Ulid::new(),
            title: "Export Spec".to_string(),
            one_liner: "For export testing".to_string(),
            goal: "Verify exports".to_string(),
            description: None,
            constraints: None,
            success_criteria: None,
            risks: None,
            notes: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        SpecState {
            core: Some(core),
            cards: BTreeMap::new(),
            transcript: Vec::new(),
            pending_question: None,
            undo_stack: Vec::new(),
            last_event_id: 0,
            lanes: vec!["Ideas".to_string(), "Plan".to_string(), "Spec".to_string()],
        }
    }

    #[test]
    fn storage_manager_creates_directories() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("barnstormer_home");

        let mgr = StorageManager::new(home.clone()).unwrap();

        assert!(home.join("specs").exists());
        assert_eq!(mgr.home(), &home);
    }

    #[test]
    fn storage_manager_creates_spec_dir() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("barnstormer_home");
        let mgr = StorageManager::new(home).unwrap();

        let spec_id = Ulid::new();
        let spec_dir = mgr.create_spec_dir(&spec_id).unwrap();

        assert!(spec_dir.exists());
        assert!(spec_dir.join("snapshots").exists());
        assert!(spec_dir.join("exports").exists());

        // get_spec_dir should return the same path
        assert_eq!(mgr.get_spec_dir(&spec_id), spec_dir);
    }

    #[test]
    fn storage_manager_writes_exports() {
        let dir = TempDir::new().unwrap();
        let spec_dir = dir.path().join("spec_dir");
        fs::create_dir_all(&spec_dir).unwrap();

        let mut state = make_state_with_core();
        let mut card = Card::new(
            "idea".to_string(),
            "Export Card".to_string(),
            "human".to_string(),
        );
        // Place in Plan lane so DOT exporter includes it (Ideas lane is excluded)
        card.lane = "Plan".to_string();
        state.cards.insert(card.card_id, card);

        StorageManager::write_exports(&spec_dir, &state).unwrap();

        let exports_dir = spec_dir.join("exports");
        assert!(exports_dir.join("spec.md").exists());
        assert!(exports_dir.join("spec.yaml").exists());
        assert!(exports_dir.join("pipeline.dot").exists());

        // Verify content
        let md = fs::read_to_string(exports_dir.join("spec.md")).unwrap();
        assert!(md.contains("# Export Spec"));
        assert!(md.contains("Export Card"));

        let yaml = fs::read_to_string(exports_dir.join("spec.yaml")).unwrap();
        assert!(yaml.contains("Export Spec"));

        let dot = fs::read_to_string(exports_dir.join("pipeline.dot")).unwrap();
        assert!(dot.contains("digraph export_spec"));
        assert!(dot.contains("Export Card"), "Card title should appear in synthesized prompt");
    }
}
