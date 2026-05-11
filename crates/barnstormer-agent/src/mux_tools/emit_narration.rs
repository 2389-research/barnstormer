// ABOUTME: Implements the emit_narration tool. Accepts either a pre-written message
// ABOUTME: (legacy path) or structured intent+points that get rendered via a NarrationRenderer.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;

use barnstormer_core::actor::SpecActorHandle;
use barnstormer_core::command::Command;

use crate::narration_renderer::{NarrationIntent, NarrationRenderer};

/// Tool that emits a narration message to the spec transcript.
///
/// Two call shapes are accepted:
///
/// 1. Legacy: `{"message": "..."}` — full prose authored by the calling agent.
///    The text is posted verbatim. Backward compatible with every existing
///    Manager / Brainstormer / Planner / DotGen / Critic system prompt.
///
/// 2. Structured: `{"intent": "...", "points": [...]}` — the calling agent
///    supplies a voice-library intent and an ordered list of points; the
///    NarrationRenderer expands them into prose with the right shape for the
///    intent (typically via a Haiku-class model). Cheaper than authoring prose
///    in Sonnet output tokens.
///
/// If both `message` and (`intent` or `points`) are supplied, `message` wins
/// — the legacy path is preserved for any tool-use call that pre-dates the
/// structured shape.
#[derive(Clone)]
pub struct EmitNarrationTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) agent_id: String,
    pub(crate) renderer: Option<Arc<dyn NarrationRenderer>>,
}

#[async_trait]
impl Tool for EmitNarrationTool {
    fn name(&self) -> &str {
        "emit_narration"
    }

    fn description(&self) -> &str {
        "Emit a narration message to the spec transcript. Use to explain your reasoning or share observations with the user. \
         Prefer the structured form (intent + points) for typical narrations — it's cheaper. Use the legacy 'message' field \
         only when you need exact control over the prose."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Pre-written narration text. Use this when you need exact control over the prose. \
                                    If omitted, you must supply 'intent' and 'points' so the system can render the narration."
                },
                "intent": {
                    "type": "string",
                    "enum": NarrationIntent::all_wire_values(),
                    "description": "What kind of narration this is. Drives voice and length on the rendering side:\n\
                                    - structural_analysis: 2-4 analytical paragraphs referring to specific cards/lanes\n\
                                    - gap_identification: short 'Critical Gaps' bulleted list\n\
                                    - completion_summary: 1-2 paragraph recap of what was just accomplished\n\
                                    - user_acknowledgment: 1-3 conversational sentences\n\
                                    - step_explanation: brief paragraph on the next planned step\n\
                                    - phase_transition_recap: 1-2 paragraph recap of progress through the prior phase\n\
                                    - exploratory_brainstorm: bulleted raw ideas with brief framing"
                },
                "points": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Ordered key points the narration should convey. The renderer expands these in the right voice."
                },
                "spec_state_relevant": {
                    "type": "boolean",
                    "description": "Optional hint: set true when the narration should reference specific cards/lanes/relationships from the current spec state."
                }
            }
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        // Legacy path: explicit `message` wins. Empty/missing message and no
        // intent is a tool-call error (helps the agent learn the right shape).
        let message_param = params.get("message").and_then(|v| v.as_str());

        let content = match message_param {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => self.render_from_intent(&params).await?,
        };

        self.actor
            .send_command(Command::AppendTranscript {
                sender: self.agent_id.clone(),
                content,
            })
            .await
            .map_err(|e| anyhow::anyhow!("failed to append transcript: {}", e))?;

        Ok(ToolResult::text("Narration posted"))
    }
}

impl EmitNarrationTool {
    /// Render prose from structured intent+points args. Returns an error if
    /// the args are malformed or if no renderer is configured for this tool
    /// (renderer is optional so the tool is still usable in test harnesses
    /// without an LLM client).
    async fn render_from_intent(
        &self,
        params: &serde_json::Value,
    ) -> Result<String, anyhow::Error> {
        let intent_str = params
            .get("intent")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "emit_narration requires either 'message' or both 'intent' and 'points'"
                )
            })?;
        let intent = NarrationIntent::from_wire(intent_str).ok_or_else(|| {
            anyhow::anyhow!(
                "unknown intent '{}'; valid values: {:?}",
                intent_str,
                NarrationIntent::all_wire_values()
            )
        })?;

        let points_val = params
            .get("points")
            .ok_or_else(|| anyhow::anyhow!("'points' is required when using the structured shape"))?;
        let points_arr = points_val
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("'points' must be an array of strings"))?;
        if points_arr.is_empty() {
            return Err(anyhow::anyhow!("'points' must not be empty"));
        }
        let points: Vec<String> = points_arr
            .iter()
            .map(|v| match v.as_str() {
                Some(s) => Ok(s.to_string()),
                None => Err(anyhow::anyhow!("each element of 'points' must be a string")),
            })
            .collect::<Result<Vec<_>, _>>()?;

        let spec_state_relevant = params
            .get("spec_state_relevant")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let renderer = self.renderer.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "emit_narration was called with structured intent but no NarrationRenderer is configured"
            )
        })?;

        renderer
            .render(intent, &points, spec_state_relevant)
            .await
            .map_err(|e| anyhow::anyhow!("narration render failed: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::actor;
    use barnstormer_core::state::SpecState;
    use ulid::Ulid;

    #[derive(Debug)]
    struct StubRenderer;

    #[async_trait::async_trait]
    impl NarrationRenderer for StubRenderer {
        async fn render(
            &self,
            intent: NarrationIntent,
            points: &[String],
            _spec_state_relevant: bool,
        ) -> Result<String, String> {
            Ok(format!("[{:?}] {}", intent, points.join(" / ")))
        }
    }

    fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        (spec_id, handle)
    }

    fn renderer() -> Option<Arc<dyn NarrationRenderer>> {
        Some(Arc::new(StubRenderer))
    }

    #[tokio::test]
    async fn tool_has_correct_name() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
            renderer: None,
        };
        assert_eq!(tool.name(), "emit_narration");
    }

    #[tokio::test]
    async fn schema_lists_all_intent_values() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "test-agent".to_string(),
            renderer: None,
        };
        let schema = tool.schema();
        let enum_arr = schema
            .pointer("/properties/intent/enum")
            .and_then(|v| v.as_array())
            .expect("intent.enum should be an array");
        assert_eq!(enum_arr.len(), 7);
        let names: Vec<&str> = enum_arr.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"structural_analysis"));
        assert!(names.contains(&"user_acknowledgment"));
        assert!(names.contains(&"exploratory_brainstorm"));
    }

    #[tokio::test]
    async fn legacy_message_path_still_works() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle.clone()),
            agent_id: "narrator".to_string(),
            renderer: None,
        };

        let result = tool
            .execute(json!({ "message": "Pre-written narration." }))
            .await
            .unwrap();
        assert!(!result.is_error);

        let state = handle.read_state().await;
        assert_eq!(state.transcript.len(), 1);
        assert_eq!(state.transcript[0].sender, "narrator");
        assert_eq!(state.transcript[0].content, "Pre-written narration.");
    }

    #[tokio::test]
    async fn structured_path_dispatches_to_renderer() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle.clone()),
            agent_id: "narrator".to_string(),
            renderer: renderer(),
        };

        let result = tool
            .execute(json!({
                "intent": "step_explanation",
                "points": ["Reading state first", "Then writing cards"],
            }))
            .await
            .unwrap();
        assert!(!result.is_error);

        let state = handle.read_state().await;
        assert_eq!(state.transcript.len(), 1);
        let posted = &state.transcript[0].content;
        // Stub renderer echoes the intent debug + points
        assert!(posted.contains("StepExplanation"));
        assert!(posted.contains("Reading state first"));
        assert!(posted.contains("Then writing cards"));
    }

    #[tokio::test]
    async fn message_wins_over_structured_when_both_provided() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle.clone()),
            agent_id: "narrator".to_string(),
            renderer: renderer(),
        };

        tool.execute(json!({
            "message": "Verbatim text wins",
            "intent": "step_explanation",
            "points": ["ignored point"],
        }))
        .await
        .unwrap();

        let state = handle.read_state().await;
        assert_eq!(state.transcript[0].content, "Verbatim text wins");
    }

    #[tokio::test]
    async fn errors_on_neither_message_nor_intent() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "narrator".to_string(),
            renderer: renderer(),
        };

        let err = tool.execute(json!({})).await.unwrap_err();
        assert!(err.to_string().contains("'message' or both 'intent' and 'points'"));
    }

    #[tokio::test]
    async fn errors_on_empty_message_without_intent() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "narrator".to_string(),
            renderer: renderer(),
        };

        let err = tool
            .execute(json!({ "message": "   " }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("'message' or both 'intent' and 'points'"));
    }

    #[tokio::test]
    async fn errors_on_unknown_intent() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "narrator".to_string(),
            renderer: renderer(),
        };

        let err = tool
            .execute(json!({
                "intent": "nonexistent_intent",
                "points": ["a"],
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("unknown intent"));
    }

    #[tokio::test]
    async fn errors_on_intent_without_points() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "narrator".to_string(),
            renderer: renderer(),
        };

        let err = tool
            .execute(json!({ "intent": "step_explanation" }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("'points' is required"));
    }

    #[tokio::test]
    async fn errors_on_empty_points_array() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "narrator".to_string(),
            renderer: renderer(),
        };

        let err = tool
            .execute(json!({
                "intent": "step_explanation",
                "points": [],
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[tokio::test]
    async fn errors_when_structured_path_used_without_renderer_configured() {
        let (_id, handle) = make_test_actor();
        let tool = EmitNarrationTool {
            actor: Arc::new(handle),
            agent_id: "narrator".to_string(),
            renderer: None, // not configured
        };

        let err = tool
            .execute(json!({
                "intent": "step_explanation",
                "points": ["a", "b"],
            }))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no NarrationRenderer is configured"));
    }
}
