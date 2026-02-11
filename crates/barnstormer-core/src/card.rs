// ABOUTME: Defines the Card struct representing a kanban-style card within a spec.
// ABOUTME: Cards have types (idea, plan, task, etc.), belong to lanes, and track authorship.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// A card within a spec's board. Cards represent discrete units of work,
/// ideas, decisions, or other categorized content.
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
    }
}
