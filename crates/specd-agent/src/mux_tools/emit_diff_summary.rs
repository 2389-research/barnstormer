// ABOUTME: Implements the emit_diff_summary tool for finishing an agent step with a change summary.
// ABOUTME: Sends a FinishAgentStep command to mark the end of an agent's work cycle.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;

use specd_core::actor::SpecActorHandle;
use specd_core::command::Command;

/// Tool that emits a summary of changes made during an agent step.
#[derive(Clone)]
pub struct EmitDiffSummaryTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) agent_id: String,
}

#[async_trait]
impl Tool for EmitDiffSummaryTool {
    fn name(&self) -> &str {
        "emit_diff_summary"
    }

    fn description(&self) -> &str {
        "Emit a summary of changes made during this agent step. Used to describe what was added, modified, or removed."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "A concise summary of the changes made in this step."
                }
            },
            "required": ["summary"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let summary = params
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'summary' parameter"))?
            .to_string();

        self.actor
            .send_command(Command::FinishAgentStep {
                agent_id: self.agent_id.clone(),
                diff_summary: summary,
            })
            .await
            .map_err(|e| anyhow::anyhow!("failed to finish agent step: {}", e))?;

        Ok(ToolResult::text("Step finished"))
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
        let tool = EmitDiffSummaryTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };
        assert_eq!(tool.name(), "emit_diff_summary");
    }

    #[tokio::test]
    async fn tool_has_correct_description() {
        let (_id, handle) = make_test_actor();
        let tool = EmitDiffSummaryTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };
        assert!(tool.description().contains("summary of changes"));
    }

    #[tokio::test]
    async fn tool_schema_is_valid_object() {
        let (_id, handle) = make_test_actor();
        let tool = EmitDiffSummaryTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };
        let schema = tool.schema();
        assert!(schema.is_object());
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    }

    #[tokio::test]
    async fn execute_finishes_agent_step() {
        let (_id, handle) = make_test_actor();
        let tool = EmitDiffSummaryTool {
            actor: Arc::new(handle.clone()),
            agent_id: "summarizer".to_string(),
        };

        let params = json!({ "summary": "Added 3 cards and updated the goal." });
        let result = tool.execute(params).await.unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content, "Step finished");

        // Verify it shows up in transcript as a step-finished message
        let state = handle.read_state().await;
        assert_eq!(state.transcript.len(), 1);
        assert!(state.transcript[0].content.contains("step finished"));
        assert!(state.transcript[0]
            .content
            .contains("Added 3 cards and updated the goal."));
    }

    #[tokio::test]
    async fn execute_errors_on_missing_summary() {
        let (_id, handle) = make_test_actor();
        let tool = EmitDiffSummaryTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
        };

        let result = tool.execute(json!({})).await;
        assert!(result.is_err());
    }
}
