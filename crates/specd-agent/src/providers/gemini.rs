// ABOUTME: Google Gemini API adapter implementing the AgentRuntime trait.
// ABOUTME: Translates AgentContext into Gemini generateContent API calls with function declarations.

use async_trait::async_trait;
use serde_json::{json, Value};
use ulid::Ulid;

use specd_core::command::Command;
use specd_core::transcript::UserQuestion;

use crate::context::AgentContext;
use crate::providers::role_prompt;
use crate::runtime::{AgentAction, AgentError, AgentRuntime};
use crate::tools::all_tool_definitions;

const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com";
const DEFAULT_MODEL: &str = "gemini-2.0-flash";
const MAX_TOKENS: u32 = 4096;

/// Google Gemini runtime adapter. Calls the generateContent API with function
/// declarations and maps functionCall responses back to AgentActions.
pub struct GeminiRuntime {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    model: String,
}

impl GeminiRuntime {
    /// Create a new GeminiRuntime reading configuration from environment variables.
    /// Required: `GEMINI_API_KEY`
    /// Optional: `GEMINI_BASE_URL` (defaults to https://generativelanguage.googleapis.com)
    /// Optional: `GEMINI_MODEL` (defaults to gemini-2.0-flash)
    pub fn from_env() -> Result<Self, AgentError> {
        let api_key = std::env::var("GEMINI_API_KEY")
            .map_err(|_| AgentError::ProviderError("GEMINI_API_KEY not set".to_string()))?;

        let base_url =
            std::env::var("GEMINI_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.to_string());

        let model =
            std::env::var("GEMINI_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());

        Ok(Self::new(api_key, base_url, model))
    }

    /// Create a new GeminiRuntime with explicit configuration.
    pub fn new(api_key: String, base_url: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            base_url,
            model,
        }
    }

    /// Build the JSON request body for the Gemini generateContent API.
    pub fn build_request_body(&self, context: &AgentContext) -> Value {
        let system_prompt = role_prompt(&context.agent_role, &context.state_summary);

        let tools = build_gemini_tools();

        // Build conversation contents
        let mut contents = Vec::new();

        // Add rolling summary as context
        if !context.rolling_summary.is_empty() {
            contents.push(json!({
                "role": "user",
                "parts": [{"text": format!("[Context] Rolling summary: {}", context.rolling_summary)}]
            }));
        }

        // Add recent transcript
        for msg in &context.recent_transcript {
            let role = if msg.sender == "human" {
                "user"
            } else {
                "model"
            };
            contents.push(json!({
                "role": role,
                "parts": [{"text": msg.content}]
            }));
        }

        // Add current state summary
        if !context.state_summary.is_empty() {
            contents.push(json!({
                "role": "user",
                "parts": [{"text": format!(
                    "[Current state] {}\n\n[Key decisions] {}\n\nWhat should we do next?",
                    context.state_summary,
                    context.key_decisions.join("; ")
                )}]
            }));
        }

        // Ensure at least one user message
        if contents.is_empty() {
            contents.push(json!({
                "role": "user",
                "parts": [{"text": "The spec is empty. What should we do to get started?"}]
            }));
        }

        // Coalesce consecutive same-role messages (Gemini requires alternating)
        let contents = coalesce_gemini_contents(contents);

        json!({
            "system_instruction": {
                "parts": [{"text": system_prompt}]
            },
            "contents": contents,
            "tools": [{"function_declarations": tools}],
            "generation_config": {
                "max_output_tokens": MAX_TOKENS
            }
        })
    }

    /// Parse a Gemini generateContent response into an AgentAction.
    pub fn parse_response(response_body: &Value) -> Result<AgentAction, AgentError> {
        let candidates = response_body
            .get("candidates")
            .and_then(|c| c.as_array())
            .ok_or_else(|| {
                AgentError::InvalidResponse("missing candidates array in response".to_string())
            })?;

        let candidate = candidates
            .first()
            .ok_or_else(|| AgentError::InvalidResponse("empty candidates array".to_string()))?;

        let content = candidate.get("content").ok_or_else(|| {
            AgentError::InvalidResponse("missing content in candidate".to_string())
        })?;

        let parts = content
            .get("parts")
            .and_then(|p| p.as_array())
            .ok_or_else(|| {
                AgentError::InvalidResponse("missing parts array in content".to_string())
            })?;

        // Look for functionCall parts first
        for part in parts {
            if let Some(function_call) = part.get("functionCall") {
                return parse_gemini_function_call(function_call);
            }
        }

        // Fall back to text parts
        for part in parts {
            if let Some(text) = part.get("text").and_then(|t| t.as_str())
                && !text.is_empty()
            {
                return Ok(AgentAction::EmitNarration(text.to_string()));
            }
        }

        // Check finish reason
        let finish_reason = candidate
            .get("finishReason")
            .and_then(|f| f.as_str())
            .unwrap_or("");

        if finish_reason == "STOP" {
            return Ok(AgentAction::Done);
        }

        Err(AgentError::InvalidResponse(
            "no actionable content in response".to_string(),
        ))
    }
}

/// Convert tool definitions to Gemini's function declaration format.
fn build_gemini_tools() -> Vec<Value> {
    all_tool_definitions()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.get("name").cloned().unwrap_or(Value::Null),
                "description": tool.get("description").cloned().unwrap_or(Value::Null),
                "parameters": tool.get("parameters").cloned().unwrap_or(json!({"type": "object"}))
            })
        })
        .collect()
}

/// Parse a Gemini functionCall object into an AgentAction.
fn parse_gemini_function_call(function_call: &Value) -> Result<AgentAction, AgentError> {
    let tool_name = function_call
        .get("name")
        .and_then(|n| n.as_str())
        .ok_or_else(|| {
            AgentError::InvalidResponse("functionCall missing name".to_string())
        })?;

    let args = function_call
        .get("args")
        .cloned()
        .unwrap_or(json!({}));

    match tool_name {
        "ask_user_boolean" => {
            let question = args
                .get("question")
                .and_then(|q| q.as_str())
                .unwrap_or("Yes or no?")
                .to_string();
            let default = args.get("default").and_then(|d| d.as_bool());

            Ok(AgentAction::AskUser(UserQuestion::Boolean {
                question_id: Ulid::new(),
                question,
                default,
            }))
        }

        "ask_user_multiple_choice" => {
            let question = args
                .get("question")
                .and_then(|q| q.as_str())
                .unwrap_or("Choose one:")
                .to_string();
            let choices = args
                .get("choices")
                .and_then(|c| c.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let allow_multi = args
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
            let question = args
                .get("question")
                .and_then(|q| q.as_str())
                .unwrap_or("Please elaborate:")
                .to_string();
            let placeholder = args
                .get("placeholder")
                .and_then(|p| p.as_str())
                .map(String::from);
            let validation_hint = args
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
            let commands_value = args.get("commands").cloned().unwrap_or(json!([]));
            let commands: Vec<Command> = serde_json::from_value(commands_value).map_err(|e| {
                AgentError::InvalidResponse(format!("failed to parse commands: {}", e))
            })?;

            Ok(AgentAction::WriteCommands(commands))
        }

        "emit_narration" => {
            let message = args
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();

            Ok(AgentAction::EmitNarration(message))
        }

        "emit_diff_summary" => {
            let summary = args
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

/// Coalesce consecutive Gemini contents with the same role.
fn coalesce_gemini_contents(contents: Vec<Value>) -> Vec<Value> {
    if contents.is_empty() {
        return contents;
    }

    let mut result: Vec<Value> = Vec::new();

    for content in contents {
        let role = content
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("user");
        let text = content
            .get("parts")
            .and_then(|p| p.as_array())
            .and_then(|arr| arr.first())
            .and_then(|part| part.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        if let Some(last) = result.last_mut() {
            let last_role = last
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("");

            if last_role == role {
                let prev_text = last
                    .get("parts")
                    .and_then(|p| p.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|part| part.get("text"))
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                let merged = format!("{}\n\n{}", prev_text, text);
                *last = json!({
                    "role": role,
                    "parts": [{"text": merged}]
                });
                continue;
            }
        }

        result.push(json!({
            "role": role,
            "parts": [{"text": text}]
        }));
    }

    result
}

#[async_trait]
impl AgentRuntime for GeminiRuntime {
    async fn run_step(&self, context: &AgentContext) -> Result<AgentAction, AgentError> {
        let body = self.build_request_body(context);
        let url = format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.base_url, self.model, self.api_key
        );

        let response = self
            .client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AgentError::ProviderError(format!("HTTP request failed: {}", e)))?;

        let status = response.status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(AgentError::RateLimited);
        }

        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Err(AgentError::ProviderError(
                "Unauthorized: check GEMINI_API_KEY".to_string(),
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
        "gemini"
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
    fn gemini_runtime_creation() {
        let runtime = GeminiRuntime::new(
            "test-key".to_string(),
            "https://generativelanguage.googleapis.com".to_string(),
            "gemini-2.0-flash".to_string(),
        );

        assert_eq!(runtime.provider_name(), "gemini");
        assert_eq!(runtime.model_name(), "gemini-2.0-flash");
        assert_eq!(runtime.api_key, "test-key");
        assert_eq!(
            runtime.base_url,
            "https://generativelanguage.googleapis.com"
        );
    }

    #[test]
    fn gemini_builds_request_body() {
        let runtime = GeminiRuntime::new(
            "test-key".to_string(),
            "https://generativelanguage.googleapis.com".to_string(),
            "gemini-2.0-flash".to_string(),
        );

        let spec_id = Ulid::new();
        let mut ctx =
            AgentContext::new(spec_id, "critic-1".to_string(), AgentRole::Critic);
        ctx.state_summary = "A spec about building a monitoring dashboard".to_string();

        let body = runtime.build_request_body(&ctx);

        // Verify system_instruction
        let sys = body.get("system_instruction").unwrap();
        let sys_text = sys
            .get("parts")
            .and_then(|p| p.as_array())
            .and_then(|arr| arr.first())
            .and_then(|part| part.get("text"))
            .and_then(|t| t.as_str())
            .unwrap();
        assert!(sys_text.contains("critic"));

        // Verify contents array exists
        let contents = body.get("contents").and_then(|c| c.as_array()).unwrap();
        assert!(!contents.is_empty());

        // Verify tools with function_declarations format
        let tools = body.get("tools").and_then(|t| t.as_array()).unwrap();
        assert_eq!(tools.len(), 1); // Single tools object wrapping declarations
        let declarations = tools[0]
            .get("function_declarations")
            .and_then(|f| f.as_array())
            .unwrap();
        assert_eq!(declarations.len(), 7);

        // Verify generation_config
        let gen_config = body.get("generation_config").unwrap();
        assert_eq!(
            gen_config
                .get("max_output_tokens")
                .and_then(|m| m.as_u64()),
            Some(4096)
        );
    }

    #[test]
    fn gemini_parses_function_call_response() {
        let response = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "ask_user_boolean",
                                    "args": {
                                        "question": "Should we add rate limiting?",
                                        "default": true
                                    }
                                }
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ]
        });

        let action = GeminiRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::AskUser(UserQuestion::Boolean {
                question, default, ..
            }) => {
                assert_eq!(question, "Should we add rate limiting?");
                assert_eq!(default, Some(true));
            }
            other => panic!("expected AskUser(Boolean), got {:?}", other),
        }
    }

    #[test]
    fn gemini_parses_text_response() {
        let response = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "text": "Reviewing the spec for consistency issues..."
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ]
        });

        let action = GeminiRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::EmitNarration(text) => {
                assert!(text.contains("consistency"));
            }
            other => panic!("expected EmitNarration, got {:?}", other),
        }
    }

    #[test]
    fn gemini_parses_write_commands_tool() {
        let response = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "write_commands",
                                    "args": {
                                        "commands": [
                                            {
                                                "type": "CreateCard",
                                                "card_type": "assumption",
                                                "title": "Users will have OAuth",
                                                "body": null,
                                                "lane": null,
                                                "created_by": "critic-1"
                                            }
                                        ]
                                    }
                                }
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ]
        });

        let action = GeminiRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::WriteCommands(cmds) => {
                assert_eq!(cmds.len(), 1);
            }
            other => panic!("expected WriteCommands, got {:?}", other),
        }
    }

    #[test]
    fn gemini_parses_emit_diff_summary() {
        let response = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "emit_diff_summary",
                                    "args": {
                                        "summary": "Created 2 assumption cards and 1 open question"
                                    }
                                }
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ]
        });

        let action = GeminiRuntime::parse_response(&response).unwrap();
        match action {
            AgentAction::EmitDiffSummary(summary) => {
                assert!(summary.contains("assumption"));
            }
            other => panic!("expected EmitDiffSummary, got {:?}", other),
        }
    }

    #[test]
    fn gemini_rejects_unknown_tool() {
        let response = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [
                            {
                                "functionCall": {
                                    "name": "nonexistent_function",
                                    "args": {}
                                }
                            }
                        ],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ]
        });

        let result = GeminiRuntime::parse_response(&response);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("unknown tool"));
    }

    #[test]
    fn gemini_parses_stop_with_empty_parts_as_done() {
        let response = json!({
            "candidates": [
                {
                    "content": {
                        "parts": [],
                        "role": "model"
                    },
                    "finishReason": "STOP"
                }
            ]
        });

        let action = GeminiRuntime::parse_response(&response).unwrap();
        assert!(matches!(action, AgentAction::Done));
    }

    #[test]
    fn coalesce_gemini_merges_consecutive_same_role() {
        let contents = vec![
            json!({"role": "user", "parts": [{"text": "First"}]}),
            json!({"role": "user", "parts": [{"text": "Second"}]}),
            json!({"role": "model", "parts": [{"text": "Reply"}]}),
            json!({"role": "user", "parts": [{"text": "Third"}]}),
        ];

        let result = coalesce_gemini_contents(contents);
        assert_eq!(result.len(), 3);

        let first_text = result[0]
            .get("parts")
            .and_then(|p| p.as_array())
            .and_then(|arr| arr.first())
            .and_then(|part| part.get("text"))
            .and_then(|t| t.as_str())
            .unwrap();
        assert!(first_text.contains("First"));
        assert!(first_text.contains("Second"));
    }

    #[tokio::test]
    #[cfg(feature = "live-test")]
    async fn gemini_adapter_basic() {
        let runtime = GeminiRuntime::from_env().expect("GEMINI_API_KEY must be set");

        let spec_id = Ulid::new();
        let mut ctx =
            AgentContext::new(spec_id, "brainstormer-1".to_string(), AgentRole::Brainstormer);
        ctx.state_summary = "A spec about building a web scraper".to_string();

        let result = runtime.run_step(&ctx).await;
        assert!(result.is_ok(), "live test failed: {:?}", result.err());
    }
}
