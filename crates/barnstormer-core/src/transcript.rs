// ABOUTME: Defines transcript and question types for human-agent conversation within a spec.
// ABOUTME: Supports message history and structured question formats (boolean, multiple-choice, freeform).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// Classifies how a transcript message should be displayed.
/// Chat messages render as full bubbles; step variants render as compact status lines.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum MessageKind {
    #[default]
    Chat,
    StepStarted,
    StepFinished,
}

impl MessageKind {
    /// Returns true for step variants (StepStarted, StepFinished).
    pub fn is_step(&self) -> bool {
        matches!(self, MessageKind::StepStarted | MessageKind::StepFinished)
    }

    /// Returns the display prefix used when formatting transcripts for LLM context.
    pub fn prefix(&self) -> &'static str {
        match self {
            MessageKind::StepStarted => "[step started] ",
            MessageKind::StepFinished => "[step finished] ",
            MessageKind::Chat => "",
        }
    }
}

/// A single message in the conversation transcript between humans, agents, and the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptMessage {
    pub message_id: Ulid,
    pub sender: String,
    pub content: String,
    #[serde(default)]
    pub kind: MessageKind,
    pub timestamp: DateTime<Utc>,
}

impl TranscriptMessage {
    /// Create a new transcript message with a fresh ULID and current timestamp.
    pub fn new(sender: String, content: String) -> Self {
        Self {
            message_id: Ulid::new(),
            sender,
            content,
            kind: MessageKind::Chat,
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
            UserQuestion::Boolean {
                question, default, ..
            } => {
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
            UserQuestion::MultipleChoice {
                choices,
                allow_multi,
                ..
            } => {
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
            UserQuestion::Freeform {
                question,
                placeholder,
                validation_hint,
                ..
            } => {
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
        assert_eq!(deser.kind, MessageKind::Chat);
    }

    #[test]
    fn message_kind_serde_round_trip_all_variants() {
        for kind in [MessageKind::Chat, MessageKind::StepStarted, MessageKind::StepFinished] {
            let json = serde_json::to_string(&kind).expect("serialize");
            let deser: MessageKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(kind, deser);
        }
    }

    #[test]
    fn message_kind_defaults_to_chat_when_missing() {
        // Simulate an old TranscriptMessage JSON that lacks the `kind` field.
        let json = r#"{
            "message_id": "01HTEST0000000000000000000",
            "sender": "human",
            "content": "Legacy message",
            "timestamp": "2025-01-01T00:00:00Z"
        }"#;
        let deser: TranscriptMessage = serde_json::from_str(json).expect("deserialize");
        assert_eq!(deser.kind, MessageKind::Chat);
    }

    #[test]
    fn transcript_message_round_trip_with_step_kind() {
        let msg = TranscriptMessage {
            message_id: Ulid::new(),
            sender: "manager-01HTEST".to_string(),
            content: "Reasoning about goals".to_string(),
            kind: MessageKind::StepStarted,
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&msg).expect("serialize");
        let deser: TranscriptMessage = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(deser.kind, MessageKind::StepStarted);
        assert_eq!(deser.content, "Reasoning about goals");
    }

    #[test]
    fn message_kind_is_step() {
        assert!(!MessageKind::Chat.is_step());
        assert!(MessageKind::StepStarted.is_step());
        assert!(MessageKind::StepFinished.is_step());
    }

    #[test]
    fn message_kind_prefix() {
        assert_eq!(MessageKind::Chat.prefix(), "");
        assert_eq!(MessageKind::StepStarted.prefix(), "[step started] ");
        assert_eq!(MessageKind::StepFinished.prefix(), "[step finished] ");
    }
}
