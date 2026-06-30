// ABOUTME: Defines the SpecCore struct representing a specification's core metadata.
// ABOUTME: Contains required fields (title, one_liner, goal) and optional detail fields.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// The core specification data, holding all metadata about a single spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecCore {
    pub spec_id: Ulid,
    pub title: String,
    pub one_liner: String,
    pub goal: String,
    pub description: Option<String>,
    pub constraints: Option<String>,
    pub success_criteria: Option<String>,
    pub risks: Option<String>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Whether the spec is pinned to the top of the rail. `#[serde(default)]`
    /// so snapshots written before this field existed still deserialize.
    #[serde(default)]
    pub pinned: bool,
    /// Whether the spec is archived (hidden from the rail, shown under
    /// Settings → Archive). `#[serde(default)]` so old snapshots load.
    #[serde(default)]
    pub archived: bool,
}

impl SpecCore {
    /// Create a new SpecCore with the required fields. Generates a fresh ULID and
    /// sets timestamps to now. Optional fields default to None.
    pub fn new(title: String, one_liner: String, goal: String) -> Self {
        let now = Utc::now();
        Self {
            spec_id: Ulid::new(),
            title,
            one_liner,
            goal,
            description: None,
            constraints: None,
            success_criteria: None,
            risks: None,
            notes: None,
            created_at: now,
            updated_at: now,
            pinned: false,
            archived: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_core_defaults_pinned_false_on_deserialize() {
        // Older snapshots written before `pinned` existed must still load.
        let json = r#"{
            "spec_id":"01ARZ3NDEKTSV4RRFFQ69G5FAV",
            "title":"t","one_liner":"o","goal":"g",
            "description":null,"constraints":null,"success_criteria":null,
            "risks":null,"notes":null,
            "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"
        }"#;
        let core: SpecCore = serde_json::from_str(json).unwrap();
        assert!(!core.pinned);
        assert!(!core.archived);
    }

    #[test]
    fn spec_new_sets_required_fields() {
        let spec = SpecCore::new(
            "My Spec".to_string(),
            "A short summary".to_string(),
            "Build something great".to_string(),
        );

        assert_eq!(spec.title, "My Spec");
        assert_eq!(spec.one_liner, "A short summary");
        assert_eq!(spec.goal, "Build something great");
        assert!(spec.description.is_none());
        assert!(spec.constraints.is_none());
        assert!(spec.success_criteria.is_none());
        assert!(spec.risks.is_none());
        assert!(spec.notes.is_none());
        assert!(spec.created_at <= Utc::now());
        assert_eq!(spec.created_at, spec.updated_at);
    }

    #[test]
    fn spec_new_generates_ulid() {
        let spec_a = SpecCore::new(
            "Spec A".to_string(),
            "One liner A".to_string(),
            "Goal A".to_string(),
        );
        let spec_b = SpecCore::new(
            "Spec B".to_string(),
            "One liner B".to_string(),
            "Goal B".to_string(),
        );

        // Each call to new() must produce a distinct ULID
        assert_ne!(spec_a.spec_id, spec_b.spec_id);
    }
}
