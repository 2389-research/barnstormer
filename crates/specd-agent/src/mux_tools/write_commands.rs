// ABOUTME: Implements the write_commands tool for submitting spec-mutating commands via mux Tool trait.
// ABOUTME: Parses JSON command arrays and sends each to the actor, reporting successes and failures.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;
use tracing::warn;

use specd_core::actor::SpecActorHandle;
use specd_core::command::Command;

/// Tool that accepts an array of Command objects and sends each to the spec actor.
#[derive(Clone)]
pub struct WriteCommandsTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) agent_id: String,
}

#[async_trait]
impl Tool for WriteCommandsTool {
    fn name(&self) -> &str {
        "write_commands"
    }

    fn description(&self) -> &str {
        "Submit one or more commands to modify the spec. Commands can create/update/move/delete cards, update spec metadata, or append to the transcript."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "commands": {
                    "type": "array",
                    "description": "List of commands to execute against the spec. Each command is an object with a 'type' field.",
                    "items": {
                        "type": "object",
                        "description": "A tagged command object. The 'type' field selects the variant. Valid types and their fields:\n\n- CreateCard: { type: \"CreateCard\", card_type: string (\"idea\"|\"task\"|\"constraint\"|\"risk\"|\"note\"), title: string, body: string|null, lane: string|null (default \"Ideas\"), created_by: string (your agent_id) }\n- UpdateCard: { type: \"UpdateCard\", card_id: string (ULID), title: string|null, body: string|null|null, card_type: string|null, refs: [string]|null, updated_by: string }\n- MoveCard: { type: \"MoveCard\", card_id: string (ULID), lane: string (\"Ideas\"|\"Plan\"|\"Done\"), order: number, updated_by: string }\n- DeleteCard: { type: \"DeleteCard\", card_id: string (ULID), updated_by: string }\n- UpdateSpecCore: { type: \"UpdateSpecCore\", title: string|null, one_liner: string|null, goal: string|null, description: string|null, constraints: string|null, success_criteria: string|null, risks: string|null, notes: string|null }\n- AppendTranscript: { type: \"AppendTranscript\", sender: string (your agent_id), content: string }",
                        "properties": {
                            "type": {
                                "type": "string",
                                "enum": ["CreateCard", "UpdateCard", "MoveCard", "DeleteCard", "UpdateSpecCore", "AppendTranscript"],
                                "description": "The command type to execute."
                            }
                        },
                        "required": ["type"]
                    }
                }
            },
            "required": ["commands"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let commands_value = params
            .get("commands")
            .ok_or_else(|| anyhow::anyhow!("missing 'commands' parameter"))?;

        let commands: Vec<Command> = serde_json::from_value(commands_value.clone())
            .map_err(|e| anyhow::anyhow!("failed to parse commands: {}", e))?;

        if commands.is_empty() {
            return Ok(ToolResult::text("No commands to execute."));
        }

        let total = commands.len();
        let mut successes = 0;
        let mut failures = Vec::new();

        for (i, cmd) in commands.into_iter().enumerate() {
            match self.actor.send_command(cmd).await {
                Ok(events) => {
                    successes += 1;
                    tracing::debug!(
                        agent_id = %self.agent_id,
                        command_index = i,
                        events_produced = events.len(),
                        "command executed successfully"
                    );
                }
                Err(e) => {
                    warn!(
                        agent_id = %self.agent_id,
                        command_index = i,
                        error = %e,
                        "command execution failed"
                    );
                    failures.push(format!("command {}: {}", i, e));
                }
            }
        }

        let summary = if failures.is_empty() {
            format!("All {} commands executed successfully.", total)
        } else {
            format!(
                "{}/{} commands succeeded. Failures:\n{}",
                successes,
                total,
                failures.join("\n")
            )
        };

        Ok(ToolResult::text(summary))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use specd_core::actor;
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
        let tool = WriteCommandsTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };
        assert_eq!(tool.name(), "write_commands");
    }

    #[tokio::test]
    async fn tool_has_correct_description() {
        let (_id, handle) = make_test_actor();
        let tool = WriteCommandsTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };
        assert!(tool.description().contains("Submit one or more commands"));
    }

    #[tokio::test]
    async fn tool_schema_is_valid_object() {
        let (_id, handle) = make_test_actor();
        let tool = WriteCommandsTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };
        let schema = tool.schema();
        assert!(schema.is_object());
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    }

    #[tokio::test]
    async fn execute_creates_card_via_commands() {
        let (_id, handle) = make_test_actor();

        // Create spec first (required for card creation)
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "Test".to_string(),
                goal: "Test".to_string(),
            })
            .await
            .unwrap();

        let tool = WriteCommandsTool {
            actor: Arc::new(handle.clone()),
            agent_id: "test-agent".to_string(),
        };

        let params = json!({
            "commands": [{
                "type": "CreateCard",
                "card_type": "idea",
                "title": "Test Card",
                "body": null,
                "lane": null,
                "created_by": "test-agent"
            }]
        });

        let result = tool.execute(params).await.unwrap();
        assert!(!result.is_error);
        assert!(result.content.contains("1 commands executed successfully"));

        // Verify the card was created
        let state = handle.read_state().await;
        assert_eq!(state.cards.len(), 1);
    }

    #[tokio::test]
    async fn execute_reports_failures() {
        let (_id, handle) = make_test_actor();
        let tool = WriteCommandsTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };

        // Try to update a card that doesn't exist (spec not created)
        let params = json!({
            "commands": [{
                "type": "UpdateCard",
                "card_id": "01JMTEST000000000000000000",
                "title": "Ghost",
                "body": null,
                "card_type": null,
                "refs": null,
                "updated_by": "test-agent"
            }]
        });

        let result = tool.execute(params).await.unwrap();
        assert!(result.content.contains("Failures"));
    }

    #[tokio::test]
    async fn execute_empty_commands() {
        let (_id, handle) = make_test_actor();
        let tool = WriteCommandsTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };

        let params = json!({ "commands": [] });
        let result = tool.execute(params).await.unwrap();
        assert!(result.content.contains("No commands to execute"));
    }
}
