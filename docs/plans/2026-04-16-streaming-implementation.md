# Token Streaming & Live Agent Activity Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Stream the manager agent's LLM responses token-by-token to the chat UI and show a live worker activity status line, using ephemeral events through the existing broadcast → SSE pipeline.

**Architecture:** Two new ephemeral `EventPayload` variants (`StreamingDelta`, `StreamingToolActivity`) ride the broadcast channel to SSE but are ignored by the state reducer and persister. A `StreamingHook` in barnstormer-agent implements the mux `Hook` trait to bridge LLM streaming callbacks to the actor's command channel. The browser handles rendering via JavaScript SSE listeners.

**Tech Stack:** Rust (mux Hook trait, Axum SSE, Askama templates), JavaScript (EventSource listeners, DOM manipulation), CSS.

**Design doc:** `docs/plans/2026-04-16-streaming-design.md`

---

### Task 1: Add ephemeral event variants to barnstormer-core

Add `StreamingDelta` and `StreamingToolActivity` to `Command`, `EventPayload`, the actor's command handler, and the state reducer. Add `is_ephemeral()` to `EventPayload`.

**Files:**
- Modify: `crates/barnstormer-core/src/command.rs`
- Modify: `crates/barnstormer-core/src/event.rs`
- Modify: `crates/barnstormer-core/src/actor.rs`
- Modify: `crates/barnstormer-core/src/state.rs`

**Step 1: Write failing tests**

In `crates/barnstormer-core/src/event.rs`, add two serialization round-trip tests at the bottom of the `mod tests` block:

```rust
#[test]
fn streaming_delta_round_trip() {
    round_trip_event(EventPayload::StreamingDelta {
        agent_id: "manager-1".to_string(),
        text: "Hello".to_string(),
    });
}

#[test]
fn streaming_tool_activity_round_trip() {
    round_trip_event(EventPayload::StreamingToolActivity {
        agent_id: "brainstormer-1".to_string(),
        activity: "creating card 'Auth Flow'".to_string(),
    });
}

#[test]
fn is_ephemeral_returns_true_for_streaming_events() {
    assert!(EventPayload::StreamingDelta {
        agent_id: String::new(),
        text: String::new(),
    }.is_ephemeral());
    assert!(EventPayload::StreamingToolActivity {
        agent_id: String::new(),
        activity: String::new(),
    }.is_ephemeral());
}

#[test]
fn is_ephemeral_returns_false_for_durable_events() {
    assert!(!EventPayload::SpecCreated {
        title: String::new(),
        one_liner: String::new(),
        goal: String::new(),
    }.is_ephemeral());
    assert!(!EventPayload::TranscriptAppended {
        message: TranscriptMessage::new("x".into(), "y".into()),
    }.is_ephemeral());
}
```

In `crates/barnstormer-core/src/state.rs` tests, add:

```rust
#[test]
fn apply_streaming_delta_is_noop() {
    let mut state = SpecState::new();
    let spec_id = make_spec_id();
    let before = state.clone();
    state.apply(&make_event(
        spec_id,
        1,
        EventPayload::StreamingDelta {
            agent_id: "manager-1".to_string(),
            text: "Hello".to_string(),
        },
    ));
    // State should be unchanged except for last_event_id
    assert_eq!(state.core, before.core);
    assert_eq!(state.cards.len(), before.cards.len());
    assert_eq!(state.transcript.len(), before.transcript.len());
}
```

In `crates/barnstormer-core/src/actor.rs` tests, add:

```rust
#[tokio::test]
async fn actor_broadcasts_streaming_delta() {
    let spec_id = Ulid::new();
    let handle = spawn(spec_id, SpecState::new());
    let mut rx = handle.subscribe();

    let events = handle
        .send_command(Command::StreamDelta {
            agent_id: "manager-1".to_string(),
            text: "Hi".to_string(),
        })
        .await
        .unwrap();

    assert_eq!(events.len(), 1);
    match &events[0].payload {
        EventPayload::StreamingDelta { agent_id, text } => {
            assert_eq!(agent_id, "manager-1");
            assert_eq!(text, "Hi");
        }
        _ => panic!("expected StreamingDelta"),
    }

    let broadcast = rx.recv().await.unwrap();
    match &broadcast.payload {
        EventPayload::StreamingDelta { .. } => {}
        _ => panic!("expected StreamingDelta broadcast"),
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --all 2>&1 | tail -20`
Expected: FAIL — `StreamingDelta`, `StreamingToolActivity`, `StreamDelta`, `StreamToolActivity` don't exist yet.

**Step 3: Implement the types and handlers**

In `crates/barnstormer-core/src/event.rs`, add two new variants to `EventPayload` (after `CanvasUpdated`):

```rust
StreamingDelta {
    agent_id: String,
    text: String,
},
StreamingToolActivity {
    agent_id: String,
    activity: String,
},
```

Add an `impl EventPayload` block (or extend existing one) with:

```rust
impl EventPayload {
    /// Returns true for events that should not be persisted to the event log.
    /// Streaming events are broadcast-only — they carry ephemeral LLM state
    /// that has no meaning during replay.
    pub fn is_ephemeral(&self) -> bool {
        matches!(
            self,
            EventPayload::StreamingDelta { .. } | EventPayload::StreamingToolActivity { .. }
        )
    }
}
```

In `crates/barnstormer-core/src/command.rs`, add two new variants to `Command` (after `Undo`):

```rust
StreamDelta {
    agent_id: String,
    text: String,
},
StreamToolActivity {
    agent_id: String,
    activity: String,
},
```

In `crates/barnstormer-core/src/actor.rs`, add two arms to `command_to_events` (before the closing `};` of the match):

```rust
Command::StreamDelta { agent_id, text } => {
    vec![EventPayload::StreamingDelta { agent_id, text }]
}

Command::StreamToolActivity { agent_id, activity } => {
    vec![EventPayload::StreamingToolActivity { agent_id, activity }]
}
```

In `crates/barnstormer-core/src/state.rs`, add two no-op arms to the `apply` method's match (before the closing `}` of the match):

```rust
EventPayload::StreamingDelta { .. } => {
    // Ephemeral — no state mutation
}
EventPayload::StreamingToolActivity { .. } => {
    // Ephemeral — no state mutation
}
```

Also add the same two arms to `apply_without_undo` if it has an exhaustive match.

**Step 4: Run tests to verify they pass**

Run: `cargo test --all 2>&1 | tail -20`
Expected: all tests pass.

**Step 5: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no warnings.

**Step 6: Commit**

```bash
git add crates/barnstormer-core/src/command.rs crates/barnstormer-core/src/event.rs crates/barnstormer-core/src/actor.rs crates/barnstormer-core/src/state.rs
git commit -m "feat: add ephemeral StreamingDelta and StreamingToolActivity events"
```

---

### Task 2: Wire SSE event names and persister skip

Map the new event variants to SSE event names and make the persister skip ephemeral events.

**Files:**
- Modify: `crates/barnstormer-server/src/api/stream.rs`
- Modify: `crates/barnstormer-server/src/web/mod.rs` (persister, around line 2671)

**Step 1: Write failing test**

In `crates/barnstormer-server/src/api/stream.rs`, add to `mod tests`:

```rust
#[test]
fn event_type_names_streaming() {
    use barnstormer_core::EventPayload;

    assert_eq!(
        event_type_name(&EventPayload::StreamingDelta {
            agent_id: String::new(),
            text: String::new(),
        }),
        "streaming_delta"
    );
    assert_eq!(
        event_type_name(&EventPayload::StreamingToolActivity {
            agent_id: String::new(),
            activity: String::new(),
        }),
        "streaming_tool_activity"
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p barnstormer-server 2>&1 | tail -20`
Expected: FAIL — non-exhaustive match in `event_type_name`.

**Step 3: Implement**

In `crates/barnstormer-server/src/api/stream.rs`, add two arms to `event_type_name`:

```rust
barnstormer_core::EventPayload::StreamingDelta { .. } => "streaming_delta",
barnstormer_core::EventPayload::StreamingToolActivity { .. } => "streaming_tool_activity",
```

In `crates/barnstormer-server/src/web/mod.rs`, in the `spawn_event_persister` function, change the event receive loop (around line 2671-2680) from:

```rust
Ok(event) => {
    if let Err(e) = log.append(&event) {
```

to:

```rust
Ok(event) => {
    if event.payload.is_ephemeral() {
        continue;
    }
    if let Err(e) = log.append(&event) {
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --all 2>&1 | tail -20`
Expected: all tests pass.

**Step 5: Commit**

```bash
git add crates/barnstormer-server/src/api/stream.rs crates/barnstormer-server/src/web/mod.rs
git commit -m "feat: map streaming events to SSE, skip ephemeral events in persister"
```

---

### Task 3: Implement StreamingHook in barnstormer-agent

Create the hook that bridges mux streaming callbacks to barnstormer commands.

**Files:**
- Create: `crates/barnstormer-agent/src/streaming_hook.rs`
- Modify: `crates/barnstormer-agent/src/lib.rs`

**Step 1: Write failing test**

Create `crates/barnstormer-agent/src/streaming_hook.rs` with tests that verify the hook sends the right commands:

```rust
// ABOUTME: Mux Hook implementation that forwards LLM streaming events to the spec actor.
// ABOUTME: Bridges StreamDelta and tool-use callbacks into ephemeral broadcast events.

use std::sync::Arc;

use async_trait::async_trait;
use barnstormer_core::{Command, SpecActorHandle};
use mux::hook::{Hook, HookAction, HookEvent};

/// Forwards mux streaming events to the spec actor as ephemeral commands.
/// For the manager agent, forwards text deltas (token-by-token streaming).
/// For all agents, forwards tool activity and iteration status.
pub struct StreamingHook {
    actor: Arc<SpecActorHandle>,
    agent_id: String,
    is_manager: bool,
}

impl StreamingHook {
    pub fn new(actor: Arc<SpecActorHandle>, agent_id: String, is_manager: bool) -> Self {
        Self {
            actor,
            agent_id,
            is_manager,
        }
    }
}

#[async_trait]
impl Hook for StreamingHook {
    fn accepts(&self, event: &HookEvent) -> bool {
        matches!(
            event,
            HookEvent::StreamDelta { .. }
                | HookEvent::PostToolUse { .. }
                | HookEvent::Iteration { .. }
        )
    }

    async fn on_event(&self, event: &HookEvent) -> Result<HookAction, anyhow::Error> {
        match event {
            HookEvent::StreamDelta { text, .. } if self.is_manager => {
                let _ = self
                    .actor
                    .send_command(Command::StreamDelta {
                        agent_id: self.agent_id.clone(),
                        text: text.clone(),
                    })
                    .await;
            }
            HookEvent::PostToolUse {
                tool_name, input, ..
            } => {
                // Build a short activity description from the tool name and key input fields
                let subject = input
                    .get("title")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let activity = if subject.is_empty() {
                    format!("{}: {}", self.agent_id, tool_name)
                } else {
                    format!("{}: {} '{}'", self.agent_id, tool_name, subject)
                };
                let _ = self
                    .actor
                    .send_command(Command::StreamToolActivity {
                        agent_id: self.agent_id.clone(),
                        activity,
                    })
                    .await;
            }
            HookEvent::Iteration { .. } => {
                let _ = self
                    .actor
                    .send_command(Command::StreamToolActivity {
                        agent_id: self.agent_id.clone(),
                        activity: format!("{}: thinking...", self.agent_id),
                    })
                    .await;
            }
            _ => {}
        }
        Ok(HookAction::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnstormer_core::{SpecState, spawn};
    use ulid::Ulid;

    #[tokio::test]
    async fn hook_sends_streaming_delta_for_manager() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        let mut rx = handle.subscribe();

        let hook = StreamingHook::new(
            Arc::new(handle.clone()),
            "manager-1".to_string(),
            true,
        );

        let event = HookEvent::StreamDelta {
            agent_id: "ignored".to_string(),
            text: "Hello".to_string(),
        };
        assert!(hook.accepts(&event));
        hook.on_event(&event).await.unwrap();

        let broadcast = rx.recv().await.unwrap();
        match &broadcast.payload {
            barnstormer_core::EventPayload::StreamingDelta { agent_id, text } => {
                assert_eq!(agent_id, "manager-1");
                assert_eq!(text, "Hello");
            }
            _ => panic!("expected StreamingDelta, got {:?}", broadcast.payload),
        }
    }

    #[tokio::test]
    async fn hook_ignores_streaming_delta_for_non_manager() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        let mut rx = handle.subscribe();

        let hook = StreamingHook::new(
            Arc::new(handle.clone()),
            "brainstormer-1".to_string(),
            false,
        );

        let event = HookEvent::StreamDelta {
            agent_id: "ignored".to_string(),
            text: "Hello".to_string(),
        };
        hook.on_event(&event).await.unwrap();

        // Should NOT have received anything
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            rx.recv(),
        )
        .await;
        assert!(result.is_err(), "should timeout — no event broadcast");
    }

    #[tokio::test]
    async fn hook_sends_tool_activity_for_any_agent() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        let mut rx = handle.subscribe();

        let hook = StreamingHook::new(
            Arc::new(handle.clone()),
            "brainstormer-1".to_string(),
            false,
        );

        let event = HookEvent::PostToolUse {
            tool_name: "create_card".to_string(),
            tool_use_id: "tu-1".to_string(),
            input: serde_json::json!({ "title": "Auth Flow" }),
            result: mux::tool::ToolResult {
                content: vec![],
                is_error: false,
            },
        };
        assert!(hook.accepts(&event));
        hook.on_event(&event).await.unwrap();

        let broadcast = rx.recv().await.unwrap();
        match &broadcast.payload {
            barnstormer_core::EventPayload::StreamingToolActivity { activity, .. } => {
                assert!(activity.contains("create_card"));
                assert!(activity.contains("Auth Flow"));
            }
            _ => panic!("expected StreamingToolActivity"),
        }
    }

    #[tokio::test]
    async fn hook_sends_thinking_on_iteration() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());
        let mut rx = handle.subscribe();

        let hook = StreamingHook::new(
            Arc::new(handle.clone()),
            "planner-1".to_string(),
            false,
        );

        let event = HookEvent::Iteration {
            agent_id: "ignored".to_string(),
            iteration: 1,
        };
        assert!(hook.accepts(&event));
        hook.on_event(&event).await.unwrap();

        let broadcast = rx.recv().await.unwrap();
        match &broadcast.payload {
            barnstormer_core::EventPayload::StreamingToolActivity { activity, .. } => {
                assert!(activity.contains("thinking"));
            }
            _ => panic!("expected StreamingToolActivity"),
        }
    }

    #[test]
    fn hook_rejects_irrelevant_events() {
        let spec_id = Ulid::new();
        let handle = spawn(spec_id, SpecState::new());

        let hook = StreamingHook::new(
            Arc::new(handle),
            "manager-1".to_string(),
            true,
        );

        let event = HookEvent::AgentStart {
            agent_id: "x".to_string(),
            task: "y".to_string(),
        };
        assert!(!hook.accepts(&event));
    }
}
```

**Step 2: Register the module**

In `crates/barnstormer-agent/src/lib.rs`, add:

```rust
pub mod streaming_hook;
```

**Step 3: Run tests to verify they pass**

Run: `cargo test -p barnstormer-agent 2>&1 | tail -20`
Expected: all tests pass.

**Step 4: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no warnings.

**Step 5: Commit**

```bash
git add crates/barnstormer-agent/src/streaming_hook.rs crates/barnstormer-agent/src/lib.rs
git commit -m "feat: add StreamingHook bridging mux callbacks to spec actor commands"
```

---

### Task 4: Wire hooks into swarm agent creation

Enable streaming on the manager's `AgentDefinition` and attach `StreamingHook` to all SubAgent instances.

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs` (around lines 388-401)

**Step 1: Add imports at the top of swarm.rs**

Add to the imports section:

```rust
use crate::streaming_hook::StreamingHook;
use mux::hook::HookRegistry;
```

**Step 2: Modify the `run_agent_step` function**

In the `run_agent_step` function (around line 388), after building the `AgentDefinition`:

```rust
let definition = AgentDefinition::new(
    runner.role.label(),
    full_system_prompt(&runner.role, &runner.agent_id, phase),
)
.model(model)
.max_iterations(10);
```

Change to conditionally enable streaming for the manager:

```rust
let is_manager = runner.role == AgentRole::Manager;
let mut definition = AgentDefinition::new(
    runner.role.label(),
    full_system_prompt(&runner.role, &runner.agent_id, phase),
)
.model(model)
.max_iterations(10);

if is_manager {
    definition = definition.streaming(true);
}
```

After creating the SubAgent (line 397-401), add hook wiring:

```rust
let mut sub_agent = SubAgent::new(
    definition,
    Arc::clone(client),
    registry,
);

// Attach streaming hook for real-time event forwarding
let hook_registry = Arc::new(HookRegistry::new());
let hook = StreamingHook::new(
    Arc::clone(actor),
    runner.agent_id.clone(),
    is_manager,
);
// Use block_on-safe approach: register is async because HookRegistry uses RwLock
{
    hook_registry.register(hook).await;
}
sub_agent = sub_agent.with_hooks(hook_registry);
```

**Step 3: Add AgentRole import if not already present**

Check imports at the top of `swarm.rs`. The `AgentRole` type should already be available via `use crate::context::AgentRole;` or similar. If not, add it.

**Step 4: Run tests to verify they pass**

Run: `cargo test --all 2>&1 | tail -20`
Expected: all tests pass.

**Step 5: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no warnings.

**Step 6: Commit**

```bash
git add crates/barnstormer-agent/src/swarm.rs
git commit -m "feat: wire StreamingHook into SubAgent creation, enable manager streaming"
```

---

### Task 5: Add streaming UI elements to chat transcript template

Add the streaming message placeholder, activity status line, and JavaScript SSE listeners.

**Files:**
- Modify: `templates/partials/chat_transcript.html`
- Modify: `static/style.css`

**Step 1: Add streaming DOM elements**

In `templates/partials/chat_transcript.html`, after the throbber div (line 44, after the closing `</div>` of the throbber), add:

```html
<div id="{{ container_id }}-streaming" class="chat-streaming-msg" style="display:none;">
    <div class="chat-message">
        <div class="chat-message-header">
            <div class="chat-avatar avatar-manager">O</div>
            <span class="chat-sender">Orchestrator</span>
        </div>
        <div class="chat-body" id="{{ container_id }}-streaming-body"></div>
    </div>
</div>
<div id="{{ container_id }}-activity" class="chat-activity-status" style="display:none;">
    <span class="status-dot dot-agent"></span>
    <span id="{{ container_id }}-activity-text"></span>
</div>
```

**Step 2: Add JavaScript SSE listeners**

In the `<script>` block at the bottom of `chat_transcript.html`, inside the `if (compositor)` block (after the existing `agent_step_finished` listener, around line 182), add:

```javascript
var streamingId = '{{ container_id }}-streaming';
var streamingBodyId = '{{ container_id }}-streaming-body';
var activityId = '{{ container_id }}-activity';
var activityTextId = '{{ container_id }}-activity-text';

compositor.addEventListener('sse:streaming_delta', function(evt) {
    var data = {};
    try { data = JSON.parse(evt.detail.data || '{}'); } catch(e) {}
    if (!data.payload || data.payload.type !== 'StreamingDelta') return;
    var text = data.payload.text || '';

    // Hide throbber, show streaming message
    var th = document.getElementById(throbberId);
    if (th) th.style.display = 'none';
    var act = document.getElementById(activityId);
    if (act) act.style.display = 'none';

    var sm = document.getElementById(streamingId);
    var sb = document.getElementById(streamingBodyId);
    if (sm && sb) {
        sm.style.display = '';
        sb.textContent += text;
    }

    // Auto-scroll
    var f = document.getElementById(feedId);
    if (f) f.scrollTop = f.scrollHeight;
});

compositor.addEventListener('sse:streaming_tool_activity', function(evt) {
    var data = {};
    try { data = JSON.parse(evt.detail.data || '{}'); } catch(e) {}
    if (!data.payload || data.payload.type !== 'StreamingToolActivity') return;
    var activity = data.payload.activity || '';

    var act = document.getElementById(activityId);
    var actText = document.getElementById(activityTextId);
    if (act && actText) {
        act.style.display = '';
        actText.textContent = activity;
    }

    // Auto-scroll
    var f = document.getElementById(feedId);
    if (f) f.scrollTop = f.scrollHeight;
});

// When transcript_appended arrives (HTMX will re-render), clean up streaming state.
// The HTMX swap replaces the entire container including our streaming div,
// so this is handled automatically. But we also listen for the raw SSE event
// to hide the streaming div slightly before the swap for a cleaner transition.
compositor.addEventListener('sse:transcript_appended', function() {
    var sm = document.getElementById(streamingId);
    if (sm) sm.style.display = 'none';
});

// Hide activity on agent_step_finished (already have a listener that hides throbber)
// Extend the existing listener or add a new one:
compositor.addEventListener('sse:agent_step_finished', function() {
    var act = document.getElementById(activityId);
    if (act) act.style.display = 'none';
});
```

Note: there's already an `agent_step_finished` listener that hides the throbber. The second listener here adds activity-hiding behavior. Alternatively, merge them into one listener — but two listeners on the same event is fine and keeps concerns separate.

**Step 3: Add CSS styling**

Append to `static/style.css`:

```css
/* Streaming message — raw text while tokens arrive */
.chat-streaming-msg {
    padding: 8px 20px;
}
.chat-streaming-msg .chat-body {
    white-space: pre-wrap;
    font-family: inherit;
    min-height: 20px;
}

/* Worker activity status line — "tail -n 1" of agent activity */
.chat-activity-status {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 4px 20px;
    font-size: 12px;
    color: var(--text-muted);
    opacity: 0.8;
}
.chat-activity-status .status-dot {
    flex-shrink: 0;
}
```

**Step 4: Build and verify**

Run: `cargo build --all 2>&1 | tail -5`
Expected: compilation succeeds (Askama templates are compiled at build time).

**Step 5: Commit**

```bash
git add templates/partials/chat_transcript.html static/style.css
git commit -m "feat: add streaming message placeholder and worker activity status line"
```

---

### Task 6: Update command round-trip tests

The existing command round-trip test in `command.rs` tests all variants. Add the new ones.

**Files:**
- Modify: `crates/barnstormer-core/src/command.rs`

**Step 1: Add to existing round-trip test**

In `crates/barnstormer-core/src/command.rs`, in the `command_serializes_round_trip` test, add to the `commands` vec:

```rust
Command::StreamDelta {
    agent_id: "manager-1".to_string(),
    text: "token".to_string(),
},
Command::StreamToolActivity {
    agent_id: "brainstormer-1".to_string(),
    activity: "creating card".to_string(),
},
```

**Step 2: Run tests**

Run: `cargo test -p barnstormer-core 2>&1 | tail -20`
Expected: all tests pass.

**Step 3: Commit**

```bash
git add crates/barnstormer-core/src/command.rs
git commit -m "test: add streaming command round-trip tests"
```

---

### Task 7: Full test suite and clippy

**Step 1: Run all tests**

Run: `cargo test --all 2>&1 | tail -30`
Expected: all tests pass.

**Step 2: Run clippy**

Run: `cargo clippy --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no warnings.

**Step 3: Fix any issues, commit if needed**

---

## Task Order & Dependencies

```
Task 1 (core event types)  ── must be first, all others depend on it
Task 2 (SSE + persister)   ── depends on Task 1
Task 3 (StreamingHook)     ── depends on Task 1
Task 4 (swarm wiring)      ── depends on Tasks 1 + 3
Task 5 (UI templates)      ── depends on Task 2 (SSE event names)
Task 6 (test cleanup)      ── depends on Task 1
Task 7 (full test suite)   ── must be last
```

Tasks 2, 3, and 5 can run in parallel after Task 1 is done (but with subagent-driven development they run sequentially).
