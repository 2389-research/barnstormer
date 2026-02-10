// ABOUTME: Implements the read_state tool for reading current spec state via mux Tool trait.
// ABOUTME: Formats SpecState into a human-readable text summary for LLM consumption.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;

use specd_core::actor::SpecActorHandle;

/// Truncate a string to at most `max_chars` characters, appending "..." if truncated.
/// Safe for multibyte UTF-8 (never slices mid-character).
fn truncate_utf8_safe(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars).collect();
        format!("{}...", truncated)
    }
}

/// Tool that reads the current spec state and returns a formatted text summary.
#[derive(Clone)]
pub struct ReadStateTool {
    pub(crate) actor: Arc<SpecActorHandle>,
}

#[async_trait]
impl Tool for ReadStateTool {
    fn name(&self) -> &str {
        "read_state"
    }

    fn description(&self) -> &str {
        "Read the current spec state summary including cards, transcript, and metadata. Returns a text summary of the spec's current state."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        // Clone the data we need so we release the RwLockReadGuard quickly
        // instead of holding it across the entire formatting operation.
        let (core, cards, pending_question, transcript_len, recent_transcript, lanes) = {
            let state = self.actor.read_state().await;
            (
                state.core.clone(),
                state.cards.values().cloned().collect::<Vec<_>>(),
                state.pending_question.clone(),
                state.transcript.len(),
                state
                    .transcript
                    .iter()
                    .rev()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>(),
                state.lanes.clone(),
            )
        };

        let mut lines = Vec::new();

        // Spec core metadata
        match &core {
            Some(core) => {
                lines.push(format!("# {}", core.title));
                lines.push(format!("One-liner: {}", core.one_liner));
                lines.push(format!("Goal: {}", core.goal));
                if let Some(desc) = &core.description {
                    lines.push(format!("Description: {}", desc));
                }
                if let Some(constraints) = &core.constraints {
                    lines.push(format!("Constraints: {}", constraints));
                }
                if let Some(criteria) = &core.success_criteria {
                    lines.push(format!("Success Criteria: {}", criteria));
                }
                if let Some(risks) = &core.risks {
                    lines.push(format!("Risks: {}", risks));
                }
                if let Some(notes) = &core.notes {
                    lines.push(format!("Notes: {}", notes));
                }
            }
            None => {
                lines.push("(No spec created yet)".to_string());
            }
        }

        // Lanes
        lines.push(String::new());
        lines.push(format!("## Lanes: {}", lanes.join(", ")));

        // Cards summary
        lines.push(String::new());
        lines.push(format!("## Cards ({})", cards.len()));
        for card in &cards {
            let body_preview = card
                .body
                .as_deref()
                .map(|b| truncate_utf8_safe(b, 80))
                .unwrap_or_default();
            lines.push(format!(
                "- [{}] {} (type: {}, lane: {}) {}",
                card.card_id, card.title, card.card_type, card.lane, body_preview
            ));
        }

        // Pending question
        lines.push(String::new());
        match &pending_question {
            Some(q) => {
                lines.push(format!("## Pending Question: {:?}", q));
            }
            None => {
                lines.push("## No pending question".to_string());
            }
        }

        // Transcript summary
        lines.push(String::new());
        lines.push(format!("## Transcript ({} messages)", transcript_len));
        for msg in &recent_transcript {
            let prefix = msg.kind.prefix();
            lines.push(format!(
                "  [{}] {}: {}{}",
                msg.timestamp, msg.sender, prefix, msg.content
            ));
        }

        Ok(ToolResult::text(lines.join("\n")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use specd_core::actor;
    use specd_core::command::Command;
    use specd_core::state::SpecState;
    use ulid::Ulid;

    fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        (spec_id, handle)
    }

    #[tokio::test]
    async fn tool_has_correct_name() {
        let (_id, handle) = make_test_actor();
        let tool = ReadStateTool {
            actor: Arc::new(handle),
        };
        assert_eq!(tool.name(), "read_state");
    }

    #[tokio::test]
    async fn tool_has_correct_description() {
        let (_id, handle) = make_test_actor();
        let tool = ReadStateTool {
            actor: Arc::new(handle),
        };
        assert!(tool
            .description()
            .contains("Read the current spec state summary"));
    }

    #[tokio::test]
    async fn tool_schema_is_valid_object() {
        let (_id, handle) = make_test_actor();
        let tool = ReadStateTool {
            actor: Arc::new(handle),
        };
        let schema = tool.schema();
        assert!(schema.is_object());
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    }

    #[tokio::test]
    async fn execute_returns_state_text_empty_spec() {
        let (_id, handle) = make_test_actor();
        let tool = ReadStateTool {
            actor: Arc::new(handle),
        };
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("No spec created yet"));
    }

    #[tokio::test]
    async fn execute_returns_state_text_with_spec() {
        let (_id, handle) = make_test_actor();
        handle
            .send_command(Command::CreateSpec {
                title: "Test Spec".to_string(),
                one_liner: "A test spec".to_string(),
                goal: "Test goal".to_string(),
            })
            .await
            .unwrap();

        handle
            .send_command(Command::CreateCard {
                card_type: "idea".to_string(),
                title: "Test Card".to_string(),
                body: None,
                lane: None,
                created_by: "agent".to_string(),
            })
            .await
            .unwrap();

        let tool = ReadStateTool {
            actor: Arc::new(handle),
        };
        let result = tool.execute(json!({})).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("Test Spec"));
        assert!(result.content.contains("Test Card"));
        assert!(result.content.contains("Cards (1)"));
        assert!(result.content.contains("Lanes:"));
    }
}
