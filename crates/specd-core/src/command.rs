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
    Undo,
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
                lane: "Done".to_string(),
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
            Command::Undo,
        ];

        for cmd in &commands {
            let json = serde_json::to_string(cmd).expect("serialize command");
            let deser: Command = serde_json::from_str(&json).expect("deserialize command");
            // Verify the type tag round-trips by re-serializing
            let json2 = serde_json::to_string(&deser).expect("re-serialize");
            assert_eq!(json, json2, "round-trip mismatch for command");
        }
    }
}
