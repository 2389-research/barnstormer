# barnstormer

Agentic spec builder -- an event-sourced specification management tool with a web UI.

## Names

- **AI**: Turbo Rex 9000
- **Human**: The Harp Dogfather

## Build & Run

```bash
# Build everything
cargo build --all

# Run all tests
cargo test --all

# Run with clippy
cargo clippy --all-targets -- -D warnings

# Start the server (default: http://127.0.0.1:7331)
cargo run -- start

# Start without opening browser
cargo run -- start --no-open

# Check if running
cargo run -- status

# Dev mode: auto-rebuild and restart on file changes (requires cargo-watch)
cargo watch -x 'run -- start --no-open' -w crates -w templates -w static -w src
```

## Architecture

Four crates in a Cargo workspace:

- **barnstormer-core** (`crates/barnstormer-core/`) -- Domain types, event/command definitions, state reducer, actor, and exporters (Markdown, YAML, DOT)
- **barnstormer-store** (`crates/barnstormer-store/`) -- Persistence layer: JSONL event log, snapshots, SQLite index, crash recovery, storage manager
- **barnstormer-server** (`crates/barnstormer-server/`) -- Axum HTTP API, SSE streaming, Askama+HTMX web UI, auth middleware, config
- **barnstormer-agent** (`crates/barnstormer-agent/`) -- Agent runtime, LLM provider adapters (Anthropic, OpenAI, Gemini), swarm orchestrator

Binary entrypoint: `src/main.rs`

## Key Conventions

- All code files start with a 2-line `ABOUTME:` comment describing the file's purpose
- IDs use ULID (universally unique lexicographically sortable identifiers)
- Event sourcing: all mutations go through Command -> Event -> State
- State is materialized by replaying events through a reducer
- **SSE event handling in templates**: `htmx-ext-sse@2.2.2` only subscribes to SSE event names that appear in a `hx-trigger="sse:<name>"` or `sse-swap="<name>"` attribute somewhere inside the `sse-connect` element. SSE events with no matching attribute are received by the browser's EventSource but never dispatched anywhere — they vanish. Prefer the declarative form: put `hx-trigger="sse:<name>"` + `hx-get="..."` directly on the element that should re-render. For cases that genuinely need imperative handling (appending streamed tokens, debounced re-fetches of stateful UI), the library dispatches the event as a bubbling DOM event on the element carrying the `hx-trigger` — so an `addEventListener('sse:<name>', ...)` on a parent works only if some descendant declares `hx-trigger="sse:<name>"`. To wake up an event purely for imperative consumption, add a hidden `<span hx-trigger="sse:<name>" style="display:none"></span>` inside the compositor. Examples: `templates/partials/cards_feed.html` (declarative — the `#cards-feed` wrapper re-fetches itself on card SSE events via `hx-trigger` + `hx-swap="outerHTML"`), `templates/partials/chat_transcript.html` sse-sub span (subscription-only sink).
- **`hx-target` and `hx-swap` inherit to descendants**: if you put `hx-target="#workspace"` on the `sse-connect` compositor, every descendant that doesn't override it (like `#canvas` loading the chat panel on `hx-trigger="load"`) will target `#workspace` instead of itself — swapping the whole layout out from under you. Always put workspace-level re-fetches on a dedicated hidden sentinel element inside the compositor (see `#sse-phase-sub` in `spec_view.html`), not on the compositor itself.
- Port: **7331**
- Environment config via dotenv (see `.env.example`)

## Data Flow

```
Command -> SpecActor -> Event -> SpecState (in-memory)
                           |
                           +-> JSONL log (durable)
                           +-> SQLite index (queryable cache)
                           +-> SSE broadcast (real-time)
```

## Testing

- Unit tests in each module (`#[cfg(test)]`)
- Integration smoke test in `tests/smoke.rs`
- Run all: `cargo test --all`

## Configuration

See `.env.example` for all environment variables. Key ones:

- `BARNSTORMER_HOME` -- data directory (default: `~/.barnstormer`)
- `BARNSTORMER_BIND` -- listen address (default: `127.0.0.1:7331`)
- `BARNSTORMER_AUTH_TOKEN` -- bearer token for API auth (optional)
- `BARNSTORMER_ALLOW_REMOTE` -- allow non-loopback connections (requires auth token)
