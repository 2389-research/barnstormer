// ABOUTME: Implements the emit_narration tool for posting agent narration to the spec transcript.
// ABOUTME: Sends an AppendTranscript command with the agent's identity as the sender.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;

use specd_core::actor::SpecActorHandle;
use specd_core::command::Command;

/// Tool that emits a narration message to the spec transcript.
#[derive(Clone)]
pub struct EmitNarrationTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) agent_id: String,
}

#[async_trait]
impl Tool for EmitNarrationTool {
    fn name(&self) -> &str {
        "emit_narration"
    }

    fn description(&self) -> &str {
        "Emit a narration message to the spec transcript. Use to explain your reasoning or share observations with the user."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "The narration text to add to the transcript."
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let message = params
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'message' parameter"))?
            .to_string();

        self.actor
            .send_command(Command::AppendTranscript {
                sender: self.agent_id.clone(),
                content: message,
            })
            .await
            .map_err(|e| anyhow::anyhow!("failed to append transcript: {}", e))?;

        Ok(ToolResult::text("Narration posted"))
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
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };
        assert_eq!(tool.name(), "emit_narration");
    }

    #[tokio::test]
    async fn tool_has_correct_description() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };
        assert!(tool.description().contains("narration message"));
    }

    #[tokio::test]
    async fn tool_schema_is_valid_object() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };
        let schema = tool.schema();
        assert!(schema.is_object());
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    }

    #[tokio::test]
    async fn execute_appends_to_transcript() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle.clone()),
            agent_id: "narrator".to_string(),
        };

        let params = json!({ "message": "This is a narration." });
        let result = tool.execute(params).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "Narration posted");

        // Verify the message was appended to the transcript
        let state = handle.read_state().await;
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].sender, "narrator");
        assert_eq!(state.transcript[0].content, "This is a narration.");
    }

    #[tokio::test]
    async fn execute_errors_on_missing_message() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };

        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }
}
