# Token Streaming & Live Agent Activity Design

**Goal:** Stream the manager agent's LLM responses token-by-token to the chat UI, and show a live "what's happening now" status line for worker agent activity.

**Approach:** Ephemeral events through the existing event broadcast → SSE pipeline. No new transport mechanisms.

**Tech Stack:** Rust (mux Hook trait, Axum SSE), JavaScript (EventSource listeners, DOM manipulation).

---

## 1. Ephemeral Events

Two new variants added to `EventPayload` and `Command`:

- **`StreamingDelta { agent_id: String, text: String }`** — a text fragment from the manager's LLM response.
- **`StreamingToolActivity { agent_id: String, activity: String }`** — a one-line description of what a worker is currently doing (e.g., "Researcher: creating card 'Auth Flow'").

These are **ephemeral**: the state reducer's `apply()` ignores them (no state mutation), and the persister skips them (no JSONL write). They exist solely to ride the broadcast channel to SSE subscribers.

An `is_ephemeral()` method on `EventPayload` lets the persister check before writing.

## 2. Mux Hook Implementation

A `StreamingHook` struct in `barnstormer-agent` implements the mux `Hook` trait, constructed with an `Arc<SpecActorHandle>`.

**Manager agent (streaming enabled):**
- `HookEvent::StreamDelta { text, .. }` → sends `Command::StreamingDelta { agent_id, text }`

**All agents (manager + workers):**
- `HookEvent::PostToolUse { tool_name, result, .. }` → sends `Command::StreamingToolActivity { agent_id, activity }` with a short description.
- `HookEvent::Iteration { .. }` → sends `StreamingToolActivity` with `"<agent>: thinking..."`.

Wiring in `swarm.rs`:
- Build a `HookRegistry` containing `StreamingHook`.
- Call `.with_hooks(hook_registry)` on SubAgent.
- Call `.streaming(true)` on `AgentDefinition` **only for the manager agent** (workers get hook callbacks but not token streaming).

## 3. SSE Event Mapping

Two new SSE event names in `stream.rs`:
- `StreamingDelta` → `"streaming_delta"`
- `StreamingToolActivity` → `"streaming_tool_activity"`

Same SSE pipe as all other events. No new endpoints. HTMX `hx-trigger` does NOT include these — JavaScript SSE listeners handle them directly (same pattern as the throbber).

## 4. Client-Side Rendering

### Manager token streaming (chat panel)

1. **First `streaming_delta`:** Hide throbber, create a new `.chat-message` div with manager avatar and empty `.chat-body`. Append to chat feed.
2. **Subsequent `streaming_delta`:** Append `text` to `.chat-body`'s `textContent`. Auto-scroll.
3. **`transcript_appended`:** HTMX re-fetches the full transcript, replacing the streaming placeholder with authoritative server-rendered HTML (with proper markdown formatting).

Streaming text is raw/unformatted while typing, then "snaps" to formatted HTML on completion. Standard chat UX pattern.

### Worker activity status line

1. A `<div id="{{container_id}}-activity" class="chat-activity-status">` sits below the throbber area.
2. On `streaming_tool_activity`: show element, overwrite text content with `activity` string.
3. On `agent_step_finished`: hide the element.
4. On `streaming_delta` from manager: also hide (manager is talking, worker status irrelevant).

Each `streaming_tool_activity` overwrites the previous — "tail -n 1" behavior. No accumulation.

## 5. Error Handling & Edge Cases

- **SSE drop mid-stream:** Orphaned partial text gets replaced on next HTMX transcript swap. Self-healing.
- **Broadcast lag:** Streaming deltas are high-frequency but tiny. If lagged, SSE silently drops them. Final `transcript_appended` delivers the complete message.
- **Workers with no tool calls:** `Iteration` hook fires "thinking..." until a tool call happens. `agent_step_finished` clears the status line.
- **No new failure modes.** Everything degrades gracefully to existing non-streaming behavior.

## 6. Testing

**Unit tests (barnstormer-core):**
- Round-trip serialization for both new event payloads.
- `is_ephemeral()` returns true for new variants, false for existing.
- `state.apply()` is a no-op for ephemeral events.

**Unit tests (barnstormer-agent):**
- `StreamingHook` sends correct commands for `StreamDelta`, `PostToolUse`, `Iteration` hook events.
- Hook `accepts()` filters correctly.

**Integration test (barnstormer-server):**
- SSE stream receives `streaming_delta` and `streaming_tool_activity` events.
- Persister skips ephemeral events (not written to JSONL).

## Files Touched

- `crates/barnstormer-core/src/event.rs` — new EventPayload variants, `is_ephemeral()`
- `crates/barnstormer-core/src/command.rs` — new Command variants
- `crates/barnstormer-core/src/state.rs` — no-op apply arms for ephemeral events
- `crates/barnstormer-core/src/actor.rs` — command_to_events for new commands
- `crates/barnstormer-agent/src/streaming_hook.rs` — new file, StreamingHook implementation
- `crates/barnstormer-agent/src/swarm.rs` — wire hooks + .streaming(true) for manager
- `crates/barnstormer-server/src/api/stream.rs` — new event_type_name arms
- `crates/barnstormer-server/src/web/mod.rs` — persister skip for ephemeral events
- `templates/partials/chat_transcript.html` — streaming div, activity status line, JS listeners
- `static/style.css` — streaming message + activity status styling
