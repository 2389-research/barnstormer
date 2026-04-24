// ABOUTME: Defines the Card struct representing a kanban-style card within a spec.
// ABOUTME: Cards have types (idea, plan, task, etc.), belong to lanes, and track authorship.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// A card within a spec's board. Cards represent discrete units of work,
/// ideas, decisions, or other categorized content.
///
/// `source_attachment_id` is an optional link back to the context attachment
/// that sourced this card (e.g. a card synthesized from an uploaded design
/// brief). Cards authored organically during brainstorming leave this field
/// as None. The field deserializes as None when absent, so pre-existing
/// events in the log continue to materialize without migration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Card {
    pub card_id: Ulid,
    pub card_type: String,
    pub title: String,
    pub body: Option<String>,
    pub lane: String,
    pub order: f64,
    pub refs: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub created_by: String,
    pub updated_by: String,
    #[serde(default)]
    pub source_attachment_id: Option<Ulid>,
}

impl Card {
    /// Create a new Card with the given type, title, and creator. Defaults
    /// to the "Ideas" lane with order 0.0, no body, and empty refs.
    pub fn new(card_type: String, title: String, created_by: String) -> Self {
        let now = Utc::now();
        Self {
            card_id: Ulid::new(),
            card_type,
            title,
            body: None,
            lane: "Ideas".to_string(),
            order: 0.0,
            refs: Vec::new(),
            created_at: now,
            updated_at: now,
            created_by: created_by.clone(),
            updated_by: created_by,
            source_attachment_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn card_defaults_to_ideas_lane() {
        let card = Card::new(
            "idea".to_string(),
            "My Card".to_string(),
            "agent-1".to_string(),
        );

        assert_eq!(card.lane, "Ideas");
        assert_eq!(card.card_type, "idea");
        assert_eq!(card.title, "My Card");
        assert_eq!(card.created_by, "agent-1");
        assert_eq!(card.updated_by, "agent-1");
        assert!(card.body.is_none());
        assert!(card.refs.is_empty());
        assert_eq!(card.order, 0.0);
    }

    #[test]
    fn card_serde_round_trip() {
        let card = Card::new(
            "task".to_string(),
            "Write tests".to_string(),
            "human".to_string(),
        );

        let json = serde_json::to_string(&card).expect("serialize");
        let deserialized: Card = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(card.card_id, deserialized.card_id);
        assert_eq!(card.card_type, deserialized.card_type);
        assert_eq!(card.title, deserialized.title);
        assert_eq!(card.lane, deserialized.lane);
        assert_eq!(card.order, deserialized.order);
        assert_eq!(card.created_by, deserialized.created_by);
        assert_eq!(card.updated_by, deserialized.updated_by);
        assert_eq!(card.source_attachment_id, deserialized.source_attachment_id);
    }

    #[test]
    fn card_new_defaults_source_attachment_id_to_none() {
        let card = Card::new(
            "idea".to_string(),
            "Organic".to_string(),
            "agent-1".to_string(),
        );
        assert!(card.source_attachment_id.is_none());
    }

    #[test]
    fn card_serde_round_trip_with_source_attachment_id() {
        let att_id = Ulid::new();
        let mut card = Card::new(
            "idea".to_string(),
            "From file".to_string(),
            "agent-1".to_string(),
        );
        card.source_attachment_id = Some(att_id);

        let json = serde_json::to_string(&card).expect("serialize");
        let deserialized: Card = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(deserialized.source_attachment_id, Some(att_id));
    }

    #[test]
    fn card_deserializes_without_source_attachment_id_field() {
        // Legacy cards persisted before this field existed must still load.
        let legacy = serde_json::json!({
            "card_id": Ulid::new().to_string(),
            "card_type": "idea",
            "title": "Legacy",
            "body": null,
            "lane": "Ideas",
            "order": 0.0,
            "refs": [],
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "created_by": "human",
            "updated_by": "human"
        });
        let card: Card = serde_json::from_value(legacy).expect("deserialize legacy");
        assert!(card.source_attachment_id.is_none());
    }
}
