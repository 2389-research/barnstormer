# specd

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
```

## Architecture

Four crates in a Cargo workspace:

- **specd-core** (`crates/specd-core/`) -- Domain types, event/command definitions, state reducer, actor, and exporters (Markdown, YAML, DOT)
- **specd-store** (`crates/specd-store/`) -- Persistence layer: JSONL event log, snapshots, SQLite index, crash recovery, storage manager
- **specd-server** (`crates/specd-server/`) -- Axum HTTP API, SSE streaming, Askama+HTMX web UI, auth middleware, config
- **specd-agent** (`crates/specd-agent/`) -- Agent runtime, LLM provider adapters (Anthropic, OpenAI, Gemini), swarm orchestrator

Binary entrypoint: `src/main.rs`

## Key Conventions

- All code files start with a 2-line `ABOUTME:` comment describing the file's purpose
- IDs use ULID (universally unique lexicographically sortable identifiers)
- Event sourcing: all mutations go through Command -> Event -> State
- State is materialized by replaying events through a reducer
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

- `SPECD_HOME` -- data directory (default: `~/.specd`)
- `SPECD_BIND` -- listen address (default: `127.0.0.1:7331`)
- `SPECD_AUTH_TOKEN` -- bearer token for API auth (optional)
- `SPECD_ALLOW_REMOTE` -- allow non-loopback connections (requires auth token)
