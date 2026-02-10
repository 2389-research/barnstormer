// ABOUTME: Defines the AgentRuntime trait that all LLM provider adapters must implement.
// ABOUTME: Also defines AgentAction (what agents produce) and AgentError (what can go wrong).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use specd_core::command::Command;
use specd_core::transcript::UserQuestion;

use crate::context::AgentContext;

/// The set of actions an agent can produce from a single reasoning step.
/// Each variant maps to one or more commands or internal operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentAction {
    /// Emit a narration message to the transcript.
    EmitNarration(String),

    /// Submit one or more commands to the spec actor.
    WriteCommands(Vec<Command>),

    /// Ask the human user a structured question.
    AskUser(UserQuestion),

    /// Route a question to another agent in the swarm.
    AskAgent { agent_id: String, question: String },

    /// Emit a diff summary describing what changed in this step.
    EmitDiffSummary(String),

    /// Signal that this agent has finished its current work and will idle
    /// until the next relevant event arrives.
    Done,
}

/// Errors that can occur during agent execution.
#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("Provider error: {0}")]
    ProviderError(String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Rate limited")]
    RateLimited,

    #[error("Context too large")]
    ContextTooLarge,
}

/// Trait that all LLM provider adapters must implement. Each provider
/// (Anthropic, OpenAI, Gemini, etc.) translates AgentContext into API
/// calls and parses responses into AgentActions.
#[async_trait]
pub trait AgentRuntime: Send + Sync {
    /// Execute one step of agent reasoning given the current context.
    /// Returns the action(s) the agent wants to take.
    async fn run_step(&self, context: &AgentContext) -> Result<AgentAction, AgentError>;

    /// Provider name for logging and display (e.g. "anthropic", "openai").
    fn provider_name(&self) -> &str;

    /// Model identifier being used (e.g. "claude-sonnet-4-5-20250929").
    fn model_name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use specd_core::command::Command;
    use specd_core::transcript::UserQuestion;
    use ulid::Ulid;

    #[test]
    fn agent_action_variants() {
        // Verify each variant can be constructed and debug-printed.
        let actions: Vec<AgentAction> = vec![
            AgentAction::EmitNarration("Thinking about the spec...".to_string()),
            AgentAction::WriteCommands(vec![Command::CreateCard {
                card_type: "idea".to_string(),
                title: "Test idea".to_string(),
                body: None,
                lane: None,
                created_by: "brainstormer-1".to_string(),
            }]),
            AgentAction::AskUser(UserQuestion::Boolean {
                question_id: Ulid::new(),
                question: "Should we continue?".to_string(),
                default: Some(true),
            }),
            AgentAction::AskAgent {
                agent_id: "planner-1".to_string(),
                question: "What's the priority?".to_string(),
            },
            AgentAction::EmitDiffSummary("Added 2 idea cards".to_string()),
            AgentAction::Done,
        ];

        for action in &actions {
            let debug_str = format!("{:?}", action);
            assert!(!debug_str.is_empty());
        }

        // Verify serde round-trip for each action.
        for action in &actions {
            let json = serde_json::to_string(action).expect("serialize action");
            let deser: AgentAction = serde_json::from_str(&json).expect("deserialize action");
            let json2 = serde_json::to_string(&deser).expect("re-serialize action");
            assert_eq!(json, json2, "round-trip mismatch for action");
        }
    }

    #[test]
    fn agent_error_display() {
        let errors = vec![
            AgentError::ProviderError("connection timeout".to_string()),
            AgentError::InvalidResponse("missing tool_use block".to_string()),
            AgentError::RateLimited,
            AgentError::ContextTooLarge,
        ];

        for err in &errors {
            let msg = err.to_string();
            assert!(!msg.is_empty());
        }

        assert!(
            AgentError::ProviderError("test".to_string())
                .to_string()
                .contains("test")
        );
    }
}
