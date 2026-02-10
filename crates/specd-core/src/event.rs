// ABOUTME: Defines the event envelope and all event payload variants for the specd event log.
// ABOUTME: Events represent immutable facts about what happened to a spec over time.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::card::Card;
use crate::transcript::{TranscriptMessage, UserQuestion};

/// An event envelope wrapping a timestamped, sequenced payload for a given spec.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub event_id: u64,
    pub spec_id: Ulid,
    pub timestamp: DateTime<Utc>,
    pub payload: EventPayload,
}

/// The set of things that can happen to a spec. Each variant captures the
/// minimum data needed to reconstruct or replay state changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventPayload {
    SpecCreated {
        title: String,
        one_liner: String,
        goal: String,
    },
    SpecCoreUpdated {
        title: Option<String>,
        one_liner: Option<String>,
        goal: Option<String>,
        description: Option<String>,
        constraints: Option<String>,
        success_criteria: Option<String>,
        risks: Option<String>,
        notes: Option<String>,
    },
    CardCreated {
        card: Card,
    },
    CardUpdated {
        card_id: Ulid,
        title: Option<String>,
        body: Option<Option<String>>,
        card_type: Option<String>,
        refs: Option<Vec<String>>,
    },
    CardMoved {
        card_id: Ulid,
        lane: String,
        order: f64,
    },
    CardDeleted {
        card_id: Ulid,
    },
    TranscriptAppended {
        message: TranscriptMessage,
    },
    QuestionAsked {
        question: UserQuestion,
    },
    QuestionAnswered {
        question_id: Ulid,
        answer: String,
    },
    AgentStepStarted {
        agent_id: String,
        description: String,
    },
    AgentStepFinished {
        agent_id: String,
        diff_summary: String,
    },
    UndoApplied {
        target_event_id: u64,
        inverse_events: Vec<EventPayload>,
    },
    SnapshotWritten {
        snapshot_id: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip_event(payload: EventPayload) {
        let event = Event {
            event_id: 1,
            spec_id: Ulid::new(),
            timestamp: Utc::now(),
            payload,
        };
        let json = serde_json::to_string(&event).expect("serialize event");
        let deser: Event = serde_json::from_str(&json).expect("deserialize event");
        assert_eq!(event.event_id, deser.event_id);
        assert_eq!(event.spec_id, deser.spec_id);
    }

    #[test]
    fn event_serializes_round_trip_spec_created() {
        round_trip_event(EventPayload::SpecCreated {
            title: "Test Spec".to_string(),
            one_liner: "A test".to_string(),
            goal: "Verify serialization".to_string(),
        });
    }

    #[test]
    fn event_serializes_round_trip_spec_core_updated() {
        round_trip_event(EventPayload::SpecCoreUpdated {
            title: Some("Updated Title".to_string()),
            one_liner: None,
            goal: None,
            description: Some("New description".to_string()),
            constraints: None,
            success_criteria: None,
            risks: None,
            notes: None,
        });
    }

    #[test]
    fn event_serializes_round_trip_card_created() {
        let card = Card::new(
            "idea".to_string(),
            "Test Card".to_string(),
            "agent-1".to_string(),
        );
        round_trip_event(EventPayload::CardCreated { card });
    }

    #[test]
    fn event_serializes_round_trip_card_updated() {
        round_trip_event(EventPayload::CardUpdated {
            card_id: Ulid::new(),
            title: Some("Renamed Card".to_string()),
            body: Some(Some("New body content".to_string())),
            card_type: None,
            refs: Some(vec!["ref-1".to_string()]),
        });
    }

    #[test]
    fn event_serializes_round_trip_card_moved() {
        round_trip_event(EventPayload::CardMoved {
            card_id: Ulid::new(),
            lane: "In Progress".to_string(),
            order: 1.5,
        });
    }

    #[test]
    fn event_serializes_round_trip_card_deleted() {
        round_trip_event(EventPayload::CardDeleted {
            card_id: Ulid::new(),
        });
    }

    #[test]
    fn event_serializes_round_trip_transcript_appended() {
        let msg = TranscriptMessage::new("human".to_string(), "Hello".to_string());
        round_trip_event(EventPayload::TranscriptAppended { message: msg });
    }

    #[test]
    fn event_serializes_round_trip_question_asked() {
        let q = UserQuestion::Boolean {
            question_id: Ulid::new(),
            question: "Proceed?".to_string(),
            default: Some(true),
        };
        round_trip_event(EventPayload::QuestionAsked { question: q });
    }

    #[test]
    fn event_serializes_round_trip_question_answered() {
        round_trip_event(EventPayload::QuestionAnswered {
            question_id: Ulid::new(),
            answer: "Yes".to_string(),
        });
    }

    #[test]
    fn event_serializes_round_trip_agent_step_started() {
        round_trip_event(EventPayload::AgentStepStarted {
            agent_id: "explorer".to_string(),
            description: "Analyzing requirements".to_string(),
        });
    }

    #[test]
    fn event_serializes_round_trip_agent_step_finished() {
        round_trip_event(EventPayload::AgentStepFinished {
            agent_id: "explorer".to_string(),
            diff_summary: "Added 3 cards".to_string(),
        });
    }

    #[test]
    fn event_serializes_round_trip_undo_applied() {
        round_trip_event(EventPayload::UndoApplied {
            target_event_id: 5,
            inverse_events: vec![EventPayload::CardDeleted {
                card_id: Ulid::new(),
            }],
        });
    }

    #[test]
    fn event_serializes_round_trip_snapshot_written() {
        round_trip_event(EventPayload::SnapshotWritten { snapshot_id: 42 });
    }
}
