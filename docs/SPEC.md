# barnstormer — Agentic Spec Builder (v1)

Date: 2026-02-10  
Status: Draft v1 implementation spec

## 0. One-liner

A local-first, web-based, agentic product-spec workstation that interrogates a user (one question at a time), continuously builds a living spec as cards + a document view, and continuously emits portable artifacts (Markdown/YAML/DOT).

## 1. Goals

1. **Agent-led spec creation**: user states “I want to build X”, the system drives discovery through constrained question tools and visible narration.
2. **Living specs**: specs evolve over time; everything is event-sourced; changes are visible and reversible.
3. **Realtime visuals**: the spec is rendered as **cards-first** (board) plus a **document view**, updating live as agents work.
4. **Multiple specs (threads)**: many specs exist concurrently, selectable from a project switcher; each spec is isolated.
5. **Portable outputs**: emit deterministic Markdown and YAML exports, and continuously-maintained DOT as the **primary build artifact**.
6. **Pluggable LLM runtime**: first-party adapters for OpenAI, Anthropic, Gemini; env-based config, base_url override support.
7. **Crash-safe recovery**: append-only JSONL event log as truth; SQLite index/cache derived; recover by replay + self-heal; persist agent context snapshots to resume after restart.

## 2. Non-goals (v1)

- Multi-user realtime collaboration (simultaneous edits by multiple humans).
- Accounts, login, hosted SaaS (v1 is local-first).
- Tagging/filtering/advanced agile metadata (keep cards minimal).
- Full “git-style branching” spec history (undo is linear).
- Deterministic agent replay test harness.

## 3. Primary UX

### 3.1 App layout (web)

Left rail:
- Spec switcher (list/search)
- Create spec
- Settings (LLM providers, storage folder, server bind, auth token)

Main:
- **Board View (default)**: lanes (default 3: Ideas / Plan / Spec) with draggable cards.
- **Document View**: single scroll narrative (generated from current state).

Right rail:
- **Agent Activity Panel** (always visible):
  - Live narration stream (per agent)
  - Diff summaries (“changed cards: …”)
  - Event log view (scrollable)
  - Agent pause/resume toggle for this spec
  - “Pending question” widget (if any)

### 3.2 Interaction contract: questions-only UI

Agents may only interact with the user using three “question tools”:
- `ask_user_boolean(question, default?)`
- `ask_user_multiple_choice(question, choices[], allow_multi=false)`
- `ask_user_freeform(question, placeholder?, validation_hint?)`

Agents may also emit “narration” messages visible to the user, but **not** as a chat that expects a response.

**Backend enforcement**:
- Exactly **one pending question per spec** at a time.
- User answers are recorded as events.
- Agents can keep working in the background, but cannot issue a new user question while one is pending.

### 3.3 Manual editing

Users can:
- Create/edit/move/delete cards (minimal fields).
- Edit core spec fields (title, one-liner, goal).
- Trigger exports (optional; exports also update automatically on events).
- Pause/resume agents per spec.

Agents can:
- Create/edit/move/delete cards.
- Edit core spec fields.
- Create “assumption” and “open question” cards when blocked (not required, but recommended).
- Override each other; manager agent can resolve conflicts automatically with rationale.

## 4. Data model (logical)

### 4.1 Spec core (required fields)

- `spec_id` (ULID or UUIDv7; sortable strongly preferred)
- `title` (required, non-empty)
- `one_liner` (required, non-empty)
- `goal` (required, non-empty)

Optional core fields (freeform):
- `description` (markdown)
- `constraints` (markdown or list)
- `success_criteria` (markdown)
- `risks` (markdown)
- `notes` (markdown)

### 4.2 Cards (minimal)

Each card is freeform but has a small stable envelope:

- `card_id`
- `type` (string; recommended: idea | plan | task | inspiration | vibes | assumption | open_question | decision)
- `title` (string)
- `body` (markdown, optional)
- `lane` (string; default “Ideas”)
- `order` (float or integer for stable ordering inside a lane)
- `refs` (array of URLs/identifiers, optional)
- `created_at`, `updated_at`
- `created_by`, `updated_by` (agent_id or “human”)

### 4.3 Transcript + bidirectional sync

The system maintains:
- A transcript-like stream of messages (agent narration, tool questions, user answers).
- A structured spec state (core fields + cards).

**Bidirectional sync invariant (v1 target)**:
- Every user answer becomes a transcript event and (optionally) a spec mutation event.
- Every spec mutation is accompanied by a “diff summary” transcript entry describing what changed.
- The transcript is always reconstructible from the event log.

(Implementation note: we don’t need perfect semantic mapping for every mutation; we do need consistent traceability.)

## 5. Persistence (v1)

### 5.1 Storage layout

Base dir: `${BARNSTORMER_HOME}` default `~/.barnstormer/`

For each spec:
```
~/.barnstormer/specs/<spec_id>/
  events.jsonl                 # append-only source of truth
  snapshots/
    state_<n>.json             # periodic snapshots (state + undo + agent context)
  index.sqlite                 # derived cache/index (rebuildable)
  exports/
    spec.md
    spec.yaml
    pipeline.dot               # PRIMARY build artifact
  attachments/                 # optional (future)
```

### 5.2 Event log (JSONL) — source of truth

- File: `events.jsonl`
- Each line: one JSON object event (newline-delimited)
- Append-only; on crash, tolerate and truncate last partial line.
- Event ids strictly increasing (monotonic) per spec.

### 5.3 SQLite index/cache (derived)

- Not authoritative.
- Used for:
  - Fast list/search of specs and cards
  - Quick board rendering (materialized views)
  - Optional full-text search later

On startup:
1. Replay JSONL into in-memory state (or start from snapshot, then apply tail).
2. Verify SQLite integrity and last_applied_event_id.
3. If mismatch/corruption: rebuild index from JSONL.

### 5.4 Undo (linear)

- UI supports single-step undo/redo-like behavior, but we only guarantee **undo** stack in v1.
- Undo is implemented as **events**, not as mutation deletion:
  - `undo_applied` references the last “undoable” mutation group and applies an inverse patch.
- Undo stack is persisted in snapshots to survive restart.

## 6. Backend architecture (Rust)

### 6.1 Process model

- One local daemon: `barnstormer`
- Clients (web) connect via HTTP + streaming (SSE or WebSocket).
- Headless mode: daemon runs without opening a browser.
- Default bind: `127.0.0.1:<port>`; remote bind must be explicitly enabled.

### 6.2 Concurrency model: Spec Actors

To avoid races:
- Each spec runs as an **actor** processing a single ordered command queue.
- All state changes happen inside the actor:
  1. validate command
  2. append event(s) to JSONL
  3. apply to in-memory state
  4. update SQLite (best-effort)
  5. publish events to subscribers

Agents run concurrently, but their write commands funnel through the spec actor.

### 6.3 Module breakdown

- `barnstormer-core`
  - state model (Spec, Card, Transcript)
  - event types + apply/reduce
  - command types + validation
  - exporters (md/yaml/dot)
  - snapshot load/save
- `barnstormer-store`
  - jsonl append/replay + truncation repair
  - sqlite indexer + rebuild
- `barnstormer-agent`
  - agent runtime trait + adapters
  - swarm orchestrator (per spec)
  - manager + subagent coordination
  - question queue enforcement
  - context snapshotting + size caps
- `barnstormer-server`
  - HTTP API + streaming
  - static web UI hosting
- `barnstormer-web`
  - Rust web UI (cards board, doc view, activity panel)

## 7. Public API (daemon)

### 7.1 HTTP (suggested)

- `GET /api/specs` → list specs (id, title, updated_at)
- `POST /api/specs` → create spec (title, one_liner, goal)
- `GET /api/specs/{id}/state` → current materialized state
- `POST /api/specs/{id}/commands` → submit command(s)
- `GET /api/specs/{id}/events/stream` → SSE stream of events (or WS at `/ws`)
- `POST /api/specs/{id}/agents/pause` / `.../resume`
- `POST /api/specs/{id}/undo` → apply undo

### 7.2 Commands (v1 minimal set)

- `CreateSpec`
- `UpdateSpecCore` (title/one_liner/goal + optional fields)
- `CreateCard`
- `UpdateCard`
- `MoveCard`
- `DeleteCard`
- `AppendTranscriptMessage`
- `AskUserQuestion` (boolean | multi | freeform)  (server validates “one pending”)
- `AnswerUserQuestion`
- `AgentStep` (groups multiple events + diff summary text)
- `UndoLast`

## 8. Agent system

### 8.1 Agents per spec (isolated)

For each spec, spawn a swarm:
- `manager` (reconciler, policy enforcer, finalizer)
- `brainstormer` (elicits intent, generates ideas/cards)
- `planner` (organizes ideas into plan-ish structure)
- `dot_generator` (keeps pipeline.dot updated)
- optional `critic` (consistency, risks, edge cases)

Isolation rules:
- Agents only see state/events for their own spec.
- No cross-spec memory or retrieval.

### 8.2 Tooling available to agents

User-facing tools (enforced):
- `ask_user_boolean`
- `ask_user_multiple_choice`
- `ask_user_freeform`

Internal tools:
- `ask_agent(agent_id, question)` → request analysis/clarification
- `read_state()` → condensed state snapshot for prompting
- `read_recent_events(n | since_event_id)`
- `write_commands(commands[])` → propose mutations (executed through actor)
- `emit_narration(text)`
- `emit_diff_summary(text)` (or backend auto-generates)

### 8.3 Background operation + question queue

Agents may run in background:
- continue drafting cards, reorganizing, exporting
- but cannot issue a new user question while one is pending
- manager may choose to proceed using assumptions

### 8.4 Context snapshots

Persisted per spec:
- rolling summaries per agent (bounded size)
- key decisions + rationales
- last_event_seen per agent

Snapshot policy (suggested):
- write snapshot every N events OR every M minutes of activity OR on clean shutdown
- cap each agent summary to max tokens/bytes; keep a rolling window and a “long-term summary”

## 9. Exporters

### 9.1 Markdown export (`exports/spec.md`)

Deterministic ordering:
- Spec core header
- Board lanes in stable order (Ideas, Plan, Spec first; then alpha)
- Cards in each lane by `order`, then `card_id`

Include:
- Core fields
- Cards grouped by lane and type
- “Recent changes” section (derived from last K events) (optional)

### 9.2 YAML export (`exports/spec.yaml`)

Same data as Markdown but structured.

### 9.3 DOT export (`exports/pipeline.dot`) — PRIMARY

- Updated incrementally after every event (best-effort; if exporter fails, retry next tick).
- Must conform to the DOT Runner constrained runtime DSL:
  - `digraph <id> { ... }`
  - graph attrs only inside `graph [ ... ]`
  - key=value attrs (no `:`)
  - exactly one `start` (Mdiamond) and one terminal (`done` Msquare)
  - use outcome conditions: `condition="outcome=SUCCESS"` / `condition="outcome=FAIL"`
  - node ids snake_case
  - limited shapes: box, diamond, hexagon (type="wait.human"), parallelogram (command=), Mdiamond, Msquare

## 10. CLI

- `barnstormer start`:
  - start daemon (or connect to existing)
  - initialize store
  - open browser to web UI (unless `--no-open`)
- `barnstormer status`
- `barnstormer stop` (optional; can be best-effort)
- `barnstormer export <spec_id>` (optional; mostly redundant since auto-export)

## 11. Configuration (.env)

Examples:
- `BARNSTORMER_HOME=~/.barnstormer`
- `BARNSTORMER_BIND=127.0.0.1:7331`
- `BARNSTORMER_PUBLIC_BASE_URL=http://localhost:7331`
- `BARNSTORMER_ALLOW_REMOTE=false`
- `BARNSTORMER_AUTH_TOKEN=...` (required if remote bind enabled)

LLM providers (examples):
- `OPENAI_API_KEY=...`
- `OPENAI_BASE_URL=https://api.openai.com/v1`
- `ANTHROPIC_API_KEY=...`
- `ANTHROPIC_BASE_URL=...`
- `GEMINI_API_KEY=...`
- `GEMINI_BASE_URL=...`
- `BARNSTORMER_DEFAULT_PROVIDER=openai|anthropic|gemini`
- `BARNSTORMER_DEFAULT_MODEL=...`

## 12. Error handling & resilience

- JSONL parse: skip/truncate last partial line; refuse to run if mid-file corruption (unless repairable).
- SQLite corruption: rebuild from JSONL.
- Export failures: keep last-good exports; mark exporter dirty; retry.
- Provider failures: agents degrade gracefully (pause themselves, emit narration, create “blocked” cards); never lose spec state.

## 13. Milestones (v1 build order)

1. Event store + state reducer + snapshots
2. HTTP API + SSE/WS event stream
3. Minimal web UI: spec switcher + board view + activity panel
4. Manual card edits + core fields editing
5. Agent runtime trait + one provider adapter
6. Swarm orchestration + question queue enforcement
7. Exporters: md/yaml/dot (dot incremental)
8. Crash recovery + sqlite index + self-heal
9. Multi-provider adapters (OpenAI/Anthropic/Gemini)
10. Headless mode + remote bind safety guardrails

## 14. Open design choices (explicit)

- SSE vs WebSocket for streaming (either acceptable; SSE is simpler, WS is bidirectional).
- ULID vs UUIDv7 (both sortable; pick one).
- Front-end framework choice (Leptos/Dioxus/Yew); keep it Rust end-to-end.

---

## Appendix A — Minimal event types (suggested)

- `spec_created`
- `spec_core_updated`
- `card_created`
- `card_updated`
- `card_moved`
- `card_deleted`
- `transcript_appended`
- `question_asked`
- `question_answered`
- `agent_step_started`
- `agent_step_finished` (diff summary)
- `undo_applied`
- `snapshot_written` (optional marker)

## Appendix B — Example YAML export shape (illustrative)

See `spec.yaml` artifact.

