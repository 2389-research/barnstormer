# Barnstormer

**Agentic spec builder** — an event-sourced specification management tool with an AI agent swarm and a real-time web UI.

barnstormer helps you build software specifications collaboratively with AI. You describe what you want to build, and a swarm of specialized agents brainstorm ideas, organize plans, identify risks, and generate architecture diagrams — all in real time through an interactive web interface.

## Quick Start

```bash
# Build
cargo build --all

# Configure at least one LLM provider
cp .env.example .env
# Edit .env and set ANTHROPIC_API_KEY, OPENAI_API_KEY, or GEMINI_API_KEY

# Start the server (opens browser automatically)
cargo run -- start

# Or start without opening a browser
cargo run -- start --no-open

# Check if barnstormer is running
cargo run -- status
```

The server runs at [http://127.0.0.1:7331](http://127.0.0.1:7331) by default.

## Architecture

Four crates in a Cargo workspace, plus a binary entrypoint:

| Crate | Path | Purpose |
|-------|------|---------|
| **barnstormer-core** | `crates/barnstormer-core/` | Domain types, commands, events, state reducer, actor, exporters (Markdown, YAML, DOT) |
| **barnstormer-store** | `crates/barnstormer-store/` | Persistence: JSONL event log, snapshots, SQLite index, crash recovery |
| **barnstormer-server** | `crates/barnstormer-server/` | Axum HTTP API, SSE streaming, Askama+HTMX web UI, auth middleware |
| **barnstormer-agent** | `crates/barnstormer-agent/` | Agent runtime, LLM provider adapters (Anthropic, OpenAI, Gemini), swarm orchestrator |

Binary entrypoint: `src/main.rs`

### Data Flow

All mutations flow through event sourcing:

```
Command → SpecActor → Event → SpecState (in-memory)
                         │
                         ├─→ JSONL log (durable)
                         ├─→ SQLite index (queryable cache)
                         └─→ SSE broadcast (real-time UI)
```

State is materialized by replaying events through a reducer. The JSONL log is the source of truth; SQLite serves as a queryable cache. On startup, barnstormer recovers all specs from persisted events.

## Agent Swarm

The `SwarmOrchestrator` runs a team of specialized AI agents that collaborate on your spec. Each agent has its own event receiver and a focused role:

| Role | Description |
|------|-------------|
| **Manager** | Primary point of contact. Parses your description into structured fields, creates initial cards, asks clarifying questions, and coordinates the other agents. Prioritizes responding to human messages. |
| **Brainstormer** | Generates creative ideas and explores possibilities. Creates idea cards with breadth-first exploration and narrates its thought process. |
| **Planner** | Organizes ideas into structured, actionable plans. Moves promising ideas to the Plan lane, creates task cards, and updates constraints and success criteria. |
| **DotGenerator** | Analyzes spec structure and card relationships. Identifies gaps (ideas without plans, plans without tasks), suggests structural improvements, and summarizes pipeline health. Does not create cards. |
| **Critic** | Reviews the spec for gaps, inconsistencies, and risks. Creates risk and constraint cards and asks users about ambiguities. |

Agents communicate through 7 tools:
- **read_state** — Read current spec state summary
- **write_commands** — Submit spec-mutating commands (create/update/move/delete cards, update spec core)
- **emit_narration** — Post reasoning to the activity feed
- **emit_diff_summary** — Mark a step as finished with a change summary
- **ask_user_boolean** / **ask_user_multiple_choice** / **ask_user_freeform** — Ask the user questions (CAS-protected to prevent concurrent questions)

## Web UI

The UI is built with Askama templates, HTMX, and SSE for real-time updates without full page reloads.

**Layout:**
- **Nav rail** (left) — Spec list, provider status, new spec button
- **Command bar** (top) — Spec title, view toggles, agent controls (start/pause/resume), undo
- **Canvas** (center) — Swappable views:
  - **Document** — Auto-generated markdown from spec data
  - **Board** — Kanban-style drag-and-drop lanes with SortableJS
  - **Diagram** — DOT graph rendered with Viz.js
- **Chat rail** (right) — Conversation transcript, question cards, and message input
- **Agent LEDs** — Colored status indicators showing which agents are running, paused, or stopped

SSE events (card changes, transcript updates, agent status) trigger HTMX partial re-renders to keep the UI in sync.

## Configuration

Copy `.env.example` to `.env` and configure:

| Variable | Default | Description |
|----------|---------|-------------|
| `BARNSTORMER_HOME` | `~/.barnstormer` | Data directory for event logs, snapshots, and SQLite index |
| `BARNSTORMER_BIND` | `127.0.0.1:7331` | Listen address |
| `BARNSTORMER_PUBLIC_BASE_URL` | `http://localhost:7331` | Public base URL |
| `BARNSTORMER_AUTH_TOKEN` | *(none)* | Bearer token for API auth (optional, enables auth middleware) |
| `BARNSTORMER_ALLOW_REMOTE` | `false` | Allow non-loopback connections (requires auth token) |
| `BARNSTORMER_DEFAULT_PROVIDER` | *(auto-detect)* | LLM provider: `anthropic`, `openai`, or `gemini` |
| `BARNSTORMER_DEFAULT_MODEL` | *(provider default)* | Model override (e.g. `claude-sonnet-4-5-20250929`) |
| `ANTHROPIC_API_KEY` | — | Anthropic API key |
| `OPENAI_API_KEY` | — | OpenAI API key |
| `GEMINI_API_KEY` | — | Gemini API key |

## Exports

Specs can be exported in three formats:

- **Markdown** — Human-readable document with spec details and cards organized by lane
- **YAML** — Structured data export of the full spec state
- **DOT** — Graphviz diagram source showing card relationships and flow

Export via the web UI (`/web/specs/{id}/export/markdown|yaml|dot`) or the API.

## API

### REST Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `GET` | `/health` | Health check |
| `GET` | `/api/specs` | List all specs |
| `POST` | `/api/specs` | Create a new spec |
| `GET` | `/api/specs/{id}/state` | Get full spec state |
| `POST` | `/api/specs/{id}/commands` | Submit commands |
| `POST` | `/api/specs/{id}/undo` | Undo last command |
| `GET` | `/api/specs/{id}/events/stream` | SSE event stream |

When `BARNSTORMER_AUTH_TOKEN` is set, API routes require `Authorization: Bearer <token>`.

### SSE Events

Subscribe to `/api/specs/{id}/events/stream` for real-time updates:

`spec_created`, `spec_core_updated`, `card_created`, `card_updated`, `card_moved`, `card_deleted`, `transcript_appended`, `question_asked`, `question_answered`, `agent_step_started`, `agent_step_finished`, `undo_applied`, `snapshot_written`

## Testing

```bash
# Run all tests (293 tests across 34 files)
cargo test --all

# Run with clippy
cargo clippy --all-targets -- -D warnings
```

Tests cover domain logic, persistence, crash recovery, API routes, auth middleware, SSE streaming, agent tools, swarm orchestration, and an integration smoke test (`tests/smoke.rs`).

## Project Structure

```
barnstormer/
├── src/main.rs                    # Binary entrypoint (CLI, server startup)
├── crates/
│   ├── barnstormer-core/          # Domain types, events, commands, state, exporters
│   │   └── src/
│   │       ├── actor.rs           # SpecActor (command processing, event broadcast)
│   │       ├── command.rs         # Command definitions (tagged enum)
│   │       ├── event.rs           # Event definitions and payload types
│   │       ├── state.rs           # SpecState reducer
│   │       ├── card.rs            # Card model (idea, task, constraint, risk, note)
│   │       ├── transcript.rs      # Transcript entries
│   │       └── export/            # Markdown, YAML, DOT exporters
│   ├── barnstormer-store/         # Persistence layer
│   │   └── src/
│   │       ├── jsonl.rs           # JSONL event log
│   │       ├── snapshot.rs        # State snapshots
│   │       ├── sqlite.rs          # SQLite index
│   │       ├── recovery.rs        # Crash recovery
│   │       └── manager.rs         # StorageManager orchestration
│   ├── barnstormer-server/        # HTTP server and web UI
│   │   └── src/
│   │       ├── routes.rs          # Route definitions
│   │       ├── web/               # Web UI handlers
│   │       ├── api/               # JSON API handlers
│   │       ├── auth.rs            # Bearer token middleware
│   │       └── config.rs          # Server configuration
│   └── barnstormer-agent/         # AI agent system
│       └── src/
│           ├── swarm.rs           # SwarmOrchestrator (agent lifecycle, round-robin)
│           ├── context.rs         # AgentRole enum, per-agent context
│           ├── client.rs          # LLM provider adapters
│           └── mux_tools/         # 7 agent tools (read, write, narrate, ask)
├── static/                        # CSS, JS (board.js, style.css)
├── templates/                     # Askama HTML templates
├── tests/smoke.rs                 # Integration smoke test
└── .env.example                   # Environment variable template
```

## Conventions

- All code files start with a 2-line `ABOUTME:` comment describing the file's purpose
- IDs use **ULID** (universally unique, lexicographically sortable)
- All mutations go through event sourcing: Command → Event → State
- Commands use `#[serde(tag = "type")]` — agents must produce `{"type": "CreateCard", ...}`
- Broadcast channel (4096 capacity) for event distribution
- Question handling uses CAS (compare-and-swap) to prevent concurrent agent questions
