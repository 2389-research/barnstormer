# SPEC.md Gap Analysis

Generated: 2026-02-11 | Commit: 383a988
Baseline: `docs/SPEC.md` (v1 design spec, dated 2026-02-10)

This document catalogs every feature described in SPEC.md that is not yet implemented, partially implemented, or implemented differently than specified. Items are grouped by priority and effort.

---

## Quick Stats

- **74 verifiable features** in SPEC.md
- **53 done** (72%)
- **4 partial** (5%)
- **17 not done** (23%)

---

## 1. Not Implemented

### 1.1 Spec Core Editing via Web UI

**SPEC ref**: Section 3.3 — "Users can: Edit core spec fields (title, one-liner, goal)"

**What exists**: Users can create specs and view them. The `UpdateSpecCore` command exists and works via the API (`POST /api/specs/{id}/commands`). Agents use it.

**What's missing**: No web form or route for editing spec core fields. Users cannot change title, one_liner, goal, description, constraints, success_criteria, risks, or notes through the browser.

**Effort**: Small. Mirror the card edit form pattern — add an Askama template with inputs, a `PUT` or `POST` handler in `web/mod.rs`, and a route in `routes.rs`.

**Files to touch**:
- `templates/partials/spec_edit_form.html` (new)
- `crates/barnstormer-server/src/web/mod.rs` (add handler)
- `crates/barnstormer-server/src/routes.rs` (add route)

---

### 1.2 Spec Search/Filter in Left Rail

**SPEC ref**: Section 3.1 — "Spec switcher (list/search)"

**What exists**: Spec list renders all specs as clickable links. No filtering.

**What's missing**: A search input above the spec list that filters by title or one-liner.

**Effort**: Small. Add a text input to `spec_list.html` that uses HTMX `hx-trigger="keyup changed delay:300ms"` to filter the list client-side (or server-side with a query param).

**Files to touch**:
- `templates/partials/spec_list.html` (add search input)
- Optionally `crates/barnstormer-server/src/web/mod.rs` (server-side filter endpoint)

---

### 1.3 Settings Panel

**SPEC ref**: Section 3.1 — "Settings (LLM providers, storage folder, server bind, auth token)"

**What exists**: A read-only provider status indicator in the left rail showing which LLM providers have API keys configured.

**What's missing**: An interactive settings panel where users can change configuration at runtime. Currently all config is via `.env` / environment variables and requires a server restart.

**Effort**: Medium. Requires deciding whether settings are runtime-mutable (stored in a config file and hot-reloaded) or just a nicer view of current config. Runtime mutation adds complexity — need to persist to disk and propagate changes.

**Files to touch**:
- `templates/partials/settings.html` (new)
- `crates/barnstormer-server/src/web/mod.rs` (handlers)
- `crates/barnstormer-server/src/routes.rs` (route)
- Possibly `crates/barnstormer-server/src/config.rs` (if runtime-mutable)

---

### 1.4 Auto-Export to Disk on Events

**SPEC ref**: Section 9 — "Updated incrementally after every event"; Section 3.3 — "exports also update automatically on events"

**What exists**: `StorageManager::write_exports(spec_dir, state)` writes `spec.md`, `spec.yaml`, and `pipeline.dot` to the spec's `exports/` directory. Exports are also served on-demand via web UI download endpoints. But `write_exports` is never called at runtime — only in tests.

**What's missing**: A subscriber (or hook in the event persister) that calls `write_exports` after events are persisted.

**Effort**: Small. Add a `write_exports` call inside `spawn_event_persister` after successfully persisting events. Need to reconstruct state from the actor handle to generate exports, or pass the state along.

**Files to touch**:
- `crates/barnstormer-server/src/web/mod.rs` (`spawn_event_persister` function)

---

### 1.5 SQLite Runtime Sync

**SPEC ref**: Section 5.3 — "Used for: Fast list/search of specs and cards, Quick board rendering"

**What exists**: `SqliteIndex` with `open()`, `apply_event()`, `rebuild_from_events()`, and query methods. Used during recovery to rebuild if stale. Never written to during normal operation.

**What's missing**: Runtime event application to SQLite. The event persister could call `apply_event()` on each incoming event to keep SQLite in sync.

**Effort**: Small-Medium. Add `SqliteIndex` to the event persister loop, call `apply_event()` on each event. Need to handle the SQLite connection lifecycle (open per-spec, keep alive).

**Decision needed**: Is runtime SQLite sync actually valuable? Currently the board renders from in-memory state via the actor, not from SQLite. SQLite would only matter for cross-spec queries or if we want specs queryable without loading actors.

**Files to touch**:
- `crates/barnstormer-server/src/web/mod.rs` (`spawn_event_persister`)
- `crates/barnstormer-server/src/app_state.rs` (possibly hold SQLite handles)

---

### 1.6 Agent Context Snapshot Persistence

**SPEC ref**: Section 8.4 — "Persisted per spec: rolling summaries per agent, key decisions, last_event_seen per agent"

**What exists**: `SnapshotData` has an `agent_contexts: HashMap<String, serde_json::Value>` field. `AgentContext` implements serialization. `contexts_to_snapshot_map()` and `contexts_from_snapshot_map()` conversion functions exist. But `spawn_event_persister` always passes an empty `HashMap` when saving snapshots.

**What's missing**: Wiring. When saving a snapshot, the persister needs to read agent contexts from the swarm and include them. On recovery, the restored contexts need to be fed back into the swarm.

**Effort**: Medium. The serialization code exists — the gap is plumbing between the SwarmOrchestrator (which holds agent contexts) and the event persister (which writes snapshots). Need a way for the persister to request current contexts from the swarm.

**Files to touch**:
- `crates/barnstormer-server/src/web/mod.rs` (`spawn_event_persister` — pass swarm handle)
- `crates/barnstormer-agent/src/swarm.rs` (method to export current contexts)
- `src/main.rs` (restore contexts on recovery)

---

### 1.7 Periodic Snapshot Policy

**SPEC ref**: Section 8.4 — "write snapshot every N events OR every M minutes of activity OR on clean shutdown"

**What exists**: Snapshots are written only when the broadcast receiver detects a `Lagged` error (i.e., the event persister fell behind and missed events). This is a crash-recovery mechanism, not a periodic policy.

**What's missing**: Time-based or event-count-based snapshot triggers. Also no clean-shutdown snapshot.

**Effort**: Small. Add an event counter and/or a `tokio::time::interval` in `spawn_event_persister`. On threshold, save snapshot. For clean shutdown, handle the `SIGTERM`/drop path.

**Files to touch**:
- `crates/barnstormer-server/src/web/mod.rs` (`spawn_event_persister`)
- `src/main.rs` (graceful shutdown hook)

---

### 1.8 `ask_agent` Tool (Agent-to-Agent Communication)

**SPEC ref**: Section 8.2 — "`ask_agent(agent_id, question)` → request analysis/clarification"

**What exists**: Nothing. Agents communicate indirectly by reading shared state and writing cards/narration. No direct agent-to-agent messaging tool.

**What's missing**: A tool that lets one agent ask another agent a question and receive a response. This would require routing a message to a specific agent and blocking (or polling) for a response.

**Effort**: Large. Requires inter-agent message passing, a response mechanism, and timeout handling. The swarm's round-robin loop would need to support priority interrupts or message queues per agent.

**Decision needed**: Is this actually needed? The current indirect communication (via shared state) works. Direct agent messaging adds complexity and potential deadlocks.

**Files to touch**:
- `crates/barnstormer-agent/src/mux_tools/` (new tool module)
- `crates/barnstormer-agent/src/swarm.rs` (message routing)

---

### 1.9 `read_recent_events` Tool

**SPEC ref**: Section 8.2 — "`read_recent_events(n | since_event_id)`"

**What exists**: `read_state` returns a summary of current state. Agents see events via their context's event subscription. No tool to query historical events.

**What's missing**: A tool that returns the last N events or events since a given ID. Would let agents understand what changed recently.

**Effort**: Small-Medium. The actor already has a broadcast channel. Could subscribe and buffer recent events, or read from the JSONL log.

**Files to touch**:
- `crates/barnstormer-agent/src/mux_tools/` (new tool module)
- `crates/barnstormer-agent/src/mux_tools/mod.rs` (register tool)

---

### 1.10 `/api/specs/{id}/agents/pause` and `/resume` Endpoints

**SPEC ref**: Section 7.1 — "POST /api/specs/{id}/agents/pause / .../resume"

**What exists**: `POST /web/specs/{id}/agents/start|pause|resume` — web UI endpoints that return HTML partials. No JSON API equivalents.

**What's missing**: REST API endpoints at `/api/` that return JSON, suitable for programmatic access.

**Effort**: Small. Extract the logic from the web handlers into shared functions, add thin `/api/` handlers that return JSON.

**Files to touch**:
- `crates/barnstormer-server/src/api/` (new handlers, possibly `agents.rs`)
- `crates/barnstormer-server/src/api/mod.rs`
- `crates/barnstormer-server/src/routes.rs`

---

### 1.11 `barnstormer stop` CLI Command

**SPEC ref**: Section 10 — "`barnstormer stop`"

**What exists**: Nothing. Users kill the process manually.

**What's missing**: A CLI command that connects to the running server and requests a graceful shutdown. Would require a shutdown endpoint on the server.

**Effort**: Small-Medium. Add a `/api/shutdown` endpoint (protected by auth), and a `Stop` CLI variant that sends a POST to it.

**Files to touch**:
- `src/main.rs` (add `Stop` variant, graceful shutdown handler)
- `crates/barnstormer-server/src/routes.rs` (shutdown endpoint)

---

### 1.12 `barnstormer export <spec_id>` CLI Command

**SPEC ref**: Section 10 — "`barnstormer export <spec_id>`"

**What exists**: Exports are available via web UI. `StorageManager::write_exports()` can write to disk. No CLI command.

**What's missing**: A CLI subcommand that loads a spec from disk, materializes state, and writes exports to the spec's `exports/` directory.

**Effort**: Small. Similar pattern to the `import` subcommand — load `.env`, init StorageManager, recover spec, call `write_exports`.

**Files to touch**:
- `src/main.rs` (add `Export` variant)

---

### 1.13 Full-Text Search

**SPEC ref**: Section 5.3 — "Optional full-text search later"

**What exists**: SQLite schema with `specs` and `cards` tables. No FTS tables.

**What's missing**: SQLite FTS5 virtual tables and search queries.

**Effort**: Medium. Add FTS5 tables to the schema, populate on rebuild, and expose via a search endpoint.

**Decision needed**: SPEC marks this as "optional" and "later". Low priority unless users need it.

**Files to touch**:
- `crates/barnstormer-store/src/sqlite.rs`
- `crates/barnstormer-server/src/api/` (search endpoint)

---

### 1.14 Card `refs` Field Editing in Web UI

**SPEC ref**: Section 4.2 — "refs (array of URLs/identifiers, optional)"

**What exists**: `Card` struct has `refs: Vec<String>`. `UpdateCard` command supports `refs: Option<Vec<String>>`. The card form template has no input for refs.

**What's missing**: A text input (comma-separated or multi-line) in the card edit form for refs.

**Effort**: Small. Add an input field to `templates/partials/card_form.html` and parse it in the form handler.

**Files to touch**:
- `templates/partials/card_form.html`
- `crates/barnstormer-server/src/web/mod.rs` (form parsing)

---

## 2. Partially Implemented

### 2.1 Critic Agent

**SPEC ref**: Section 8.1 — "optional `critic` (consistency, risks, edge cases)"

**What exists**: `AgentRole::Critic` variant in the enum, system prompt defined in `swarm.rs`, can be used. Default swarm creates 4 agents (Manager, Brainstormer, Planner, DotGenerator) — Critic is excluded.

**What's needed**: Either add Critic to the default swarm, or add a way to enable it (config flag, per-spec toggle).

---

### 2.2 Export Auto-Write

**SPEC ref**: Section 9

**What exists**: `write_exports()` function, on-demand generation via web UI.

**What's needed**: Hook into event persister to call `write_exports()` after events.

---

### 2.3 Snapshot Policy

**SPEC ref**: Section 5.4, 8.4

**What exists**: Snapshots on broadcast lag only.

**What's needed**: Event-count or time-based triggers, clean-shutdown snapshot.

---

### 2.4 Agent Context in Snapshots

**SPEC ref**: Section 8.4

**What exists**: Schema supports it, serialization code exists.

**What's needed**: Plumbing to read contexts from swarm and pass to persister.

---

## 3. Implemented Differently Than Specified

### 3.1 Actor Processing Order

**SPEC says**: validate → append JSONL → apply state → update SQLite → broadcast

**Actual**: validate → apply state → broadcast. JSONL is appended asynchronously by `spawn_event_persister`. SQLite is not updated at runtime.

**Assessment**: The actual design is arguably better — it keeps the actor fast and non-blocking. The SPEC should be updated to reflect reality.

### 3.2 Crate Structure

**SPEC says**: 5 crates including `barnstormer-web` (separate from server)

**Actual**: 4 crates. Web UI lives inside `barnstormer-server` using Askama templates + HTMX.

**Assessment**: Simpler. The SPEC's separation into `barnstormer-web` was predicated on a Rust SPA framework (Leptos/Dioxus/Yew). Since we chose Askama + HTMX instead, keeping web inside server is cleaner.

### 3.3 Frontend Technology

**SPEC says**: "Front-end framework choice (Leptos/Dioxus/Yew); keep it Rust end-to-end"

**Actual**: Askama (Rust templates) + HTMX + vanilla JS (SortableJS for drag-drop, Viz.js for diagrams).

**Assessment**: Decided. Simpler stack, less Rust compile overhead, good enough for the UI needs.

### 3.4 Command Names

**SPEC says**: `AppendTranscriptMessage`, `AskUserQuestion`, `AnswerUserQuestion`, `AgentStep`, `UndoLast`

**Actual**: `AppendTranscript`, `AskQuestion`, `AnswerQuestion`, `StartAgentStep` + `FinishAgentStep`, `Undo`

**Assessment**: The actual names are cleaner. The SPEC should be updated.

### 3.5 Beyond Spec (Implemented but not in SPEC)

| Feature | Description |
|---------|-------------|
| `barnstormer import` CLI | Import specs from any text via LLM |
| `POST /api/specs/import` | HTTP API for import |
| `/web/specs/import` | Web UI import form |
| Chat messaging | Users can send freeform messages to agents (not just answer questions) |
| Agent LEDs | Visual status indicators for agent state |
| Diagram view | Dedicated DOT diagram tab with Viz.js rendering |
| Artifacts view | Combined export view with copy/download |

---

## 4. Priority Recommendations

### Do Next (small effort, high value)

1. **Auto-export to disk** — one line in event persister, fulfills a core SPEC goal
2. **Spec core editing UI** — users need to fix titles/goals without API calls
3. **Card refs editing** — small template change
4. **`barnstormer export` CLI** — mirrors import, small effort

### Do Soon (medium effort, medium value)

5. **Agent context snapshot persistence** — plumbing only, code exists
6. **Periodic snapshots** — timer + counter in event persister
7. **`/api/` agent control endpoints** — extract logic from web handlers
8. **Spec search/filter** — quality-of-life for multi-spec users

### Do Later (large effort or low priority)

9. **`ask_agent` tool** — complex, current indirect approach works
10. **`read_recent_events` tool** — nice to have
11. **Settings panel** — runtime config mutation is complex
12. **`barnstormer stop`** — graceful shutdown, requires shutdown endpoint
13. **Full-text search** — SPEC marks as optional
14. **SQLite runtime sync** — only matters if we query SQLite directly
15. **Critic agent activation** — needs design decision on when/how to enable
