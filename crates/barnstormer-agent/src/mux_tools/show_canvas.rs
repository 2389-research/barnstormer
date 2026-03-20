// ABOUTME: Tool that lets the Manager push HTML content to the canvas panel during brainstorming.
// ABOUTME: Validates phase, sanitizes HTML, and sends UpdateCanvas command to actor.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use regex::Regex;
use serde_json::json;

use barnstormer_core::actor::SpecActorHandle;
use barnstormer_core::command::Command;
use barnstormer_core::state::SpecPhase;

static RE_SCRIPT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap());
static RE_ON_EVENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\s+on\w+\s*=\s*("[^"]*"|'[^']*'|[^\s>]*)"#).unwrap());
static RE_JS_URI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)(href|src|action)\s*=\s*["']?\s*javascript:"#).unwrap());
static RE_DANGEROUS_TAGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<(?:iframe|object|embed)[^>]*>.*?</(?:iframe|object|embed)>|<(?:iframe|object|embed)[^>]*/>").unwrap());

#[derive(Clone)]
pub struct ShowCanvasTool {
    pub(crate) actor: Arc<SpecActorHandle>,
}

/// Strip dangerous HTML: <script>/<iframe>/<object>/<embed> tags, on* event
/// attributes, and javascript: URIs in href/src/action attributes.
fn sanitize_html(input: &str) -> String {
    let without_scripts = RE_SCRIPT.replace_all(input, "");
    let without_dangerous = RE_DANGEROUS_TAGS.replace_all(&without_scripts, "");
    let without_on = RE_ON_EVENT.replace_all(&without_dangerous, "");
    RE_JS_URI.replace_all(&without_on, r#"$1=""#).to_string()
}

#[async_trait]
impl Tool for ShowCanvasTool {
    fn name(&self) -> &str {
        "show_canvas"
    }

    fn description(&self) -> &str {
        "Push HTML content to the canvas panel during brainstorming. Pass an empty string to clear the canvas."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "content": {
                    "type": "string",
                    "description": "HTML fragment to display on the canvas. Empty string clears it."
                }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        let state = self.actor.read_state().await;
        if state.phase != SpecPhase::Brainstorming {
            return Ok(ToolResult::text(
                "Canvas is only available during brainstorming.",
            ));
        }
        drop(state);

        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'content' parameter"))?
            .to_string();

        let sanitized = sanitize_html(&content);

        self.actor
            .send_command(Command::UpdateCanvas { content: sanitized })
            .await
            .map_err(|e| anyhow::anyhow!("failed to update canvas: {}", e))?;

        Ok(ToolResult::text("Canvas updated."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::actor;
    use barnstormer_core::state::SpecState;
    use ulid::Ulid;

    fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        (spec_id, handle)
    }

    #[tokio::test]
    async fn tool_name_is_show_canvas() {
        let (_id, handle) = make_test_actor();
        let tool = ShowCanvasTool {
            actor: Arc::new(handle),
        };
        assert_eq!(tool.name(), "show_canvas");
    }

    #[test]
    fn sanitize_strips_script_tags() {
        let input = r#"<h1>Hi</h1><script>alert('xss')</script><p>Safe</p>"#;
        let result = sanitize_html(input);
        assert!(!result.contains("script"));
        assert!(result.contains("<h1>Hi</h1>"));
        assert!(result.contains("<p>Safe</p>"));
    }

    #[test]
    fn sanitize_strips_on_event_attributes() {
        let input = r#"<div onclick="alert('xss')" onload="hack()">Content</div>"#;
        let result = sanitize_html(input);
        assert!(!result.contains("onclick"));
        assert!(!result.contains("onload"));
        assert!(result.contains("Content"));
    }

    #[test]
    fn sanitize_preserves_safe_html() {
        let input = r#"<div style="color:red;"><h1>Title</h1><p>Body</p></div>"#;
        let result = sanitize_html(input);
        assert_eq!(result, input);
    }

    #[test]
    fn sanitize_strips_javascript_uris() {
        let input = r#"<a href="javascript:alert(document.cookie)">Click</a>"#;
        let result = sanitize_html(input);
        assert!(!result.contains("javascript:"));
        assert!(result.contains("Click"));
    }

    #[test]
    fn sanitize_strips_iframe_tags() {
        let input = r#"<p>Safe</p><iframe src="evil.com"></iframe><p>Also safe</p>"#;
        let result = sanitize_html(input);
        assert!(!result.contains("iframe"));
        assert!(result.contains("<p>Safe</p>"));
        assert!(result.contains("<p>Also safe</p>"));
    }

    #[tokio::test]
    async fn show_canvas_sends_update_canvas_command() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();

        let tool = ShowCanvasTool {
            actor: handle.clone(),
        };
        let result = tool
            .execute(json!({"content": "<h1>Test</h1>"}))
            .await
            .unwrap();
        assert!(result.content.contains("Canvas updated"));

        let state = handle.read_state().await;
        assert_eq!(state.canvas_content, Some("<h1>Test</h1>".to_string()));
    }

    #[tokio::test]
    async fn show_canvas_clears_with_empty_string() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();
        handle
            .send_command(Command::UpdateCanvas {
                content: "old".to_string(),
            })
            .await
            .unwrap();

        let tool = ShowCanvasTool {
            actor: handle.clone(),
        };
        let result = tool.execute(json!({"content": ""})).await.unwrap();
        assert!(result.content.contains("Canvas updated"));

        let state = handle.read_state().await;
        assert_eq!(state.canvas_content, None);
    }

    #[tokio::test]
    async fn show_canvas_rejects_in_active_phase() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle
            .send_command(Command::CreateSpec {
                title: "Test".to_string(),
                one_liner: "t".to_string(),
                goal: "g".to_string(),
            })
            .await
            .unwrap();
        handle
            .send_command(Command::TransitionPhase {
                target: SpecPhase::Active,
            })
            .await
            .unwrap();

        let tool = ShowCanvasTool {
            actor: handle.clone(),
        };
        let result = tool
            .execute(json!({"content": "<h1>Test</h1>"}))
            .await
            .unwrap();
        assert!(result.content.contains("only available during brainstorming"));
    }
}
