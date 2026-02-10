// ABOUTME: OpenAI API adapter implementing the AgentRuntime trait.
// ABOUTME: Translates AgentContext into OpenAI Chat Completions API calls with function calling.

use async_trait::async_trait;
use serde_json::{json, Value};
use ulid::Ulid;

use specd_core::command::Command;
use specd_core::transcript::UserQuestion;

use crate::context::AgentContext;
use crate::providers::role_prompt;
use crate::runtime::{AgentAction, AgentError, AgentRuntime};
use crate::tools::all_tool_definitions;

const DEFAULT_BASE_URL: &str = "https://api.openai.com";
const DEFAULT_MODEL: &str = "gpt-4o";
const MAX_TOKENS: u32 = 4096;

/// OpenAI runtime adapter. Calls the Chat Completions API with function
/// definitions and maps tool_calls responses back to AgentActions.
pub struct OpenAIRuntime {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl OpenAIRuntime {
    /// Create a new OpenAIRuntime reading configuration from environment variables.
    /// Required: `OPENAI_API_KEY`
    /// Optional: `OPENAI_BASE_URL` (defaults to https://api.openai.com)
    /// Optional: `OPENAI_MODEL` (defaults to gpt-4o)
    pub fn from_env() -> Result<Self, AgentError> {
        let api_key = std::env::var("OPENAI_API_KEY")
            .map_err(|_| AgentError::ProviderError("OPENAI_API_KEY not set".to_string()))?;

        let base_url =
            std::env::var("OPENAI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());

        let model = std::env::var("OPENAI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Ok(Self::new(api_key, base_url, model))
    }

    /// Create a new OpenAIRuntime with explicit configuration.
    pub fn new(api_key: String, base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url,
            model,
        }
    }

    /// Build the JSON request body for the OpenAI Chat Completions API.
    pub fn build_request_body(&self, context: &AgentContext) -> Value {
        let system_prompt = role_prompt(&context.agent_role, &context.state_summary);

        let tools = build_openai_tools();

        // Build conversation messages
        let mut messages = vec![json!({
            "role": "system",
            "content": system_prompt
        })];

        // Add rolling summary as context
        if !context.rolling_summary.is_empty() {
            messages.push(json!({
                "role": "user",
                "content": format!("[Context] Rolling summary: {}", context.rolling_summary)
            }));
        }

        // Add recent transcript
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

        // Add current state summary
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

        // Ensure at least one user message after system
        if messages.len() == 1 {
            messages.push(json!({
                "role": "user",
                "content": "The spec is empty. What should we do to get started?"
            }));
        }

        json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "messages": messages,
            "tools": tools,
            "tool_choice": "auto"
        })
    }

    /// Parse an OpenAI Chat Completions response into an AgentAction.
    pub fn parse_response(response_body: &Value) -> Result<AgentAction, AgentError> {
        let choices = response_body
            .get("choices")
            .and_then(|c| c.as_array())
            .ok_or_else(|| {
                AgentError::InvalidResponse("missing choices array in response".to_string())
            })?;

        let choice = choices
            .first()
            .ok_or_else(|| AgentError::InvalidResponse("empty choices array".to_string()))?;

        let message = choice.get("message").ok_or_else(|| {
            AgentError::InvalidResponse("missing message in choice".to_string())
        })?;

        // Check for tool_calls first
        if let Some(tool_calls) = message.get("tool_calls").and_then(|t| t.as_array())
            && let Some(tool_call) = tool_calls.first()
        {
            return parse_openai_tool_call(tool_call);
        }

        // Fall back to text content
        if let Some(content) = message.get("content").and_then(|c| c.as_str())
            && !content.is_empty()
        {
            return Ok(AgentAction::EmitNarration(content.to_string()));
        }

        // Check finish_reason
        let finish_reason = choice
            .get("finish_reason")
            .and_then(|f| f.as_str())
            .unwrap_or("");

        if finish_reason == "stop" {
            return Ok(AgentAction::Done);
        }

        Err(AgentError::InvalidResponse(
            "no actionable content in response".to_string(),
        ))
    }
}

/// Convert tool definitions to OpenAI's function calling format.
fn build_openai_tools() -> Vec<Value> {
    all_tool_definitions()
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.get("name").cloned().unwrap_or(Value::Null),
                    "description": tool.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": tool.get("parameters").cloned().unwrap_or(json!({"type": "object"}))
                }
            })
        })
        .collect()
}

/// Parse a single tool_call from the OpenAI response into an AgentAction.
fn parse_openai_tool_call(tool_call: &Value) -> Result<AgentAction, AgentError> {
    let function = tool_call.get("function").ok_or_else(|| {
        AgentError::InvalidResponse("tool_call missing function".to_string())
    })?;

    let tool_name = function
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| {
            AgentError::InvalidResponse("function missing name".to_string())
        })?;

    let arguments_str = function
        .get("arguments")
        .and_then(|a| a.as_str())
        .unwrap_or("{}");

    let input: Value = serde_json::from_str(arguments_str).map_err(|e| {
        AgentError::InvalidResponse(format!("failed to parse function arguments: {}", e))
    })?;

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

        "read_state" => Ok(AgentAction::Done),

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

#[async_trait]
impl AgentRuntime for OpenAIRuntime {
    async fn run_step(&self, context: &AgentContext) -> Result<AgentAction, AgentError> {
        let body = self.build_request_body(context);
        let url = format!("{}/v1/chat/completions", self.base_url);

        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
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
                "Unauthorized: check OPENAI_API_KEY".to_string(),
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
        "openai"
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
    fn openai_runtime_creation() {
        let runtime = OpenAIRuntime::new(
            "test-key".to_string(),
            "https://api.openai.com".to_string(),
            "gpt-4o".to_string(),
        );

        assert_eq!(runtime.provider_name(), "openai");
        assert_eq!(runtime.model_name(), "gpt-4o");
        assert_eq!(runtime.api_key, "test-key");
        assert_eq!(runtime.base_url, "https://api.openai.com");
    }

    #[test]
    fn openai_builds_request_body() {
        let runtime = OpenAIRuntime::new(
            "test-key".to_string(),
            "https://api.openai.com".to_string(),
            "gpt-4o".to_string(),
        );

        let spec_id = Ulid::new();
        let mut ctx =
            AgentContext::new(spec_id, "planner-1".to_string(), AgentRole::Planner);
        ctx.state_summary = "A spec about building a REST API".to_string();

        let body = runtime.build_request_body(&ctx);

        // Verify model
        assert_eq!(
            body.get("model").and_then(|m| m.as_str()),
            Some("gpt-4o")
        );

        // Verify messages array includes system message
        let messages = body.get("messages").and_then(|m| m.as_array()).unwrap();
        assert!(!messages.is_empty());
        assert_eq!(
            messages[0].get("role").and_then(|r| r.as_str()),
            Some("system")
        );

        // Verify tools array with function format
        let tools = body.get("tools").and_then(|t| t.as_array()).unwrap();
        assert_eq!(tools.len(), 7);
        for tool in tools {
            assert_eq!(
                tool.get("type").and_then(|t| t.as_str()),
                Some("function")
            );
            assert!(tool.get("function").is_some());
        }

        // Verify tool_choice
        assert_eq!(
            body.get("tool_choice").and_then(|t| t.as_str()),
            Some("auto")
        );
    }

    #[test]
    fn openai_parses_tool_call_response() {
        let response = json!({
            "id": "chatcmpl-123",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_abc",
                                "type": "function",
                                "function": {
                                    "name": "ask_user_boolean",
                                    "arguments": "{\"question\": \"Should we proceed?\", \"default\": false}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        });

        let action = OpenAIRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::AskUser(UserQuestion::Boolean {
                question, default, ..
            }) => {
                assert_eq!(question, "Should we proceed?");
                assert_eq!(default, Some(false));
            }
            other => panic!("expected AskUser(Boolean), got {:?}", other),
        }
    }

    #[test]
    fn openai_parses_text_response() {
        let response = json!({
            "id": "chatcmpl-456",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": "Let me think about the architecture..."
                    },
                    "finish_reason": "stop"
                }
            ]
        });

        let action = OpenAIRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::EmitNarration(text) => {
                assert!(text.contains("architecture"));
            }
            other => panic!("expected EmitNarration, got {:?}", other),
        }
    }

    #[test]
    fn openai_parses_write_commands_tool() {
        let response = json!({
            "id": "chatcmpl-789",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_xyz",
                                "type": "function",
                                "function": {
                                    "name": "write_commands",
                                    "arguments": "{\"commands\": [{\"type\": \"CreateCard\", \"card_type\": \"task\", \"title\": \"Setup CI\", \"body\": null, \"lane\": null, \"created_by\": \"planner-1\"}]}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        });

        let action = OpenAIRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::WriteCommands(cmds) => {
                assert_eq!(cmds.len(), 1);
            }
            other => panic!("expected WriteCommands, got {:?}", other),
        }
    }

    #[test]
    fn openai_parses_stop_as_done() {
        let response = json!({
            "id": "chatcmpl-done",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": ""
                    },
                    "finish_reason": "stop"
                }
            ]
        });

        let action = OpenAIRuntime::parse_response(&response).unwrap();
        assert!(matches!(action, AgentAction::Done));
    }

    #[test]
    fn openai_rejects_unknown_tool() {
        let response = json!({
            "id": "chatcmpl-unk",
            "object": "chat.completion",
            "choices": [
                {
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_unk",
                                "type": "function",
                                "function": {
                                    "name": "fake_tool",
                                    "arguments": "{}"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ]
        });

        let result = OpenAIRuntime::parse_response(&response);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown tool"));
    }

    #[tokio::test]
    #[cfg(feature = "live-test")]
    async fn openai_adapter_basic() {
        let runtime = OpenAIRuntime::from_env().expect("OPENAI_API_KEY must be set");

        let spec_id = Ulid::new();
        let mut ctx =
            AgentContext::new(spec_id, "brainstormer-1".to_string(), AgentRole::Brainstormer);
        ctx.state_summary = "A spec about building a task management CLI".to_string();

        let result = runtime.run_step(&ctx).await;
        assert!(result.is_ok(), "live test failed: {:?}", result.err());
    }
}
