// ABOUTME: Anthropic Claude API adapter implementing the AgentRuntime trait.
// ABOUTME: Translates AgentContext into Anthropic Messages API calls and parses tool_use responses.

use async_trait::async_trait;
use serde_json::{Value, json};
use ulid::Ulid;

use specd_core::command::Command;
use specd_core::transcript::UserQuestion;

use crate::context::AgentContext;
use crate::providers::role_prompt;
use crate::runtime::{AgentAction, AgentError, AgentRuntime};
use crate::tools::all_tool_definitions;

const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_MODEL: &str = "claude-sonnet-4-5-20250929";
const API_VERSION: &str = "2023-06-01";
const MAX_TOKENS: u32 = 4096;

/// Anthropic Claude runtime adapter. Calls the Messages API with tool definitions
/// and maps tool_use responses back to AgentActions.
pub struct AnthropicRuntime {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl AnthropicRuntime {
    /// Create a new AnthropicRuntime reading configuration from environment variables.
    /// Required: `ANTHROPIC_API_KEY`
    /// Optional: `ANTHROPIC_BASE_URL` (defaults to https://api.anthropic.com)
    /// Optional: `ANTHROPIC_MODEL` (defaults to claude-sonnet-4-5-20250929)
    pub fn from_env() -> Result<Self, AgentError> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| AgentError::ProviderError("ANTHROPIC_API_KEY not set".to_string()))?;

        let base_url =
            std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());

        let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Ok(Self::new(api_key, base_url, model))
    }

    /// Create a new AnthropicRuntime with explicit configuration.
    pub fn new(api_key: String, base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url,
            model,
        }
    }

    /// Build the JSON request body for the Anthropic Messages API.
    pub fn build_request_body(&self, context: &AgentContext) -> Value {
        let system_prompt = role_prompt(&context.agent_role, &context.state_summary);

        let tools = build_anthropic_tools();

        // Build conversation messages from context
        let mut messages = Vec::new();

        // Add rolling summary as initial assistant context if present
        if !context.rolling_summary.is_empty() {
            messages.push(json!({
                "role": "user",
                "content": format!("[Context] Rolling summary: {}", context.rolling_summary)
            }));
        }

        // Add recent transcript as conversation history
        for msg in &context.recent_transcript {
            let role = if msg.sender == "human" {
                "user"
            } else {
                "assistant"
            };
            messages.push(json!({
                "role": role,
                "content": msg.content
            }));
        }

        // Add current state summary as the final user message
        if !context.state_summary.is_empty() {
            messages.push(json!({
                "role": "user",
                "content": format!(
                    "[Current state] {}\n\n[Key decisions] {}\n\nWhat should we do next?",
                    context.state_summary,
                    context.key_decisions.join("; ")
                )
            }));
        }

        // Ensure there's at least one user message
        if messages.is_empty() {
            messages.push(json!({
                "role": "user",
                "content": "The spec is empty. What should we do to get started?"
            }));
        }

        // Ensure messages alternate roles (Anthropic API requirement)
        let messages = coalesce_messages(messages);

        json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": system_prompt,
            "messages": messages,
            "tools": tools
        })
    }

    /// Parse an Anthropic Messages API response into an AgentAction.
    pub fn parse_response(response_body: &Value) -> Result<AgentAction, AgentError> {
        let content = response_body
            .get("content")
            .and_then(|c| c.as_array())
            .ok_or_else(|| {
                AgentError::InvalidResponse("missing content array in response".to_string())
            })?;

        // Look for tool_use blocks first — they take priority
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") {
                return parse_tool_use(block);
            }
        }

        // If no tool_use, look for text content (narration)
        for block in content {
            if block.get("type").and_then(|t| t.as_str()) == Some("text")
                && let Some(text) = block.get("text").and_then(|t| t.as_str())
                && !text.is_empty()
            {
                return Ok(AgentAction::EmitNarration(text.to_string()));
            }
        }

        // If stop_reason is "end_turn" with no actionable content, agent is done
        let stop_reason = response_body
            .get("stop_reason")
            .and_then(|s| s.as_str())
            .unwrap_or("");

        if stop_reason == "end_turn" {
            return Ok(AgentAction::Done);
        }

        Err(AgentError::InvalidResponse(
            "no actionable content in response".to_string(),
        ))
    }
}

/// Convert tool definitions to Anthropic's tool format.
fn build_anthropic_tools() -> Vec<Value> {
    all_tool_definitions()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.get("name").cloned().unwrap_or(Value::Null),
                "description": tool.get("description").cloned().unwrap_or(Value::Null),
                "input_schema": tool.get("parameters").cloned().unwrap_or(json!({"type": "object"}))
            })
        })
        .collect()
}

/// Parse a single tool_use block from the Anthropic response into an AgentAction.
fn parse_tool_use(block: &Value) -> Result<AgentAction, AgentError> {
    let tool_name = block
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| AgentError::InvalidResponse("tool_use block missing name".to_string()))?;

    let input = block.get("input").cloned().unwrap_or(json!({}));

    match tool_name {
        "ask_user_boolean" => {
            let question = input
                .get("question")
                .and_then(|q| q.as_str())
                .unwrap_or("Yes or no?")
                .to_string();
            let default = input.get("default").and_then(|d| d.as_bool());

            Ok(AgentAction::AskUser(UserQuestion::Boolean {
                question_id: Ulid::new(),
                question,
                default,
            }))
        }

        "ask_user_multiple_choice" => {
            let question = input
                .get("question")
                .and_then(|q| q.as_str())
                .unwrap_or("Choose one:")
                .to_string();
            let choices = input
                .get("choices")
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let allow_multi = input
                .get("allow_multi")
                .and_then(|a| a.as_bool())
                .unwrap_or(false);

            Ok(AgentAction::AskUser(UserQuestion::MultipleChoice {
                question_id: Ulid::new(),
                question,
                choices,
                allow_multi,
            }))
        }

        "ask_user_freeform" => {
            let question = input
                .get("question")
                .and_then(|q| q.as_str())
                .unwrap_or("Please elaborate:")
                .to_string();
            let placeholder = input
                .get("placeholder")
                .and_then(|p| p.as_str())
                .map(String::from);
            let validation_hint = input
                .get("validation_hint")
                .and_then(|v| v.as_str())
                .map(String::from);

            Ok(AgentAction::AskUser(UserQuestion::Freeform {
                question_id: Ulid::new(),
                question,
                placeholder,
                validation_hint,
            }))
        }

        "read_state" => {
            // read_state is an internal tool; the orchestrator handles it.
            // Return Done to signal the agent should receive updated state.
            Ok(AgentAction::Done)
        }

        "write_commands" => {
            let commands_value = input.get("commands").cloned().unwrap_or(json!([]));
            let commands: Vec<Command> = serde_json::from_value(commands_value).map_err(|e| {
                AgentError::InvalidResponse(format!("failed to parse commands: {}", e))
            })?;

            Ok(AgentAction::WriteCommands(commands))
        }

        "emit_narration" => {
            let message = input
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();

            Ok(AgentAction::EmitNarration(message))
        }

        "emit_diff_summary" => {
            let summary = input
                .get("summary")
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();

            Ok(AgentAction::EmitDiffSummary(summary))
        }

        other => Err(AgentError::InvalidResponse(format!(
            "unknown tool: {}",
            other
        ))),
    }
}

/// Coalesce consecutive messages with the same role into single messages.
/// The Anthropic API requires alternating user/assistant messages.
fn coalesce_messages(messages: Vec<Value>) -> Vec<Value> {
    if messages.is_empty() {
        return messages;
    }

    let mut result: Vec<Value> = Vec::new();

    for msg in messages {
        let role = msg.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let content = msg
            .get("content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(last) = result.last_mut() {
            let last_role = last.get("role").and_then(|r| r.as_str()).unwrap_or("");

            if last_role == role {
                // Merge into previous message
                let prev_content = last.get("content").and_then(|c| c.as_str()).unwrap_or("");
                let merged = format!("{}\n\n{}", prev_content, content);
                *last = json!({
                    "role": role,
                    "content": merged
                });
                continue;
            }
        }

        result.push(json!({
            "role": role,
            "content": content
        }));
    }

    result
}

#[async_trait]
impl AgentRuntime for AnthropicRuntime {
    async fn run_step(&self, context: &AgentContext) -> Result<AgentAction, AgentError> {
        let body = self.build_request_body(context);
        let url = format!("{}/v1/messages", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::ProviderError(format!("HTTP request failed: {}", e)))?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AgentError::RateLimited);
        }

        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AgentError::ProviderError(
                "Unauthorized: check ANTHROPIC_API_KEY".to_string(),
            ));
        }

        if status.is_server_error() {
            return Err(AgentError::ProviderError(format!(
                "Server error: {}",
                status
            )));
        }

        if !status.is_success() {
            let error_body = response.text().await.unwrap_or_default();
            return Err(AgentError::ProviderError(format!(
                "API error {}: {}",
                status, error_body
            )));
        }

        let response_body: Value = response
            .json()
            .await
            .map_err(|e| AgentError::InvalidResponse(format!("failed to parse JSON: {}", e)))?;

        Self::parse_response(&response_body)
    }

    fn provider_name(&self) -> &str {
        "anthropic"
    }

    fn model_name(&self) -> &str {
        &self.model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{AgentContext, AgentRole};

    #[test]
    fn anthropic_runtime_creation() {
        let runtime = AnthropicRuntime::new(
            "test-key".to_string(),
            "https://api.anthropic.com".to_string(),
            "claude-sonnet-4-5-20250929".to_string(),
        );

        assert_eq!(runtime.provider_name(), "anthropic");
        assert_eq!(runtime.model_name(), "claude-sonnet-4-5-20250929");
        assert_eq!(runtime.api_key, "test-key");
        assert_eq!(runtime.base_url, "https://api.anthropic.com");
    }

    #[test]
    fn anthropic_builds_request_body() {
        let runtime = AnthropicRuntime::new(
            "test-key".to_string(),
            "https://api.anthropic.com".to_string(),
            "claude-sonnet-4-5-20250929".to_string(),
        );

        let spec_id = Ulid::new();
        let mut ctx = AgentContext::new(
            spec_id,
            "brainstormer-1".to_string(),
            AgentRole::Brainstormer,
        );
        ctx.state_summary = "A spec about building a widget".to_string();
        ctx.rolling_summary = "Previously discussed widget design".to_string();

        let body = runtime.build_request_body(&ctx);

        // Verify model
        assert_eq!(
            body.get("model").and_then(|m| m.as_str()),
            Some("claude-sonnet-4-5-20250929")
        );

        // Verify max_tokens
        assert_eq!(body.get("max_tokens").and_then(|m| m.as_u64()), Some(4096));

        // Verify system prompt is present and contains the role
        let system = body.get("system").and_then(|s| s.as_str()).unwrap();
        assert!(system.contains("brainstormer"));

        // Verify messages array exists and is non-empty
        let messages = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert!(!messages.is_empty());

        // Verify tools array exists
        let tools = body.get("tools").and_then(|t| t.as_array()).unwrap();
        assert_eq!(tools.len(), 7);

        // Verify each tool has the Anthropic format (input_schema, not parameters)
        for tool in tools {
            assert!(tool.get("name").is_some());
            assert!(tool.get("description").is_some());
            assert!(tool.get("input_schema").is_some());
        }
    }

    #[test]
    fn anthropic_parses_tool_use_response() {
        // Simulate a tool_use response for ask_user_boolean
        let response = json!({
            "id": "msg_123",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_123",
                    "name": "ask_user_boolean",
                    "input": {
                        "question": "Should we add authentication?",
                        "default": true
                    }
                }
            ],
            "stop_reason": "tool_use"
        });

        let action = AnthropicRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::AskUser(UserQuestion::Boolean {
                question, default, ..
            }) => {
                assert_eq!(question, "Should we add authentication?");
                assert_eq!(default, Some(true));
            }
            other => panic!("expected AskUser(Boolean), got {:?}", other),
        }
    }

    #[test]
    fn anthropic_parses_narration_response() {
        let response = json!({
            "id": "msg_456",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "text",
                    "text": "I'm analyzing the requirements and thinking about edge cases."
                }
            ],
            "stop_reason": "end_turn"
        });

        let action = AnthropicRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::EmitNarration(text) => {
                assert!(text.contains("analyzing the requirements"));
            }
            other => panic!("expected EmitNarration, got {:?}", other),
        }
    }

    #[test]
    fn anthropic_parses_emit_narration_tool() {
        let response = json!({
            "id": "msg_789",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_456",
                    "name": "emit_narration",
                    "input": {
                        "message": "Exploring the design space..."
                    }
                }
            ],
            "stop_reason": "tool_use"
        });

        let action = AnthropicRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::EmitNarration(msg) => {
                assert_eq!(msg, "Exploring the design space...");
            }
            other => panic!("expected EmitNarration, got {:?}", other),
        }
    }

    #[test]
    fn anthropic_parses_write_commands_tool() {
        let response = json!({
            "id": "msg_abc",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_789",
                    "name": "write_commands",
                    "input": {
                        "commands": [
                            {
                                "type": "CreateCard",
                                "card_type": "idea",
                                "title": "Add caching layer",
                                "body": "Redis-based caching for API responses",
                                "lane": "Ideas",
                                "created_by": "brainstormer-1"
                            }
                        ]
                    }
                }
            ],
            "stop_reason": "tool_use"
        });

        let action = AnthropicRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::WriteCommands(cmds) => {
                assert_eq!(cmds.len(), 1);
            }
            other => panic!("expected WriteCommands, got {:?}", other),
        }
    }

    #[test]
    fn anthropic_parses_multiple_choice_tool() {
        let response = json!({
            "id": "msg_mc",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_mc",
                    "name": "ask_user_multiple_choice",
                    "input": {
                        "question": "Which database?",
                        "choices": ["PostgreSQL", "SQLite", "DynamoDB"],
                        "allow_multi": false
                    }
                }
            ],
            "stop_reason": "tool_use"
        });

        let action = AnthropicRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::AskUser(UserQuestion::MultipleChoice {
                question,
                choices,
                allow_multi,
                ..
            }) => {
                assert_eq!(question, "Which database?");
                assert_eq!(choices.len(), 3);
                assert!(!allow_multi);
            }
            other => panic!("expected AskUser(MultipleChoice), got {:?}", other),
        }
    }

    #[test]
    fn anthropic_parses_freeform_tool() {
        let response = json!({
            "id": "msg_ff",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_ff",
                    "name": "ask_user_freeform",
                    "input": {
                        "question": "Describe the authentication requirements",
                        "placeholder": "e.g. OAuth2, API keys, JWT...",
                        "validation_hint": "Be specific about supported providers"
                    }
                }
            ],
            "stop_reason": "tool_use"
        });

        let action = AnthropicRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::AskUser(UserQuestion::Freeform {
                question,
                placeholder,
                validation_hint,
                ..
            }) => {
                assert_eq!(question, "Describe the authentication requirements");
                assert_eq!(
                    placeholder,
                    Some("e.g. OAuth2, API keys, JWT...".to_string())
                );
                assert!(validation_hint.is_some());
            }
            other => panic!("expected AskUser(Freeform), got {:?}", other),
        }
    }

    #[test]
    fn anthropic_parses_end_turn_as_done() {
        let response = json!({
            "id": "msg_end",
            "type": "message",
            "role": "assistant",
            "content": [],
            "stop_reason": "end_turn"
        });

        let action = AnthropicRuntime::parse_response(&response).unwrap();
        assert!(matches!(action, AgentAction::Done));
    }

    #[test]
    fn anthropic_parses_diff_summary_tool() {
        let response = json!({
            "id": "msg_diff",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_diff",
                    "name": "emit_diff_summary",
                    "input": {
                        "summary": "Added 3 idea cards and 1 task card"
                    }
                }
            ],
            "stop_reason": "tool_use"
        });

        let action = AnthropicRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::EmitDiffSummary(summary) => {
                assert_eq!(summary, "Added 3 idea cards and 1 task card");
            }
            other => panic!("expected EmitDiffSummary, got {:?}", other),
        }
    }

    #[test]
    fn anthropic_rejects_unknown_tool() {
        let response = json!({
            "id": "msg_unk",
            "type": "message",
            "role": "assistant",
            "content": [
                {
                    "type": "tool_use",
                    "id": "toolu_unk",
                    "name": "nonexistent_tool",
                    "input": {}
                }
            ],
            "stop_reason": "tool_use"
        });

        let result = AnthropicRuntime::parse_response(&response);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }

    #[test]
    fn coalesce_merges_consecutive_same_role() {
        let messages = vec![
            json!({"role": "user", "content": "First"}),
            json!({"role": "user", "content": "Second"}),
            json!({"role": "assistant", "content": "Reply"}),
            json!({"role": "user", "content": "Third"}),
        ];

        let result = coalesce_messages(messages);
        assert_eq!(result.len(), 3);
        assert!(
            result[0]
                .get("content")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("First")
        );
        assert!(
            result[0]
                .get("content")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("Second")
        );
    }

    #[tokio::test]
    #[cfg(feature = "live-test")]
    async fn anthropic_adapter_basic() {
        let runtime = AnthropicRuntime::from_env().expect("ANTHROPIC_API_KEY must be set");

        let spec_id = Ulid::new();
        let mut ctx = AgentContext::new(
            spec_id,
            "brainstormer-1".to_string(),
            AgentRole::Brainstormer,
        );
        ctx.state_summary = "A spec about building a CLI tool for managing notes".to_string();

        let result = runtime.run_step(&ctx).await;
        assert!(result.is_ok(), "live test failed: {:?}", result.err());
    }
}
