// ABOUTME: Implements three ask_user tool variants (boolean, multiple_choice, freeform) via mux Tool trait.
// ABOUTME: Each tool gates on an AtomicBool to prevent multiple concurrent questions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;
use ulid::Ulid;

use specd_core::actor::SpecActorHandle;
use specd_core::command::Command;
use specd_core::transcript::UserQuestion;

// ---------------------------------------------------------------------------
// ask_user_boolean
// ---------------------------------------------------------------------------

/// Tool that asks the user a yes/no question.
#[derive(Clone)]
pub struct AskUserBooleanTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) question_pending: Arc<AtomicBool>,
    /// Retained for symmetry with other tools and future use in question metadata.
    #[allow(dead_code)]
    pub(crate) agent_id: String,
}

#[async_trait]
impl Tool for AskUserBooleanTool {
    fn name(&self) -> &str {
        "ask_user_boolean"
    }

    fn description(&self) -> &str {
        "Ask the user a yes/no question. Use when you need a simple binary decision from the human."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The yes/no question to ask the user."
                },
                "default": {
                    "type": "boolean",
                    "description": "Optional default answer (true for yes, false for no)."
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        if self.question_pending.load(Ordering::SeqCst) {
            return Ok(ToolResult::text("Question already pending, skipping"));
        }

        let question_text = params
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'question' parameter"))?
            .to_string();

        let default = params.get("default").and_then(|v| v.as_bool());

        let question = UserQuestion::Boolean {
            question_id: Ulid::new(),
            question: question_text,
            default,
        };

        self.actor
            .send_command(Command::AskQuestion { question })
            .await
            .map_err(|e| anyhow::anyhow!("failed to ask question: {}", e))?;

        self.question_pending.store(true, Ordering::SeqCst);

        Ok(ToolResult::text("Question asked"))
    }
}

// ---------------------------------------------------------------------------
// ask_user_multiple_choice
// ---------------------------------------------------------------------------

/// Tool that asks the user to choose from a list of options.
#[derive(Clone)]
pub struct AskUserMultipleChoiceTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) question_pending: Arc<AtomicBool>,
    /// Retained for symmetry with other tools and future use in question metadata.
    #[allow(dead_code)]
    pub(crate) agent_id: String,
}

#[async_trait]
impl Tool for AskUserMultipleChoiceTool {
    fn name(&self) -> &str {
        "ask_user_multiple_choice"
    }

    fn description(&self) -> &str {
        "Ask the user to choose from a list of options. Use when you have specific alternatives to present."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to present along with the choices."
                },
                "choices": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "List of choices for the user to select from."
                },
                "allow_multi": {
                    "type": "boolean",
                    "description": "Whether the user can select multiple choices. Defaults to false."
                }
            },
            "required": ["question", "choices"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        if self.question_pending.load(Ordering::SeqCst) {
            return Ok(ToolResult::text("Question already pending, skipping"));
        }

        let question_text = params
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'question' parameter"))?
            .to_string();

        let choices: Vec<String> = params
            .get("choices")
            .ok_or_else(|| anyhow::anyhow!("missing 'choices' parameter"))?
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("'choices' must be an array"))?
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();

        let allow_multi = params
            .get("allow_multi")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let question = UserQuestion::MultipleChoice {
            question_id: Ulid::new(),
            question: question_text,
            choices,
            allow_multi,
        };

        self.actor
            .send_command(Command::AskQuestion { question })
            .await
            .map_err(|e| anyhow::anyhow!("failed to ask question: {}", e))?;

        self.question_pending.store(true, Ordering::SeqCst);

        Ok(ToolResult::text("Question asked"))
    }
}

// ---------------------------------------------------------------------------
// ask_user_freeform
// ---------------------------------------------------------------------------

/// Tool that asks the user a free-form question.
#[derive(Clone)]
pub struct AskUserFreeformTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) question_pending: Arc<AtomicBool>,
    /// Retained for symmetry with other tools and future use in question metadata.
    #[allow(dead_code)]
    pub(crate) agent_id: String,
}

#[async_trait]
impl Tool for AskUserFreeformTool {
    fn name(&self) -> &str {
        "ask_user_freeform"
    }

    fn description(&self) -> &str {
        "Ask the user a free-form question. Use when you need detailed or unstructured input."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "question": {
                    "type": "string",
                    "description": "The question to ask the user."
                },
                "placeholder": {
                    "type": "string",
                    "description": "Optional placeholder text for the input field."
                },
                "validation_hint": {
                    "type": "string",
                    "description": "Optional hint about expected format or content."
                }
            },
            "required": ["question"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        if self.question_pending.load(Ordering::SeqCst) {
            return Ok(ToolResult::text("Question already pending, skipping"));
        }

        let question_text = params
            .get("question")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'question' parameter"))?
            .to_string();

        let placeholder = params
            .get("placeholder")
            .and_then(|v| v.as_str())
            .map(String::from);

        let validation_hint = params
            .get("validation_hint")
            .and_then(|v| v.as_str())
            .map(String::from);

        let question = UserQuestion::Freeform {
            question_id: Ulid::new(),
            question: question_text,
            placeholder,
            validation_hint,
        };

        self.actor
            .send_command(Command::AskQuestion { question })
            .await
            .map_err(|e| anyhow::anyhow!("failed to ask question: {}", e))?;

        self.question_pending.store(true, Ordering::SeqCst);

        Ok(ToolResult::text("Question asked"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use specd_core::actor;
    use specd_core::state::SpecState;

    fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        (spec_id, handle)
    }

    fn make_pending_flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    // --- ask_user_boolean tests ---

    #[tokio::test]
    async fn boolean_tool_has_correct_name() {
        let (_id, handle) = make_test_actor();
        let tool = AskUserBooleanTool {
            actor: Arc::new(handle),
            question_pending: make_pending_flag(),
            agent_id: "test".to_string(),
        };
        assert_eq!(tool.name(), "ask_user_boolean");
    }

    #[tokio::test]
    async fn boolean_tool_schema_is_valid_object() {
        let (_id, handle) = make_test_actor();
        let tool = AskUserBooleanTool {
            actor: Arc::new(handle),
            question_pending: make_pending_flag(),
            agent_id: "test".to_string(),
        };
        let schema = tool.schema();
        assert!(schema.is_object());
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    }

    #[tokio::test]
    async fn boolean_sets_question_pending_flag() {
        let (_id, handle) = make_test_actor();
        let pending = make_pending_flag();
        let tool = AskUserBooleanTool {
            actor: Arc::new(handle),
            question_pending: pending.clone(),
            agent_id: "test".to_string(),
        };

        assert!(!pending.load(Ordering::SeqCst));

        let result = tool
            .execute(json!({ "question": "Continue?" }))
            .await
            .unwrap();
        assert_eq!(result.content, "Question asked");
        assert!(pending.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn boolean_returns_already_pending_when_flag_set() {
        let (_id, handle) = make_test_actor();
        let pending = Arc::new(AtomicBool::new(true));
        let tool = AskUserBooleanTool {
            actor: Arc::new(handle),
            question_pending: pending,
            agent_id: "test".to_string(),
        };

        let result = tool
            .execute(json!({ "question": "Another?" }))
            .await
            .unwrap();
        assert_eq!(result.content, "Question already pending, skipping");
    }

    #[tokio::test]
    async fn boolean_creates_pending_question_in_state() {
        let (_id, handle) = make_test_actor();
        let pending = make_pending_flag();
        let tool = AskUserBooleanTool {
            actor: Arc::new(handle.clone()),
            question_pending: pending,
            agent_id: "test".to_string(),
        };

        tool.execute(json!({ "question": "Is this working?", "default": true }))
            .await
            .unwrap();

        let state = handle.read_state().await;
        assert!(state.pending_question.is_some());
        match &state.pending_question {
            Some(UserQuestion::Boolean {
                question, default, ..
            }) => {
                assert_eq!(question, "Is this working?");
                assert_eq!(*default, Some(true));
            }
            other => panic!("expected Boolean question, got: {:?}", other),
        }
    }

    // --- ask_user_multiple_choice tests ---

    #[tokio::test]
    async fn multiple_choice_tool_has_correct_name() {
        let (_id, handle) = make_test_actor();
        let tool = AskUserMultipleChoiceTool {
            actor: Arc::new(handle),
            question_pending: make_pending_flag(),
            agent_id: "test".to_string(),
        };
        assert_eq!(tool.name(), "ask_user_multiple_choice");
    }

    #[tokio::test]
    async fn multiple_choice_tool_schema_is_valid_object() {
        let (_id, handle) = make_test_actor();
        let tool = AskUserMultipleChoiceTool {
            actor: Arc::new(handle),
            question_pending: make_pending_flag(),
            agent_id: "test".to_string(),
        };
        let schema = tool.schema();
        assert!(schema.is_object());
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    }

    #[tokio::test]
    async fn multiple_choice_creates_question() {
        let (_id, handle) = make_test_actor();
        let pending = make_pending_flag();
        let tool = AskUserMultipleChoiceTool {
            actor: Arc::new(handle.clone()),
            question_pending: pending,
            agent_id: "test".to_string(),
        };

        let result = tool
            .execute(json!({
                "question": "Pick a color",
                "choices": ["red", "blue", "green"],
                "allow_multi": true
            }))
            .await
            .unwrap();
        assert_eq!(result.content, "Question asked");

        let state = handle.read_state().await;
        match &state.pending_question {
            Some(UserQuestion::MultipleChoice {
                question,
                choices,
                allow_multi,
                ..
            }) => {
                assert_eq!(question, "Pick a color");
                assert_eq!(choices.len(), 3);
                assert!(*allow_multi);
            }
            other => panic!("expected MultipleChoice question, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn multiple_choice_returns_already_pending() {
        let (_id, handle) = make_test_actor();
        let pending = Arc::new(AtomicBool::new(true));
        let tool = AskUserMultipleChoiceTool {
            actor: Arc::new(handle),
            question_pending: pending,
            agent_id: "test".to_string(),
        };

        let result = tool
            .execute(json!({ "question": "Pick", "choices": ["a", "b"] }))
            .await
            .unwrap();
        assert_eq!(result.content, "Question already pending, skipping");
    }

    // --- ask_user_freeform tests ---

    #[tokio::test]
    async fn freeform_tool_has_correct_name() {
        let (_id, handle) = make_test_actor();
        let tool = AskUserFreeformTool {
            actor: Arc::new(handle),
            question_pending: make_pending_flag(),
            agent_id: "test".to_string(),
        };
        assert_eq!(tool.name(), "ask_user_freeform");
    }

    #[tokio::test]
    async fn freeform_tool_schema_is_valid_object() {
        let (_id, handle) = make_test_actor();
        let tool = AskUserFreeformTool {
            actor: Arc::new(handle),
            question_pending: make_pending_flag(),
            agent_id: "test".to_string(),
        };
        let schema = tool.schema();
        assert!(schema.is_object());
        assert_eq!(schema.get("type").and_then(|v| v.as_str()), Some("object"));
    }

    #[tokio::test]
    async fn freeform_creates_question() {
        let (_id, handle) = make_test_actor();
        let pending = make_pending_flag();
        let tool = AskUserFreeformTool {
            actor: Arc::new(handle.clone()),
            question_pending: pending,
            agent_id: "test".to_string(),
        };

        let result = tool
            .execute(json!({
                "question": "Describe your feature",
                "placeholder": "Type here...",
                "validation_hint": "Be specific"
            }))
            .await
            .unwrap();
        assert_eq!(result.content, "Question asked");

        let state = handle.read_state().await;
        match &state.pending_question {
            Some(UserQuestion::Freeform {
                question,
                placeholder,
                validation_hint,
                ..
            }) => {
                assert_eq!(question, "Describe your feature");
                assert_eq!(placeholder.as_deref(), Some("Type here..."));
                assert_eq!(validation_hint.as_deref(), Some("Be specific"));
            }
            other => panic!("expected Freeform question, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn freeform_returns_already_pending() {
        let (_id, handle) = make_test_actor();
        let pending = Arc::new(AtomicBool::new(true));
        let tool = AskUserFreeformTool {
            actor: Arc::new(handle),
            question_pending: pending,
            agent_id: "test".to_string(),
        };

        let result = tool
            .execute(json!({ "question": "What?" }))
            .await
            .unwrap();
        assert_eq!(result.content, "Question already pending, skipping");
    }
}
