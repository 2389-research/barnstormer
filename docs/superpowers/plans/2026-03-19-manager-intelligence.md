# Manager Intelligence & Canvas Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rework the Manager agent's brainstorming behavior with structured Q&A prompts, a `show_canvas` tool for HTML visuals, and a `propose_transition` tool for phase transitions.

**Architecture:** Two new tools (`show_canvas`, `propose_transition`) registered in the mux tool registry. `show_canvas` introduces a new Command/Event pair (`UpdateCanvas`/`CanvasUpdated`) with state persistence. `propose_transition` reuses existing `AskQuestion` infrastructure with swarm-level answer-watching via `Arc<Mutex<Option<Ulid>>>`. Manager system prompt switches based on phase.

**Tech Stack:** Rust, mux Tool trait, serde, tokio, Askama, HTMX SSE

**Spec:** `docs/superpowers/specs/2026-03-19-manager-intelligence-design.md`

**Prerequisites:** Plan 1 (Phase Model) and Plan 2 (Brainstorming UI) MUST be fully implemented first. Before starting, verify:
- `SpecPhase` enum exists in `crates/barnstormer-core/src/state.rs`
- `state.phase` field on `SpecState`
- `Command::TransitionPhase` in `command.rs`
- Brainstorming conditional layout in `spec_view.html`
- `#agent-canvas` div exists in brainstorming template

If any are missing, STOP and implement the prerequisite plan first.

---

## File Structure

| Action | File | Responsibility |
|--------|------|---------------|
| Modify | `crates/barnstormer-core/src/command.rs` | Add `UpdateCanvas` command variant |
| Modify | `crates/barnstormer-core/src/event.rs` | Add `CanvasUpdated` event variant |
| Modify | `crates/barnstormer-core/src/state.rs` | Add `canvas_content` field, reducer logic, undo clearing |
| Modify | `crates/barnstormer-core/src/actor.rs` | Handle `UpdateCanvas` command |
| Create | `crates/barnstormer-agent/src/mux_tools/show_canvas.rs` | `show_canvas` tool implementation |
| Create | `crates/barnstormer-agent/src/mux_tools/propose_transition.rs` | `propose_transition` tool implementation |
| Modify | `crates/barnstormer-agent/src/mux_tools/mod.rs` | Register new tools, pass `pending_transition_question` arc |
| Modify | `crates/barnstormer-agent/src/swarm.rs` | Brainstorming system prompt, `pending_transition_question` field, answer-watching |
| Modify | `crates/barnstormer-agent/src/context.rs` | Add `CanvasUpdated` arm to `describe_event_payload` |
| Modify | `crates/barnstormer-server/src/api/stream.rs` | Add `CanvasUpdated` arm to `event_type_name` |
| Modify | `templates/partials/spec_view.html` | Canvas SSE listener JS, pre-populate canvas on load |
| Modify | `crates/barnstormer-server/src/web/mod.rs` | Pass `canvas_content` to brainstorming template |

---

### Task 1: Add UpdateCanvas command and CanvasUpdated event to core

**Files:**
- Modify: `crates/barnstormer-core/src/command.rs`
- Modify: `crates/barnstormer-core/src/event.rs`
- Modify: `crates/barnstormer-core/src/state.rs`
- Modify: `crates/barnstormer-core/src/actor.rs`

- [ ] **Step 1: Write failing tests for canvas state**

Add to `state.rs` test module:

```rust
#[test]
fn canvas_updated_sets_content() {
    let mut state = SpecState::new();
    let event = Event {
        event_id: 1,
        spec_id: Ulid::new(),
        timestamp: Utc::now(),
        payload: EventPayload::CanvasUpdated {
            content: "<h1>Hello</h1>".to_string(),
        },
    };
    state.apply(&event);
    assert_eq!(state.canvas_content, Some("<h1>Hello</h1>".to_string()));
}

#[test]
fn canvas_updated_empty_clears_content() {
    let mut state = SpecState::new();
    state.canvas_content = Some("old".to_string());
    let event = Event {
        event_id: 1,
        spec_id: Ulid::new(),
        timestamp: Utc::now(),
        payload: EventPayload::CanvasUpdated {
            content: String::new(),
        },
    };
    state.apply(&event);
    assert_eq!(state.canvas_content, None);
}

#[test]
fn canvas_updated_does_not_push_undo() {
    let mut state = SpecState::new();
    let event = Event {
        event_id: 1,
        spec_id: Ulid::new(),
        timestamp: Utc::now(),
        payload: EventPayload::CanvasUpdated {
            content: "html".to_string(),
        },
    };
    state.apply(&event);
    assert!(state.undo_stack.is_empty());
}

#[test]
fn undo_applied_clears_canvas_content() {
    let mut state = SpecState::new();
    state.canvas_content = Some("stale diagram".to_string());
    let event = Event {
        event_id: 2,
        spec_id: Ulid::new(),
        timestamp: Utc::now(),
        payload: EventPayload::UndoApplied {
            target_event_id: 1,
            inverse_events: vec![],
        },
    };
    state.apply(&event);
    assert_eq!(state.canvas_content, None);
}

#[test]
fn canvas_content_serde_round_trip() {
    let mut state = SpecState::new();
    state.canvas_content = Some("<div>test</div>".to_string());
    let json = serde_json::to_string(&state).unwrap();
    let back: SpecState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.canvas_content, Some("<div>test</div>".to_string()));
}

#[test]
fn snapshot_without_canvas_content_deserializes_as_none() {
    // JSON matches current SpecState shape (with phase from Plan 1)
    let json = serde_json::to_string(&SpecState::new()).unwrap();
    // Verify default has no canvas_content
    let back: SpecState = serde_json::from_str(&json).unwrap();
    assert_eq!(back.canvas_content, None);
}
```

- [ ] **Step 2: Write failing tests for command and event serde**

Add to `command.rs` test module (also add to the existing comprehensive round-trip test vec):

```rust
#[test]
fn update_canvas_round_trip() {
    let cmd = Command::UpdateCanvas {
        content: "<h1>Test</h1>".to_string(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"UpdateCanvas\""));
    let back: Command = serde_json::from_str(&json).unwrap();
    match back {
        Command::UpdateCanvas { content } => assert_eq!(content, "<h1>Test</h1>"),
        _ => panic!("wrong variant"),
    }
}
```

Add to `event.rs` test module (uses existing `round_trip_event()` helper):

```rust
#[test]
fn canvas_updated_round_trip() {
    round_trip_event(EventPayload::CanvasUpdated {
        content: "<h1>Test</h1>".to_string(),
    });
}
```

- [ ] **Step 3: Write failing test for actor handling**

Add to `actor.rs` test module, following inline pattern:

```rust
#[tokio::test]
async fn update_canvas_produces_event() {
    let spec_id = Ulid::new();
    let handle = spawn(spec_id, SpecState::new());
    handle.send_command(Command::CreateSpec {
        title: "Test".to_string(),
        one_liner: "t".to_string(),
        goal: "g".to_string(),
    }).await.unwrap();

    let events = handle
        .send_command(Command::UpdateCanvas {
            content: "<h1>Hello</h1>".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(events.len(), 1);
    match &events[0].payload {
        EventPayload::CanvasUpdated { content } => {
            assert_eq!(content, "<h1>Hello</h1>");
        }
        _ => panic!("wrong event"),
    }
}
```

- [ ] **Step 4: Run tests to verify they fail**

Run: `cargo test --package barnstormer-core -- canvas`
Expected: compilation errors

- [ ] **Step 5: Add UpdateCanvas to Command enum**

In `command.rs`:

```rust
UpdateCanvas { content: String },
```

- [ ] **Step 6: Add CanvasUpdated to EventPayload enum**

In `event.rs`:

```rust
CanvasUpdated { content: String },
```

- [ ] **Step 7: Add canvas_content field to SpecState**

In `state.rs`, add to `SpecState`:

```rust
#[serde(default)]
pub canvas_content: Option<String>,
```

Update both `Default` impl and `new()` to include `canvas_content: None`.

- [ ] **Step 8: Add reducer arm for CanvasUpdated**

In `state.rs`, in the `apply()` match:

```rust
EventPayload::CanvasUpdated { content } => {
    if content.is_empty() {
        self.canvas_content = None;
    } else {
        self.canvas_content = Some(content.clone());
    }
    // No undo entry — canvas updates are transient
}
```

Also in `apply_without_undo()`:

```rust
EventPayload::CanvasUpdated { content } => {
    if content.is_empty() {
        self.canvas_content = None;
    } else {
        self.canvas_content = Some(content.clone());
    }
}
```

In the `UndoApplied` handler within `apply()`, add canvas clearing:

```rust
self.canvas_content = None; // Clear stale canvas on undo
```

- [ ] **Step 9: Add command handling in actor**

In `actor.rs`, in `command_to_events()`:

```rust
Command::UpdateCanvas { content } => {
    vec![EventPayload::CanvasUpdated { content }]
}
```

- [ ] **Step 10: Run tests to verify they pass**

Run: `cargo test --package barnstormer-core`
Expected: all tests PASS

- [ ] **Step 11: Commit**

```bash
git add crates/barnstormer-core/src/command.rs crates/barnstormer-core/src/event.rs crates/barnstormer-core/src/state.rs crates/barnstormer-core/src/actor.rs
git commit -m "feat: add UpdateCanvas command and CanvasUpdated event with state persistence"
```

---

### Task 2: Update SSE stream and agent context for CanvasUpdated

**Files:**
- Modify: `crates/barnstormer-server/src/api/stream.rs`
- Modify: `crates/barnstormer-agent/src/context.rs`

- [ ] **Step 1: Add CanvasUpdated arm to event_type_name**

In `stream.rs`, add to the `event_type_name` match:

```rust
EventPayload::CanvasUpdated { .. } => "canvas_updated",
```

- [ ] **Step 2: Add CanvasUpdated arm to describe_event_payload**

In `context.rs`, add to the `describe_event_payload` match:

```rust
EventPayload::CanvasUpdated { content } => {
    if content.is_empty() {
        "canvas cleared".to_string()
    } else {
        "canvas updated with new content".to_string()
    }
}
```

- [ ] **Step 3: Run full workspace tests**

Run: `cargo test --all`
Expected: all tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/barnstormer-server/src/api/stream.rs crates/barnstormer-agent/src/context.rs
git commit -m "feat: add CanvasUpdated to SSE stream and agent context descriptions"
```

---

### Task 3: Implement show_canvas tool

**Files:**
- Create: `crates/barnstormer-agent/src/mux_tools/show_canvas.rs`
- Modify: `crates/barnstormer-agent/src/mux_tools/mod.rs`
- Possibly modify: `crates/barnstormer-agent/Cargo.toml` (add `regex`)

- [ ] **Step 1: Add regex dependency**

Check if `regex` is in `crates/barnstormer-agent/Cargo.toml`. If not, add it. Check workspace `Cargo.toml` first — if `regex` is in `[workspace.dependencies]`, use `regex.workspace = true`. Otherwise add `regex = "1"` directly.

- [ ] **Step 2: Create show_canvas.rs with implementation and tests**

Create `crates/barnstormer-agent/src/mux_tools/show_canvas.rs`. Follow exact import pattern from `ask_user.rs`:

```rust
// ABOUTME: Tool that lets the Manager push HTML content to the canvas panel during brainstorming.
// ABOUTME: Validates phase, sanitizes HTML, and sends UpdateCanvas command to actor.

use std::sync::Arc;

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;

use barnstormer_core::actor::SpecActorHandle;
use barnstormer_core::command::Command;
use barnstormer_core::state::SpecPhase;

#[derive(Clone)]
pub struct ShowCanvasTool {
    pub(crate) actor: Arc<SpecActorHandle>,
}

/// Strip <script> tags and on* event attributes from HTML content.
fn sanitize_html(input: &str) -> String {
    let re_script = regex::Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap();
    let without_scripts = re_script.replace_all(input, "");
    let re_on = regex::Regex::new(r#"(?i)\s+on\w+\s*=\s*("[^"]*"|'[^']*'|[^\s>]*)"#).unwrap();
    re_on.replace_all(&without_scripts, "").to_string()
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

    #[test]
    fn tool_name_is_show_canvas() {
        let (_id, handle) = make_test_actor();
        let tool = ShowCanvasTool { actor: Arc::new(handle) };
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

    #[tokio::test]
    async fn show_canvas_sends_update_canvas_command() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        // Put spec in Brainstorming via CreateSpec
        handle.send_command(Command::CreateSpec {
            title: "Test".to_string(),
            one_liner: "t".to_string(),
            goal: "g".to_string(),
        }).await.unwrap();

        let tool = ShowCanvasTool { actor: handle.clone() };
        let result = tool.execute(json!({"content": "<h1>Test</h1>"})).await.unwrap();
        assert!(result.content.contains("Canvas updated"));

        let state = handle.read_state().await;
        assert_eq!(state.canvas_content, Some("<h1>Test</h1>".to_string()));
    }

    #[tokio::test]
    async fn show_canvas_clears_with_empty_string() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle.send_command(Command::CreateSpec {
            title: "Test".to_string(),
            one_liner: "t".to_string(),
            goal: "g".to_string(),
        }).await.unwrap();
        // Set some content first
        handle.send_command(Command::UpdateCanvas {
            content: "old".to_string(),
        }).await.unwrap();

        let tool = ShowCanvasTool { actor: handle.clone() };
        let result = tool.execute(json!({"content": ""})).await.unwrap();
        assert!(result.content.contains("Canvas updated"));

        let state = handle.read_state().await;
        assert_eq!(state.canvas_content, None);
    }

    #[tokio::test]
    async fn show_canvas_rejects_in_active_phase() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle.send_command(Command::CreateSpec {
            title: "Test".to_string(),
            one_liner: "t".to_string(),
            goal: "g".to_string(),
        }).await.unwrap();
        // Transition to Active
        handle.send_command(Command::TransitionPhase {
            target: SpecPhase::Active,
        }).await.unwrap();

        let tool = ShowCanvasTool { actor: handle.clone() };
        let result = tool.execute(json!({"content": "<h1>Test</h1>"})).await.unwrap();
        assert!(result.content.contains("only available during brainstorming"));
    }
}
```

- [ ] **Step 3: Register in mod.rs**

In `mux_tools/mod.rs`, add:

```rust
pub mod show_canvas;
```

And in `build_registry()`, add:

```rust
registry.register(show_canvas::ShowCanvasTool {
    actor: actor.clone(),
}).await;
```

- [ ] **Step 4: Run tests**

Run: `cargo test --package barnstormer-agent -- show_canvas`
Expected: all tests PASS

- [ ] **Step 5: Commit**

```bash
git add crates/barnstormer-agent/src/mux_tools/show_canvas.rs crates/barnstormer-agent/src/mux_tools/mod.rs crates/barnstormer-agent/Cargo.toml
git commit -m "feat: add show_canvas tool with HTML sanitization and phase gating"
```

---

### Task 4: Implement propose_transition tool

**Files:**
- Create: `crates/barnstormer-agent/src/mux_tools/propose_transition.rs`
- Modify: `crates/barnstormer-agent/src/mux_tools/mod.rs`

- [ ] **Step 1: Create propose_transition.rs with implementation and tests**

Follow exact import pattern from `ask_user.rs` (line 8: `use mux::tool::{Tool, ToolResult};`, line 14: `use barnstormer_core::transcript::UserQuestion;`):

```rust
// ABOUTME: Tool that lets the Manager propose transitioning from brainstorming to active mode.
// ABOUTME: Reuses existing AskQuestion infrastructure with swarm-level answer-watching.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mux::tool::{Tool, ToolResult};
use serde_json::json;
use ulid::Ulid;

use barnstormer_core::actor::SpecActorHandle;
use barnstormer_core::command::Command;
use barnstormer_core::transcript::UserQuestion;

#[derive(Clone)]
pub struct ProposeTransitionTool {
    pub(crate) actor: Arc<SpecActorHandle>,
    pub(crate) question_pending: Arc<AtomicBool>,
    pub(crate) pending_transition_question: Arc<Mutex<Option<Ulid>>>,
}

#[async_trait]
impl Tool for ProposeTransitionTool {
    fn name(&self) -> &str {
        "propose_transition"
    }

    fn description(&self) -> &str {
        "Propose transitioning from brainstorming to active mode. Summarize what you've learned and ask the user if they're ready to build the spec."
    }

    fn schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {
                    "type": "string",
                    "description": "Brief recap of what you've learned from brainstorming."
                }
            },
            "required": ["summary"]
        })
    }

    async fn execute(&self, params: serde_json::Value) -> Result<ToolResult, anyhow::Error> {
        if self.question_pending
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(ToolResult::text(
                "A question is already pending. Wait for the user to answer before proposing a transition.",
            ));
        }

        let summary = params
            .get("summary")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("missing 'summary' parameter"))?
            .to_string();

        let question_id = Ulid::new();
        let question = UserQuestion::Boolean {
            question_id,
            question: format!("{}\n\nReady to move on and build the spec?", summary),
            default: Some(true),
        };

        if let Err(e) = self.actor.send_command(Command::AskQuestion { question }).await {
            self.question_pending.store(false, Ordering::SeqCst);
            return Err(anyhow::anyhow!("failed to ask transition question: {}", e));
        }

        {
            let mut guard = self.pending_transition_question.lock().unwrap();
            *guard = Some(question_id);
        }

        Ok(ToolResult::text(
            "Transition proposal sent to the user. They will see a confirmation prompt. Wait for their response before continuing.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::actor;
    use barnstormer_core::state::SpecState;

    fn make_test_actor() -> (Ulid, SpecActorHandle) {
        let spec_id = Ulid::new();
        let handle = actor::spawn(spec_id, SpecState::new());
        (spec_id, handle)
    }

    #[test]
    fn tool_name_is_propose_transition() {
        let (_id, handle) = make_test_actor();
        let tool = ProposeTransitionTool {
            actor: Arc::new(handle),
            question_pending: Arc::new(AtomicBool::new(false)),
            pending_transition_question: Arc::new(Mutex::new(None)),
        };
        assert_eq!(tool.name(), "propose_transition");
    }

    #[tokio::test]
    async fn propose_transition_sends_boolean_question() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle.send_command(Command::CreateSpec {
            title: "Test".to_string(),
            one_liner: "t".to_string(),
            goal: "g".to_string(),
        }).await.unwrap();

        let question_pending = Arc::new(AtomicBool::new(false));
        let pending_transition = Arc::new(Mutex::new(None));

        let tool = ProposeTransitionTool {
            actor: handle.clone(),
            question_pending: question_pending.clone(),
            pending_transition_question: pending_transition.clone(),
        };

        let result = tool.execute(json!({"summary": "We decided on WebSocket architecture."})).await.unwrap();
        assert!(result.content.contains("Transition proposal sent"));

        // Verify question_pending is set
        assert!(question_pending.load(Ordering::SeqCst));

        // Verify pending_transition_question is set
        let stored = pending_transition.lock().unwrap();
        assert!(stored.is_some());

        // Verify state has a pending question
        let state = handle.read_state().await;
        assert!(state.pending_question.is_some());
    }

    #[tokio::test]
    async fn propose_transition_rejects_when_question_pending() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        let question_pending = Arc::new(AtomicBool::new(true)); // Already pending

        let tool = ProposeTransitionTool {
            actor: handle,
            question_pending,
            pending_transition_question: Arc::new(Mutex::new(None)),
        };

        let result = tool.execute(json!({"summary": "test"})).await.unwrap();
        assert!(result.content.contains("already pending"));
    }

    #[tokio::test]
    async fn propose_transition_stores_question_id() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle.send_command(Command::CreateSpec {
            title: "Test".to_string(),
            one_liner: "t".to_string(),
            goal: "g".to_string(),
        }).await.unwrap();

        let pending_transition = Arc::new(Mutex::new(None));
        let tool = ProposeTransitionTool {
            actor: handle,
            question_pending: Arc::new(AtomicBool::new(false)),
            pending_transition_question: pending_transition.clone(),
        };

        tool.execute(json!({"summary": "test"})).await.unwrap();
        let stored = pending_transition.lock().unwrap();
        assert!(stored.is_some(), "should store question ID");
    }

    #[tokio::test]
    async fn propose_transition_allows_reproposal_after_clear() {
        let (_id, handle) = make_test_actor();
        let handle = Arc::new(handle);
        handle.send_command(Command::CreateSpec {
            title: "Test".to_string(),
            one_liner: "t".to_string(),
            goal: "g".to_string(),
        }).await.unwrap();

        let question_pending = Arc::new(AtomicBool::new(false));
        let pending_transition = Arc::new(Mutex::new(None));

        let tool = ProposeTransitionTool {
            actor: handle.clone(),
            question_pending: question_pending.clone(),
            pending_transition_question: pending_transition.clone(),
        };

        // First proposal
        tool.execute(json!({"summary": "first"})).await.unwrap();
        let q1 = *pending_transition.lock().unwrap();
        assert!(q1.is_some());

        // Simulate "no" answer clearing the state
        *pending_transition.lock().unwrap() = None;
        question_pending.store(false, Ordering::SeqCst);
        // Answer the pending question so another can be asked
        handle.send_command(Command::AnswerQuestion {
            question_id: q1.unwrap(),
            answer: "no".to_string(),
        }).await.unwrap();

        // Second proposal should work
        let result = tool.execute(json!({"summary": "second"})).await.unwrap();
        assert!(result.content.contains("Transition proposal sent"));
        let q2 = *pending_transition.lock().unwrap();
        assert!(q2.is_some());
        assert_ne!(q1, q2);
    }
}
```

- [ ] **Step 2: Update build_registry to accept and pass new arc**

In `mux_tools/mod.rs`, update `build_registry` signature:

```rust
pub async fn build_registry(
    actor: Arc<SpecActorHandle>,
    question_pending: Arc<AtomicBool>,
    pending_transition_question: Arc<Mutex<Option<Ulid>>>,
    agent_id: String,
) -> Registry {
```

Add to imports: `use std::sync::Mutex;` and `use ulid::Ulid;`

Add module and registration:

```rust
pub mod propose_transition;

// In build_registry body:
registry.register(propose_transition::ProposeTransitionTool {
    actor: actor.clone(),
    question_pending: question_pending.clone(),
    pending_transition_question: pending_transition_question.clone(),
}).await;
```

Update the existing tests in `mod.rs` to pass the new parameter:

```rust
let registry = build_registry(
    Arc::new(handle),
    Arc::new(AtomicBool::new(false)),
    Arc::new(Mutex::new(None)),
    "test-agent".to_string(),
).await;
```

Update tool count assertion from 7 to 9, and add the two new names to the `names.contains` assertions:

```rust
assert_eq!(registry.count().await, 9);
// ... existing assertions ...
assert!(names.contains(&"show_canvas".to_string()));
assert!(names.contains(&"propose_transition".to_string()));
```

Update all call sites of `build_registry` in `swarm.rs` to pass the new parameter.

- [ ] **Step 3: Run tests**

Run: `cargo test --package barnstormer-agent -- propose_transition`
Expected: all tests PASS

Run: `cargo test --package barnstormer-agent -- build_registry`
Expected: updated registry tests PASS

- [ ] **Step 4: Commit**

```bash
git add crates/barnstormer-agent/src/mux_tools/propose_transition.rs crates/barnstormer-agent/src/mux_tools/mod.rs
git commit -m "feat: add propose_transition tool with question-pending guard"
```

---

### Task 5: Swarm answer-watching and pending_transition_question field

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs`

- [ ] **Step 1: Add pending_transition_question field to SwarmOrchestrator**

Add to `SwarmOrchestrator` struct:

```rust
pub pending_transition_question: Arc<Mutex<Option<Ulid>>>,
```

Add `use std::sync::Mutex;` to imports.

Initialize in both `with_defaults()` and `with_agents()`:

```rust
pending_transition_question: Arc::new(Mutex::new(None)),
```

- [ ] **Step 2: Pass pending_transition_question to build_registry**

In `run_agent_step` (or wherever `build_registry` is called), pass `self.pending_transition_question.clone()`:

```rust
let registry = build_registry(
    self.actor.clone(),
    self.question_pending.clone(),
    self.pending_transition_question.clone(),
    agent.agent_id.clone(),
).await;
```

- [ ] **Step 3: Add answer-watching logic in run_loop**

In `run_loop`, after events are processed (or in a dedicated event-drain section), add logic to check for transition question answers. Subscribe to the broadcast channel at the start of `run_loop`:

```rust
let mut transition_rx = {
    let s = swarm.lock().await;
    s.actor.subscribe()
};
```

In the event processing or after the `tokio::select!`, drain and check:

```rust
while let Ok(event) = transition_rx.try_recv() {
    if let EventPayload::QuestionAnswered { question_id, answer } = &event.payload {
        let pending = {
            let s = swarm.lock().await;
            let guard = s.pending_transition_question.lock().unwrap();
            *guard
        };
        if let Some(pending_id) = pending {
            if *question_id == pending_id {
                {
                    let s = swarm.lock().await;
                    *s.pending_transition_question.lock().unwrap() = None;
                }
                if answer.to_lowercase().starts_with('y') || answer == "true" {
                    let s = swarm.lock().await;
                    let _ = s.actor.send_command(Command::TransitionPhase {
                        target: SpecPhase::Active,
                    }).await;
                }
            }
        }
    }
}
```

- [ ] **Step 4: Write tests for answer-watching**

Extract the transition-check logic into a testable helper function:

```rust
/// Check if a QuestionAnswered event matches a pending transition question.
/// Returns true if transition should proceed (yes answer).
fn should_transition_on_answer(
    pending: &Mutex<Option<Ulid>>,
    question_id: Ulid,
    answer: &str,
) -> bool {
    let stored = { *pending.lock().unwrap() };
    if let Some(pending_id) = stored {
        if question_id == pending_id {
            *pending.lock().unwrap() = None;
            return answer.to_lowercase().starts_with('y') || answer == "true";
        }
    }
    false
}
```

Tests:

```rust
#[test]
fn should_transition_on_yes_answer() {
    let pending = Mutex::new(Some(Ulid::from_string("01HTEST0000000000000000000").unwrap()));
    let qid = Ulid::from_string("01HTEST0000000000000000000").unwrap();
    assert!(should_transition_on_answer(&pending, qid, "yes"));
    assert!(pending.lock().unwrap().is_none(), "should clear pending");
}

#[test]
fn should_not_transition_on_no_answer() {
    let pending = Mutex::new(Some(Ulid::from_string("01HTEST0000000000000000000").unwrap()));
    let qid = Ulid::from_string("01HTEST0000000000000000000").unwrap();
    assert!(!should_transition_on_answer(&pending, qid, "no"));
    assert!(pending.lock().unwrap().is_none(), "should still clear pending");
}

#[test]
fn should_not_transition_on_wrong_question_id() {
    let pending = Mutex::new(Some(Ulid::from_string("01HTEST0000000000000000000").unwrap()));
    let wrong_qid = Ulid::new();
    assert!(!should_transition_on_answer(&pending, wrong_qid, "yes"));
    assert!(pending.lock().unwrap().is_some(), "should NOT clear pending");
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test --package barnstormer-agent`
Expected: all tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/barnstormer-agent/src/swarm.rs
git commit -m "feat: swarm answer-watching for transition questions"
```

---

### Task 6: Manager brainstorming system prompt

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs`

- [ ] **Step 1: Write failing tests for prompt switching**

```rust
#[test]
fn manager_gets_brainstorming_prompt_in_brainstorming() {
    let prompt = full_system_prompt(&AgentRole::Manager, "agent-123", &SpecPhase::Brainstorming);
    assert!(prompt.contains("one question at a time"));
}

#[test]
fn manager_gets_standard_prompt_in_active() {
    let prompt = full_system_prompt(&AgentRole::Manager, "agent-123", &SpecPhase::Active);
    assert!(!prompt.contains("one question at a time"));
}

#[test]
fn non_manager_gets_same_prompt_regardless_of_phase() {
    let active = full_system_prompt(&AgentRole::Brainstormer, "agent-123", &SpecPhase::Active);
    let brainstorming = full_system_prompt(&AgentRole::Brainstormer, "agent-123", &SpecPhase::Brainstorming);
    assert_eq!(active, brainstorming);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --package barnstormer-agent -- brainstorming_prompt`
Expected: FAIL — `full_system_prompt` doesn't accept phase parameter

- [ ] **Step 3: Add brainstorming system prompt constant**

```rust
const MANAGER_BRAINSTORMING_PROMPT: &str = r#"You are the Manager agent in brainstorming mode. Your job is to understand the user's idea through structured Q&A before building a spec.

## Rules
1. Ask ONE question at a time — never multiple questions in one message
2. Prefer multiple choice questions — easier for the user, faster iteration
3. Use Boolean (yes/no) questions for binary decisions
4. Use Freeform questions only when the answer can't be anticipated
5. Understand the idea before creating cards — don't rush to populate the board
6. Capture decisions as cards only when something is clearly decided
7. Read existing cards for context — especially after "Resume brainstorming"
8. Use show_canvas when a visual would help the user decide
9. Call propose_transition when you have enough context to build a full spec

## Flow
- Start by understanding the core idea
- Explore key decisions: architecture, scope, constraints, users
- Capture firm decisions as cards along the way
- When you have enough context, propose transitioning to active mode
"#;
```

- [ ] **Step 4: Update full_system_prompt signature**

Change `fn full_system_prompt(role: &AgentRole, agent_id: &str)` (currently at line 117) to:

```rust
fn full_system_prompt(role: &AgentRole, agent_id: &str, phase: &SpecPhase) -> String {
    let base = if *role == AgentRole::Manager && *phase == SpecPhase::Brainstorming {
        MANAGER_BRAINSTORMING_PROMPT
    } else {
        system_prompt_for_role(role)
    };
    format!("{}\n\n{}", base, tool_usage_guide(agent_id))
}
```

Update all call sites in `swarm.rs` to pass the phase. Read phase from the actor state before building the prompt in `run_agent_step`.

- [ ] **Step 5: Run tests**

Run: `cargo test --package barnstormer-agent`
Expected: all tests PASS

- [ ] **Step 6: Commit**

```bash
git add crates/barnstormer-agent/src/swarm.rs
git commit -m "feat: Manager gets brainstorming system prompt during brainstorming phase"
```

---

### Task 7: Canvas SSE listener and template integration

**Files:**
- Modify: `templates/partials/spec_view.html`
- Modify: `crates/barnstormer-server/src/web/mod.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[tokio::test]
async fn spec_view_brainstorming_contains_agent_canvas() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Canvas+test+spec"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::get(&format!("/web/specs/{}", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("agent-canvas"), "brainstorming view should have agent-canvas div");
    assert!(html.contains("sse:canvas_updated"), "should have canvas SSE listener");
}

#[tokio::test]
async fn spec_view_prepopulates_canvas_content() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Canvas+prepopulate+test"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    // Set canvas content
    {
        let actors = state.actors.read().await;
        let handle = actors.get(&spec_id).unwrap();
        handle.send_command(Command::UpdateCanvas {
            content: "<h1>Test Canvas</h1>".to_string(),
        }).await.unwrap();
    }

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::get(&format!("/web/specs/{}", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let html = String::from_utf8(body.to_vec()).unwrap();
    assert!(html.contains("Test Canvas"), "canvas content should be pre-populated");
}
```

- [ ] **Step 2: Update SpecViewTemplate to include canvas_content**

Add `canvas_content: Option<String>` to the `SpecViewTemplate` struct. Read from state in handler:

```rust
let canvas_content = spec_state.canvas_content.clone();
```

- [ ] **Step 3: Add canvas SSE listener and pre-population to spec_view.html**

In the brainstorming section, update the `#agent-canvas` div to pre-populate:

```html
{% match canvas_content %}
{% when Some with (content) %}
<div id="agent-canvas" style="display:block;">{{ content|safe }}</div>
{% when None %}
<div id="agent-canvas" style="display:none;"></div>
{% endmatch %}
```

Add SSE listener in the brainstorming script block. The SSE data is the full Event JSON serialized by `stream.rs`, so content is at `data.payload.content`:

```javascript
compositor.addEventListener('sse:canvas_updated', function(evt) {
    var data = JSON.parse(evt.detail.data);
    var canvas = document.getElementById('agent-canvas');
    var content = data.payload && data.payload.content;
    if (content && content.trim() !== '') {
        canvas.innerHTML = content;
        canvas.style.display = 'block';
    } else {
        canvas.innerHTML = '';
        canvas.style.display = 'none';
    }
});
```

- [ ] **Step 4: Run tests**

Run: `cargo test --package barnstormer-server -- canvas`
Expected: all tests PASS

- [ ] **Step 5: Commit**

```bash
git add templates/partials/spec_view.html crates/barnstormer-server/src/web/mod.rs
git commit -m "feat: canvas SSE listener and pre-population in brainstorming view"
```

---

### Task 8: Integration tests and final verification

- [ ] **Step 1: Write SSE integration test**

```rust
#[tokio::test]
async fn state_api_includes_canvas_content() {
    let state = test_state();
    let app = create_router(Arc::clone(&state), None);
    app.oneshot(
        Request::post("/web/specs")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from("description=Canvas+state+test"))
            .unwrap(),
    ).await.unwrap();

    let spec_id = {
        let actors = state.actors.read().await;
        *actors.keys().next().unwrap()
    };

    {
        let actors = state.actors.read().await;
        let handle = actors.get(&spec_id).unwrap();
        handle.send_command(Command::UpdateCanvas {
            content: "<p>Check</p>".to_string(),
        }).await.unwrap();
    }

    let app2 = create_router(Arc::clone(&state), None);
    let resp = app2
        .oneshot(
            Request::get(&format!("/api/specs/{}/state", spec_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        json.get("canvas_content").and_then(|v| v.as_str()),
        Some("<p>Check</p>")
    );
}
```

- [ ] **Step 2: Run full workspace tests and clippy**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings`
Expected: all pass

- [ ] **Step 3: Commit**

```bash
git add crates/barnstormer-server/src/web/mod.rs
git commit -m "test: add integration tests for canvas state API"
```
