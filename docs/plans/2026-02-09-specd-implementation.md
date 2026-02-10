# specd Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build specd — a local-first, web-based, agentic spec workstation that drives discovery through questions, renders specs as cards + document, and emits Markdown/YAML/DOT artifacts.

**Architecture:** Rust workspace with 4 library crates (specd-core, specd-store, specd-server, specd-agent) and a thin binary. Event-sourced via append-only JSONL with SQLite cache. Per-spec actor model for concurrency. Server-rendered web UI via Axum + Askama + HTMX + SSE.

**Tech Stack:** Rust (edition 2024), Axum, Askama, HTMX, SSE, SQLite (rusqlite), ULID, serde/serde_json, tokio, tower

---

## Phase 1: Scaffold + Core Types

### Task 1: Convert to Workspace and Scaffold Crates

**Files:**
- Modify: `Cargo.toml` (convert to workspace root + binary)
- Create: `crates/specd-core/Cargo.toml`
- Create: `crates/specd-core/src/lib.rs`
- Create: `crates/specd-store/Cargo.toml`
- Create: `crates/specd-store/src/lib.rs`
- Create: `crates/specd-server/Cargo.toml`
- Create: `crates/specd-server/src/lib.rs`
- Create: `crates/specd-agent/Cargo.toml`
- Create: `crates/specd-agent/src/lib.rs`
- Modify: `src/main.rs` (import and call specd-server)

**Step 1: Create workspace Cargo.toml and crate scaffolds**

Root `Cargo.toml`:
```toml
[workspace]
members = ["crates/*"]
resolver = "3"

[workspace.package]
edition = "2024"
version = "0.1.0"

[workspace.dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tokio = { version = "1", features = ["full"] }
ulid = { version = "1", features = ["serde"] }
chrono = { version = "0.4", features = ["serde"] }
thiserror = "2"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
axum = { version = "0.8", features = ["macros"] }
tower = "0.5"
tower-http = { version = "0.6", features = ["fs", "cors", "trace"] }
askama = "0.13"
askama_axum = "0.5"
rusqlite = { version = "0.34", features = ["bundled"] }
async-trait = "0.1"

specd-core = { path = "crates/specd-core" }
specd-store = { path = "crates/specd-store" }
specd-server = { path = "crates/specd-server" }
specd-agent = { path = "crates/specd-agent" }

[package]
name = "specd"
edition.workspace = true
version.workspace = true

[[bin]]
name = "specd"
path = "src/main.rs"

[dependencies]
specd-server.workspace = true
tokio.workspace = true
tracing.workspace = true
tracing-subscriber.workspace = true
clap = { version = "4", features = ["derive"] }
dotenvy = "0.15"
```

Each crate `Cargo.toml` follows the pattern:
```toml
[package]
name = "specd-core"
edition.workspace = true
version.workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
ulid.workspace = true
chrono.workspace = true
thiserror.workspace = true
tracing.workspace = true
```

**Step 2: Verify it compiles**

Run: `cargo check`
Expected: Compiles with no errors.

**Step 3: Commit**

```
feat: scaffold workspace with specd-core, specd-store, specd-server, specd-agent crates
```

---

### Task 2: Core Data Model — Spec + Card Types

**Files:**
- Create: `crates/specd-core/src/model.rs`
- Create: `crates/specd-core/src/card.rs`
- Test: `crates/specd-core/src/model.rs` (inline tests)

**Step 1: Write failing tests for Spec and Card construction**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_new_sets_required_fields() {
        let spec = SpecCore::new("My Spec".into(), "A one-liner".into(), "The goal".into());
        assert_eq!(spec.title, "My Spec");
        assert_eq!(spec.one_liner, "A one-liner");
        assert_eq!(spec.goal, "The goal");
        assert!(!spec.spec_id.to_string().is_empty());
    }

    #[test]
    fn card_defaults_to_ideas_lane() {
        let card = Card::new("idea".into(), "My Idea".into(), "human".into());
        assert_eq!(card.lane, "Ideas");
        assert_eq!(card.card_type, "idea");
    }
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p specd-core`
Expected: FAIL — types don't exist yet.

**Step 3: Implement SpecCore and Card structs**

`SpecCore` fields: spec_id (Ulid), title, one_liner, goal, description, constraints, success_criteria, risks, notes, created_at, updated_at.

`Card` fields: card_id (Ulid), card_type, title, body, lane, order (f64), refs, created_at, updated_at, created_by, updated_by.

Both derive `Debug, Clone, Serialize, Deserialize`.

**Step 4: Run tests to verify they pass**

Run: `cargo test -p specd-core`
Expected: PASS

**Step 5: Commit**

```
feat: add SpecCore and Card data model types
```

---

### Task 3: Event Types + Command Types

**Files:**
- Create: `crates/specd-core/src/event.rs`
- Create: `crates/specd-core/src/command.rs`
- Test: inline tests in both files

**Step 1: Write failing tests for event serialization round-trip**

```rust
#[test]
fn event_serializes_round_trip() {
    let event = Event {
        event_id: 1,
        spec_id: Ulid::new(),
        timestamp: Utc::now(),
        payload: EventPayload::SpecCreated { title: "T".into(), one_liner: "O".into(), goal: "G".into() },
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.event_id, 1);
}
```

**Step 2: Run tests — expect FAIL**

**Step 3: Implement Event and EventPayload enum**

Event envelope: `event_id` (u64), `spec_id` (Ulid), `timestamp` (DateTime<Utc>), `payload` (EventPayload).

EventPayload variants (from spec Appendix A):
- `SpecCreated { title, one_liner, goal }`
- `SpecCoreUpdated { field changes as Option<String> }`
- `CardCreated { card: Card }`
- `CardUpdated { card_id, changes }`
- `CardMoved { card_id, lane, order }`
- `CardDeleted { card_id }`
- `TranscriptAppended { message: TranscriptMessage }`
- `QuestionAsked { question: UserQuestion }`
- `QuestionAnswered { question_id, answer }`
- `AgentStepStarted { agent_id, description }`
- `AgentStepFinished { agent_id, diff_summary }`
- `UndoApplied { target_event_id, inverse_patch }`
- `SnapshotWritten { snapshot_id }`

Command enum mirrors this but represents intent (pre-validation).

**Step 4: Run tests — expect PASS**

**Step 5: Commit**

```
feat: add Event, EventPayload, and Command types with serde round-trip
```

---

## Phase 2: Event Store + State Reducer

### Task 4: State Reducer — Apply Events to Build State

**Files:**
- Create: `crates/specd-core/src/state.rs`
- Test: inline tests

**Step 1: Write failing test — applying SpecCreated produces a state with spec**

```rust
#[test]
fn apply_spec_created_sets_core_fields() {
    let mut state = SpecState::default();
    let event = make_event(EventPayload::SpecCreated {
        title: "T".into(), one_liner: "O".into(), goal: "G".into()
    });
    state.apply(&event);
    assert_eq!(state.core.title, "T");
}
```

**Step 2: Run test — FAIL**

**Step 3: Implement SpecState with apply method**

`SpecState` holds: `core: SpecCore`, `cards: BTreeMap<Ulid, Card>`, `transcript: Vec<TranscriptMessage>`, `pending_question: Option<UserQuestion>`, `undo_stack: Vec<UndoEntry>`, `last_event_id: u64`.

`apply(&mut self, event: &Event)` pattern-matches on EventPayload and mutates state.

**Step 4: Write additional tests for card operations + undo**

- `apply_card_created_adds_card`
- `apply_card_updated_modifies_card`
- `apply_card_moved_changes_lane_and_order`
- `apply_card_deleted_removes_card`
- `apply_question_asked_sets_pending`
- `apply_question_answered_clears_pending`

**Step 5: Run all tests — PASS**

**Step 6: Commit**

```
feat: implement SpecState reducer — apply events to build materialized state
```

---

### Task 5: JSONL Event Log — Append + Replay

**Files:**
- Create: `crates/specd-store/src/jsonl.rs`
- Test: inline tests (using tempdir)

Dependencies to add to specd-store: `tempfile` (dev-dependency)

**Step 1: Write failing test — append event, replay gets it back**

```rust
#[test]
fn append_and_replay_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    let mut log = JsonlLog::open(&path).unwrap();

    let event = make_test_event(1);
    log.append(&event).unwrap();

    let events = JsonlLog::replay(&path).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event_id, 1);
}
```

**Step 2: Run test — FAIL**

**Step 3: Implement JsonlLog**

- `open(path)` — opens file in append mode
- `append(event)` — serialize to JSON, write line, fsync
- `replay(path)` — read file line by line, deserialize, collect
- `repair(path)` — truncate last partial line if present

**Step 4: Write test for crash recovery (partial last line)**

```rust
#[test]
fn repair_truncates_partial_last_line() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.jsonl");
    // Write a valid line + partial garbage
    std::fs::write(&path, "{\"event_id\":1}\n{\"event_id\":2").unwrap();
    JsonlLog::repair(&path).unwrap();
    let events = JsonlLog::replay(&path).unwrap();
    assert_eq!(events.len(), 1);
}
```

**Step 5: Run all tests — PASS**

**Step 6: Commit**

```
feat: implement JSONL event log with append, replay, and crash repair
```

---

### Task 6: Snapshot Save + Load

**Files:**
- Create: `crates/specd-store/src/snapshot.rs`
- Test: inline tests

**Step 1: Write failing test — save and load snapshot round-trip**

```rust
#[test]
fn snapshot_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let state = make_test_state();
    let path = dir.path().join("snapshots");
    save_snapshot(&path, &state, 42).unwrap();
    let (loaded, event_id) = load_latest_snapshot(&path).unwrap().unwrap();
    assert_eq!(event_id, 42);
    assert_eq!(loaded.core.title, state.core.title);
}
```

**Step 2: Implement snapshot save/load**

- Snapshots saved as `state_<event_id>.json` in a `snapshots/` directory.
- `save_snapshot(dir, state, event_id)` — atomic write (write to tmp, rename).
- `load_latest_snapshot(dir)` — find highest-numbered snapshot, deserialize.
- `SnapshotData` includes: `SpecState`, `last_event_id`, `agent_contexts` (placeholder map).

**Step 3: Tests PASS, commit**

```
feat: implement snapshot save/load with atomic writes
```

---

### Task 7: Spec Actor — Command Queue + Event Publishing

**Files:**
- Create: `crates/specd-core/src/actor.rs`
- Test: inline tests (tokio::test)

**Step 1: Write failing test — send command, receive event**

```rust
#[tokio::test]
async fn actor_processes_create_card_command() {
    let (actor, mut event_rx) = SpecActor::spawn(initial_state());
    actor.send(Command::CreateCard { card_type: "idea".into(), title: "Test".into(), created_by: "human".into() }).await.unwrap();
    let event = event_rx.recv().await.unwrap();
    matches!(event.payload, EventPayload::CardCreated { .. });
}
```

**Step 2: Implement SpecActor**

- `SpecActor` owns: `SpecState`, `mpsc::Receiver<Command>`, `broadcast::Sender<Event>`, `next_event_id: u64`
- `spawn()` returns `(SpecActorHandle, broadcast::Receiver<Event>)`
- `SpecActorHandle` wraps `mpsc::Sender<Command>` with a `send` method
- Actor loop: receive command → validate → create event(s) → apply to state → broadcast
- Command validation: e.g., reject AskUserQuestion if one is already pending

**Step 3: Write test for question queue enforcement**

```rust
#[tokio::test]
async fn actor_rejects_second_pending_question() {
    // Send a question, then try to send another before answering — expect error
}
```

**Step 4: Tests PASS, commit**

```
feat: implement SpecActor with command queue and event broadcasting
```

---

## Phase 3: HTTP API + SSE Streaming

### Task 8: Axum Server Scaffold + Health Endpoint

**Files:**
- Modify: `crates/specd-server/src/lib.rs`
- Create: `crates/specd-server/src/routes.rs`
- Create: `crates/specd-server/src/app_state.rs`
- Modify: `src/main.rs`

**Step 1: Write test — GET /health returns 200**

```rust
#[tokio::test]
async fn health_returns_ok() {
    let app = create_app(test_state()).await;
    let resp = app.oneshot(Request::get("/health").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
```

**Step 2: Implement Axum app with router, AppState, health route**

`AppState` holds: map of `spec_id → SpecActorHandle`, `specd_home` path, config.

Router: `GET /health`, and static file serving for HTMX/CSS.

`src/main.rs`: CLI with clap (`start`, `status` subcommands), dotenvy for config, bind to `SPECD_BIND` (default `127.0.0.1:7331`).

**Step 3: Test PASS, commit**

```
feat: scaffold Axum server with health endpoint and CLI
```

---

### Task 9: Spec CRUD API

**Files:**
- Create: `crates/specd-server/src/api/specs.rs`
- Create: `crates/specd-server/src/api/mod.rs`

**Step 1: Write tests for spec list + create**

```rust
#[tokio::test]
async fn create_spec_returns_201_with_id() { ... }

#[tokio::test]
async fn list_specs_returns_created_spec() { ... }

#[tokio::test]
async fn get_spec_state_returns_current_state() { ... }
```

**Step 2: Implement API routes**

- `GET /api/specs` → list specs (id, title, updated_at)
- `POST /api/specs` → create spec (title, one_liner, goal) → starts actor, returns spec_id
- `GET /api/specs/{id}/state` → current materialized state as JSON

**Step 3: Tests PASS, commit**

```
feat: implement spec CRUD API endpoints
```

---

### Task 10: Command Submission API

**Files:**
- Create: `crates/specd-server/src/api/commands.rs`

**Step 1: Write test — POST command creates card**

```rust
#[tokio::test]
async fn post_create_card_command_returns_ok() {
    // Create spec, then POST /api/specs/{id}/commands with CreateCard
    // GET state, verify card exists
}
```

**Step 2: Implement `POST /api/specs/{id}/commands`**

Accept JSON command body, route to spec actor, return result.

**Step 3: Write test for undo**

```rust
#[tokio::test]
async fn post_undo_reverts_last_card() { ... }
```

**Step 4: Implement `POST /api/specs/{id}/undo`**

**Step 5: Tests PASS, commit**

```
feat: implement command submission and undo API endpoints
```

---

### Task 11: SSE Event Stream

**Files:**
- Create: `crates/specd-server/src/api/stream.rs`

**Step 1: Write test — SSE stream receives events after command**

```rust
#[tokio::test]
async fn sse_stream_receives_card_created_event() {
    // Create spec, connect to SSE, send CreateCard command, read event from stream
}
```

**Step 2: Implement `GET /api/specs/{id}/events/stream`**

Use Axum's SSE support (`axum::response::sse::Sse`). Subscribe to actor's broadcast channel. Each event serialized as SSE `data:` line with event type.

**Step 3: Tests PASS, commit**

```
feat: implement SSE event streaming for real-time spec updates
```

---

## Phase 4: Web UI (Askama + HTMX)

### Task 12: Base Layout + Static Assets

**Files:**
- Create: `templates/base.html`
- Create: `templates/index.html`
- Create: `static/htmx.min.js` (vendor from CDN or bundle)
- Create: `static/style.css`
- Create: `crates/specd-server/src/web/mod.rs`

**Step 1: Create base Askama template with HTMX**

Three-panel layout: left rail (spec switcher), main content (board/doc), right rail (activity).
Include HTMX via `<script>` and SSE extension.

**Step 2: Implement route `GET /` → render index**

**Step 3: Verify it renders in browser**

Run: `cargo run -- start --no-open` then `curl http://127.0.0.1:7331/`
Expected: HTML response with layout structure.

**Step 4: Commit**

```
feat: add base web layout with Askama templates and HTMX
```

---

### Task 13: Spec Switcher (Left Rail)

**Files:**
- Create: `templates/partials/spec_list.html`
- Create: `templates/partials/create_spec_form.html`
- Modify: `crates/specd-server/src/web/mod.rs`

**Step 1: Implement spec list partial**

HTMX: `hx-get="/web/specs"` loads spec list. Clicking a spec sets it as active.
Create form: `hx-post="/web/specs"` with title/one_liner/goal fields.

**Step 2: Wire up web routes for spec list + create (HTML responses)**

- `GET /web/specs` → render spec list partial
- `POST /web/specs` → create spec, return updated list
- `GET /web/specs/{id}` → render main view for selected spec

**Step 3: Verify in browser, commit**

```
feat: implement spec switcher with create form in left rail
```

---

### Task 14: Board View (Cards)

**Files:**
- Create: `templates/partials/board.html`
- Create: `templates/partials/card.html`
- Create: `templates/partials/card_form.html`
- Create: `static/board.js` (minimal JS for drag-and-drop via SortableJS)

**Step 1: Implement board template**

Three default lanes: Ideas, Plan, Done. Cards rendered in each lane by order.
Each card shows: type badge, title, body preview.
HTMX: SSE updates swap board content. Card click opens edit form.

**Step 2: Implement card CRUD via HTMX**

- Create card: `hx-post` form submits CreateCard command
- Edit card: inline form, `hx-put` submits UpdateCard
- Delete card: `hx-delete` with confirmation
- Move card: SortableJS drag fires `hx-post` with MoveCard command (lane + order)

**Step 3: Wire up web routes returning HTML partials**

**Step 4: Verify drag-and-drop in browser, commit**

```
feat: implement board view with card CRUD and drag-and-drop
```

---

### Task 15: Document View

**Files:**
- Create: `templates/partials/document.html`

**Step 1: Implement document view template**

Renders spec as a single scrollable document:
- Title, one-liner, goal at top
- Optional fields (description, constraints, etc.)
- Cards grouped by lane, ordered
- Toggle between board/doc via HTMX tab switching

**Step 2: Wire up route `GET /web/specs/{id}/document`**

**Step 3: Commit**

```
feat: implement document view for spec narrative rendering
```

---

### Task 16: Agent Activity Panel (Right Rail) + Question Widget

**Files:**
- Create: `templates/partials/activity.html`
- Create: `templates/partials/question.html`

**Step 1: Implement activity panel**

SSE-driven live feed: narration messages, diff summaries, event log.
Uses HTMX SSE extension to append new entries.

**Step 2: Implement question widget**

When pending question exists, render appropriate form:
- Boolean: Yes/No buttons
- Multiple choice: radio/checkbox list
- Freeform: text input

Submit answer via `hx-post` → AnswerUserQuestion command.

**Step 3: Implement agent pause/resume toggle**

`hx-post="/web/specs/{id}/agents/pause"` / `resume`

**Step 4: Commit**

```
feat: implement activity panel with live narration and question widget
```

---

## Phase 5: Exporters

### Task 17: Markdown Exporter

**Files:**
- Create: `crates/specd-core/src/export/mod.rs`
- Create: `crates/specd-core/src/export/markdown.rs`
- Test: inline tests

**Step 1: Write failing test — export state to markdown**

```rust
#[test]
fn export_markdown_includes_title_and_cards() {
    let state = make_state_with_cards();
    let md = export_markdown(&state);
    assert!(md.contains("# My Spec"));
    assert!(md.contains("## Ideas"));
    assert!(md.contains("- **Card Title**"));
}
```

**Step 2: Implement export_markdown**

Deterministic ordering per spec: header → lanes (Ideas, Plan, Done, then alpha) → cards by order then card_id.

**Step 3: Tests PASS, commit**

```
feat: implement deterministic Markdown exporter
```

---

### Task 18: YAML Exporter

**Files:**
- Create: `crates/specd-core/src/export/yaml.rs`

**Step 1: Write failing test — export to YAML matches expected structure**

**Step 2: Implement export_yaml using serde_yaml**

Add `serde_yaml` to workspace deps.

**Step 3: Tests PASS, commit**

```
feat: implement YAML exporter
```

---

### Task 19: DOT Exporter

**Files:**
- Create: `crates/specd-core/src/export/dot.rs`

**Step 1: Write failing test — DOT output has start/done nodes and card nodes**

```rust
#[test]
fn export_dot_has_start_and_done_nodes() {
    let state = make_state_with_cards();
    let dot = export_dot(&state);
    assert!(dot.contains("start [shape=Mdiamond"));
    assert!(dot.contains("done [shape=Msquare"));
}
```

**Step 2: Implement export_dot**

Conforms to DOT Runner constrained DSL from spec Section 9.3:
- `digraph <spec_id> { ... }`
- `start` (Mdiamond) and `done` (Msquare)
- Cards as nodes with shapes based on type
- Edges based on lane ordering (Ideas → Plan → Done flow)

**Step 3: Wire up auto-export — after each event, write exports to disk**

Integrate with SpecActor: after applying events, trigger export writes (best-effort, non-blocking).

**Step 4: Tests PASS, commit**

```
feat: implement DOT exporter conforming to DOT Runner DSL
```

---

## Phase 6: Agent System

### Task 20: Agent Runtime Trait

**Files:**
- Create: `crates/specd-agent/src/runtime.rs`
- Create: `crates/specd-agent/src/tools.rs`

**Step 1: Define AgentRuntime trait**

```rust
#[async_trait]
pub trait AgentRuntime: Send + Sync {
    async fn run_step(&self, context: &AgentContext) -> Result<AgentAction>;
}
```

Where `AgentAction` is an enum: `EmitNarration(String)`, `WriteCommands(Vec<Command>)`, `AskUser(UserQuestion)`, `AskAgent { agent_id, question }`, `Done`.

**Step 2: Define tool descriptions for LLM tool-calling**

Tools map to the spec's agent tooling (Section 8.2): ask_user_boolean, ask_user_multiple_choice, ask_user_freeform, read_state, write_commands, emit_narration, etc.

**Step 3: Commit**

```
feat: define AgentRuntime trait and agent tool specifications
```

---

### Task 21: LLM Provider Adapter — Anthropic

**Files:**
- Create: `crates/specd-agent/src/providers/mod.rs`
- Create: `crates/specd-agent/src/providers/anthropic.rs`

**Step 1: Implement Anthropic adapter**

Uses `reqwest` to call Anthropic Messages API with tool_use. Translates tool calls to `AgentAction`. Reads `ANTHROPIC_API_KEY` and `ANTHROPIC_BASE_URL` from env.

Note: This is a real API adapter, not a mock. Tests that hit the API are gated behind `#[cfg(feature = "live-test")]`.

**Step 2: Write integration test (gated)**

```rust
#[tokio::test]
#[cfg(feature = "live-test")]
async fn anthropic_adapter_returns_action() { ... }
```

**Step 3: Commit**

```
feat: implement Anthropic LLM provider adapter
```

---

### Task 22: LLM Provider Adapters — OpenAI + Gemini

**Files:**
- Create: `crates/specd-agent/src/providers/openai.rs`
- Create: `crates/specd-agent/src/providers/gemini.rs`

Same pattern as Anthropic. OpenAI uses chat completions + function calling. Gemini uses generateContent + function declarations. Both read API key + base URL from env.

**Commit:**
```
feat: implement OpenAI and Gemini LLM provider adapters
```

---

### Task 23: Swarm Orchestrator

**Files:**
- Create: `crates/specd-agent/src/swarm.rs`

**Step 1: Implement SwarmOrchestrator**

Per spec Section 8.1, each spec spawns:
- `manager` — reconciler, policy enforcer
- `brainstormer` — elicits intent, generates ideas
- `planner` — organizes into structure
- `dot_generator` — keeps pipeline.dot updated
- `critic` (optional)

Each agent runs as a tokio task. Commands funnel through the spec actor. Question queue enforced: only one pending question per spec.

**Step 2: Write test — swarm starts and agents can emit narration**

**Step 3: Implement agent pause/resume**

**Step 4: Commit**

```
feat: implement swarm orchestrator with agent lifecycle management
```

---

### Task 24: Context Snapshots for Agents

**Files:**
- Create: `crates/specd-agent/src/context.rs`

Per spec Section 8.4: rolling summaries per agent, key decisions + rationales, last_event_seen per agent. Capped size. Persisted in snapshot files.

**Commit:**
```
feat: implement agent context snapshots with rolling summaries
```

---

## Phase 7: Resilience + Polish

### Task 25: SQLite Index + Rebuild

**Files:**
- Create: `crates/specd-store/src/sqlite.rs`

**Step 1: Write test — index specs and cards, query back**

**Step 2: Implement SQLite indexer**

Tables: `specs (spec_id, title, one_liner, updated_at)`, `cards (card_id, spec_id, type, title, lane, order)`.
On startup: check `last_applied_event_id`, rebuild if stale.

**Step 3: Commit**

```
feat: implement SQLite index with auto-rebuild from JSONL
```

---

### Task 26: Crash Recovery + Self-Heal

**Files:**
- Modify: `crates/specd-store/src/jsonl.rs`
- Create: `crates/specd-store/src/recovery.rs`

**Step 1: Implement startup recovery sequence**

Per spec Section 5.3:
1. Load latest snapshot (if any)
2. Replay JSONL tail from snapshot's event_id
3. Verify SQLite integrity
4. Rebuild SQLite if mismatch
5. Resume agents from context snapshots

**Step 2: Write integration test — simulate crash, verify recovery**

**Step 3: Commit**

```
feat: implement crash recovery with snapshot + JSONL replay + self-heal
```

---

### Task 27: Storage Manager + Daemon Lifecycle

**Files:**
- Create: `crates/specd-store/src/manager.rs`
- Modify: `src/main.rs`

**Step 1: Implement StorageManager**

- Initializes `SPECD_HOME` directory structure
- Opens/creates spec directories on demand
- Manages snapshot scheduling (every N events or M minutes)
- Clean shutdown: flush pending writes, save snapshots

**Step 2: Wire into main daemon lifecycle**

`specd start`: init storage → recover specs → start actors → start HTTP server → open browser (unless --no-open).

**Step 3: Commit**

```
feat: implement storage manager and daemon lifecycle
```

---

## Phase 8: End-to-End Integration

### Task 28: Smoke Test — Full Flow

**Files:**
- Create: `tests/smoke.rs` (integration test at workspace root)

**Step 1: Write end-to-end test**

1. Start daemon (in-process)
2. Create spec via API
3. Submit CreateCard commands
4. Verify SSE stream receives events
5. Verify exports generated (spec.md, spec.yaml, pipeline.dot)
6. Verify board view renders cards
7. Submit undo, verify card removed

**Step 2: Run smoke test — PASS**

**Step 3: Commit**

```
test: add end-to-end smoke test covering spec lifecycle
```

---

### Task 29: Configuration + Security

**Files:**
- Create: `crates/specd-server/src/config.rs`
- Create: `.env.example`

**Step 1: Implement config loading**

Read all env vars from spec Section 11. Validate: if `SPECD_ALLOW_REMOTE=true`, require `SPECD_AUTH_TOKEN`.

**Step 2: Implement auth middleware**

If auth token configured, require `Authorization: Bearer <token>` header on API routes.

**Step 3: Commit**

```
feat: implement configuration loading and auth middleware
```

---

### Task 30: Final Polish + CLAUDE.md

**Files:**
- Create: `CLAUDE.md` (project-level)
- Verify all exports, recovery, UI

**Step 1: Write CLAUDE.md with project conventions**

**Step 2: Final cargo fmt + clippy + test pass**

Run: `cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test --all`

**Step 3: Commit**

```
chore: add project CLAUDE.md and final polish pass
```

---

## Execution Notes

**Parallelizable tasks (cookoff candidates):**
- Tasks 17, 18, 19 (exporters) can run in parallel
- Tasks 21, 22 (LLM adapters) can run in parallel
- Tasks 12-16 (UI templates) can run in parallel after Task 8-11

**Sequential dependencies:**
- Phase 1 → Phase 2 → Phase 3 (core → store → server)
- Phase 4 depends on Phase 3 (UI needs API)
- Phase 5 depends on Phase 2 (exporters need state model)
- Phase 6 depends on Phase 2 + Phase 3 (agents need actor + API)
- Phase 7 depends on everything

**Port:** `7331` (leet-speak for "LEET" reversed, thematic for a spec tool)
