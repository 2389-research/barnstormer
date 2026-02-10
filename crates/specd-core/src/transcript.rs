// ABOUTME: Defines transcript and question types for human-agent conversation within a spec.
// ABOUTME: Supports message history and structured question formats (boolean, multiple-choice, freeform).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// A single message in the conversation transcript between humans, agents, and the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptMessage {
    pub message_id: Ulid,
    pub sender: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

impl TranscriptMessage {
    /// Create a new transcript message with a fresh ULID and current timestamp.
    pub fn new(sender: String, content: String) -> Self {
        Self {
            message_id: Ulid::new(),
            sender,
            content,
            timestamp: Utc::now(),
        }
    }
}

/// A structured question that an agent can pose to a human, supporting multiple
/// interaction patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum UserQuestion {
    Boolean {
        question_id: Ulid,
        question: String,
        default: Option<bool>,
    },
    MultipleChoice {
        question_id: Ulid,
        question: String,
        choices: Vec<String>,
        allow_multi: bool,
    },
    Freeform {
        question_id: Ulid,
        question: String,
        placeholder: Option<String>,
        validation_hint: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_question_variants_serialize() {
        let bool_q = UserQuestion::Boolean {
            question_id: Ulid::new(),
            question: "Ready to proceed?".to_string(),
            default: Some(true),
        };
        let json = serde_json::to_string(&bool_q).expect("serialize boolean");
        let deser: UserQuestion = serde_json::from_str(&json).expect("deserialize boolean");
        match deser {
            UserQuestion::Boolean { question, default, .. } => {
                assert_eq!(question, "Ready to proceed?");
                assert_eq!(default, Some(true));
            }
            _ => panic!("expected Boolean variant"),
        }

        let mc_q = UserQuestion::MultipleChoice {
            question_id: Ulid::new(),
            question: "Pick a color".to_string(),
            choices: vec!["red".to_string(), "blue".to_string(), "green".to_string()],
            allow_multi: true,
        };
        let json = serde_json::to_string(&mc_q).expect("serialize mc");
        let deser: UserQuestion = serde_json::from_str(&json).expect("deserialize mc");
        match deser {
            UserQuestion::MultipleChoice { choices, allow_multi, .. } => {
                assert_eq!(choices.len(), 3);
                assert!(allow_multi);
            }
            _ => panic!("expected MultipleChoice variant"),
        }

        let free_q = UserQuestion::Freeform {
            question_id: Ulid::new(),
            question: "Describe the feature".to_string(),
            placeholder: Some("Type here...".to_string()),
            validation_hint: None,
        };
        let json = serde_json::to_string(&free_q).expect("serialize freeform");
        let deser: UserQuestion = serde_json::from_str(&json).expect("deserialize freeform");
        match deser {
            UserQuestion::Freeform { question, placeholder, validation_hint, .. } => {
                assert_eq!(question, "Describe the feature");
                assert_eq!(placeholder, Some("Type here...".to_string()));
                assert!(validation_hint.is_none());
            }
            _ => panic!("expected Freeform variant"),
        }
    }

    #[test]
    fn transcript_message_round_trip() {
        let msg = TranscriptMessage::new("human".to_string(), "Hello agent!".to_string());
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: TranscriptMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(msg.message_id, deser.message_id);
        assert_eq!(deser.sender, "human");
        assert_eq!(deser.content, "Hello agent!");
    }
}
