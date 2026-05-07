// ABOUTME: Defines the Command enum representing all write operations that can be applied to a spec.
// ABOUTME: Commands are intent-based inputs that get validated and converted into events.

use serde::{Deserialize, Serialize};
use ulid::Ulid;

use crate::transcript::UserQuestion;

/// A command representing a desired mutation to a spec. Commands are validated
/// and translated into one or more events by the command handler.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    CreateSpec {
        title: String,
        one_liner: String,
        goal: String,
    },
    UpdateSpecCore {
        title: Option<String>,
        one_liner: Option<String>,
        goal: Option<String>,
        description: Option<String>,
        constraints: Option<String>,
        success_criteria: Option<String>,
        risks: Option<String>,
        notes: Option<String>,
    },
    CreateCard {
        card_type: String,
        title: String,
        body: Option<String>,
        lane: Option<String>,
        created_by: String,
        /// Optional link to the context attachment this card was synthesized
        /// from. `None` for cards authored without reference to a specific
        /// attachment. Deserializes as `None` when absent so JSON from clients
        /// that don't know about the field continues to work.
        #[serde(default)]
        source_attachment_id: Option<Ulid>,
    },
    UpdateCard {
        card_id: Ulid,
        title: Option<String>,
        body: Option<Option<String>>,
        card_type: Option<String>,
        refs: Option<Vec<String>>,
        updated_by: String,
    },
    MoveCard {
        card_id: Ulid,
        lane: String,
        order: f64,
        updated_by: String,
    },
    DeleteCard {
        card_id: Ulid,
        updated_by: String,
    },
    AppendTranscript {
        sender: String,
        content: String,
    },
    AskQuestion {
        question: UserQuestion,
    },
    AnswerQuestion {
        question_id: Ulid,
        answer: String,
    },
    StartAgentStep {
        agent_id: String,
        description: String,
    },
    FinishAgentStep {
        agent_id: String,
        diff_summary: String,
    },
    TransitionPhase {
        target: crate::state::SpecPhase,
    },
    UpdateCanvas {
        content: String,
    },
    AttachContext {
        attachment_id: Ulid,
        filename: String,
        mime_type: String,
        size_bytes: u64,
    },
    SummarizeContext {
        attachment_id: Ulid,
        summary: String,
    },
    MarkContextSummarizeFailed {
        attachment_id: Ulid,
        reason: String,
    },
    UpdateContextNotes {
        attachment_id: Ulid,
        notes: String,
    },
    RemoveContext {
        attachment_id: Ulid,
    },
    Undo,
    StreamDelta {
        agent_id: String,
        text: String,
    },
    StreamToolActivity {
        agent_id: String,
        activity: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_serializes_round_trip() {
        let commands = vec![
            Command::CreateSpec {
                title: "New Spec".to_string(),
                one_liner: "Short".to_string(),
                goal: "Build it".to_string(),
            },
            Command::UpdateSpecCore {
                title: Some("Updated".to_string()),
                one_liner: None,
                goal: None,
                description: Some("Details".to_string()),
                constraints: None,
                success_criteria: None,
                risks: None,
                notes: None,
            },
            Command::CreateCard {
                card_type: "idea".to_string(),
                title: "A card".to_string(),
                body: Some("Body text".to_string()),
                lane: Some("Backlog".to_string()),
                created_by: "human".to_string(),
                source_attachment_id: None,
            },
            Command::CreateCard {
                card_type: "idea".to_string(),
                title: "Sourced card".to_string(),
                body: Some("From the design doc".to_string()),
                lane: None,
                created_by: "manager-1".to_string(),
                source_attachment_id: Some(Ulid::new()),
            },
            Command::UpdateCard {
                card_id: Ulid::new(),
                title: Some("Renamed".to_string()),
                body: None,
                card_type: None,
                refs: None,
                updated_by: "agent-1".to_string(),
            },
            Command::MoveCard {
                card_id: Ulid::new(),
                lane: "Spec".to_string(),
                order: 2.0,
                updated_by: "human".to_string(),
            },
            Command::DeleteCard {
                card_id: Ulid::new(),
                updated_by: "human".to_string(),
            },
            Command::AppendTranscript {
                sender: "system".to_string(),
                content: "Spec created".to_string(),
            },
            Command::AskQuestion {
                question: UserQuestion::Freeform {
                    question_id: Ulid::new(),
                    question: "What next?".to_string(),
                    placeholder: None,
                    validation_hint: None,
                },
            },
            Command::AnswerQuestion {
                question_id: Ulid::new(),
                answer: "Let's go".to_string(),
            },
            Command::StartAgentStep {
                agent_id: "explorer".to_string(),
                description: "Exploring".to_string(),
            },
            Command::FinishAgentStep {
                agent_id: "explorer".to_string(),
                diff_summary: "Added cards".to_string(),
            },
            Command::TransitionPhase {
                target: crate::state::SpecPhase::Refining,
            },
            Command::UpdateCanvas {
                content: "<h1>Hello</h1>".to_string(),
            },
            Command::Undo,
            Command::StreamDelta {
                agent_id: "manager-1".to_string(),
                text: "token".to_string(),
            },
            Command::StreamToolActivity {
                agent_id: "brainstormer-1".to_string(),
                activity: "creating card".to_string(),
            },
        ];

        for cmd in &commands {
            let json = serde_json::to_string(cmd).expect("serialize command");
            let deser: Command = serde_json::from_str(&json).expect("deserialize command");
            // Verify the type tag round-trips by re-serializing
            let json2 = serde_json::to_string(&deser).expect("re-serialize");
            assert_eq!(json, json2, "round-trip mismatch for command");
        }
    }

    #[test]
    fn transition_phase_round_trip() {
        let cmd = Command::TransitionPhase {
            target: crate::state::SpecPhase::Refining,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"TransitionPhase\""));
        assert!(json.contains("\"Refining\""));
        let back: Command = serde_json::from_str(&json).unwrap();
        match back {
            Command::TransitionPhase { target } => {
                assert_eq!(target, crate::state::SpecPhase::Refining);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn update_canvas_round_trip() {
        let cmd = Command::UpdateCanvas {
            content: "<h1>Test</h1>".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"UpdateCanvas\""));
        let back: Command = serde_json::from_str(&json).unwrap();
        match back {
            Command::UpdateCanvas { content } => assert_eq!(content, "<h1>Test</h1>"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn attach_context_command_serializes() {
        let id = Ulid::new();
        let cmd = Command::AttachContext {
            attachment_id: id,
            filename: "notes.md".to_string(),
            mime_type: "text/markdown".to_string(),
            size_bytes: 1024,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"AttachContext\""));
        assert!(json.contains("\"filename\":\"notes.md\""));
    }

    #[test]
    fn summarize_context_command_serializes() {
        let cmd = Command::SummarizeContext {
            attachment_id: Ulid::new(),
            summary: "Key points...".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"SummarizeContext\""));
    }

    #[test]
    fn update_context_notes_command_serializes() {
        let cmd = Command::UpdateContextNotes {
            attachment_id: Ulid::new(),
            notes: "From the kickoff".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"UpdateContextNotes\""));
    }

    #[test]
    fn remove_context_command_serializes() {
        let cmd = Command::RemoveContext {
            attachment_id: Ulid::new(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"RemoveContext\""));
    }

    #[test]
    fn mark_context_summarize_failed_command_serializes() {
        let cmd = Command::MarkContextSummarizeFailed {
            attachment_id: Ulid::new(),
            reason: "unsupported media kind".to_string(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("\"type\":\"MarkContextSummarizeFailed\""));
        assert!(json.contains("\"reason\":\"unsupported media kind\""));
        let round: Command = serde_json::from_str(&json).unwrap();
        matches!(round, Command::MarkContextSummarizeFailed { .. });
    }

    #[test]
    fn create_card_deserializes_without_source_attachment_id_field() {
        // Clients that don't know about the new field must still be able to
        // emit a CreateCard command.
        let json = r#"{
            "type": "CreateCard",
            "card_type": "idea",
            "title": "No source",
            "body": null,
            "lane": null,
            "created_by": "human"
        }"#;
        let cmd: Command = serde_json::from_str(json).expect("parse");
        match cmd {
            Command::CreateCard {
                source_attachment_id,
                ..
            } => {
                assert!(source_attachment_id.is_none());
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn create_card_round_trips_with_source_attachment_id() {
        let att_id = Ulid::new();
        let cmd = Command::CreateCard {
            card_type: "idea".to_string(),
            title: "From file".to_string(),
            body: Some("body".to_string()),
            lane: None,
            created_by: "manager-1".to_string(),
            source_attachment_id: Some(att_id),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: Command = serde_json::from_str(&json).unwrap();
        match back {
            Command::CreateCard {
                source_attachment_id,
                ..
            } => {
                assert_eq!(source_attachment_id, Some(att_id));
            }
            _ => panic!("wrong variant"),
        }
    }
}
