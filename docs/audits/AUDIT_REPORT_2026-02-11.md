# Documentation Audit Report

Generated: 2026-02-11 | Commit: 383a988

## Executive Summary

| Metric | Count |
|--------|-------|
| Documents scanned | 4 (README.md, CLAUDE.md, SPEC.md, .env.example) |
| Claims verified | ~45 |
| Verified TRUE | 18 (40%) |
| **Verified FALSE** | **22 (49%)** |
| Uncertain | 5 (11%) |

The documentation has significant drift from the codebase, primarily due to:
1. **New features not documented** (import CLI/API/web UI)
2. **Stale counts** (test count, file count)
3. **SPEC.md design vs reality** (several design aspirations that differ from implementation)

---

## False Claims Requiring Fixes

### README.md

| Line | Claim | Reality | Severity | Fix |
|------|-------|---------|----------|-----|
| 143 | "293 tests across 34 files" | 351 tests across 36 files | Medium | Update to current count |
| 65 | Critic listed as running agent | Critic exists in enum but NOT in default swarm (only 4 agents run: Manager, Brainstormer, Planner, DotGenerator) | High | Clarify Critic is optional/available but not in default swarm |
| 99 | `BARNSTORMER_PUBLIC_BASE_URL` default `http://localhost:7331` | Default is dynamically computed from bind address: `format!("http://{}", bind)` | Low | Update to say "derived from BARNSTORMER_BIND" |
| 164 | Card types: "idea, task, constraint, risk, note" | card_type is freeform string; actual usage includes: idea, task, plan, decision, constraint, risk. No "note" type exists. | Medium | Update to match actual usage |
| 91-107 | Configuration table missing base URL env vars | `OPENAI_BASE_URL`, `ANTHROPIC_BASE_URL`, `GEMINI_BASE_URL` exist in .env.example and code but not in README table | Medium | Add to config table |
| 121-131 | API endpoints table missing import | `POST /api/specs/import` exists but not listed | High | Add import endpoint |
| 14-23 | CLI commands missing import | `barnstormer import` subcommand exists but not listed | High | Add import command to Quick Start |
| 153-191 | Project structure missing import.rs | `barnstormer-agent/src/import.rs` exists but not in tree | Low | Add to structure |

### CLAUDE.md

| Line | Claim | Reality | Severity | Fix |
|------|-------|---------|----------|-----|
| - | No mention of import feature | Import CLI, API, and web UI all exist | Medium | Add import to Key Conventions or Build & Run |

### SPEC.md

| Line | Claim | Reality | Severity | Fix |
|------|-------|---------|----------|-----|
| 182-186 | Actor processing order: "1. validate, 2. append JSONL, 3. apply state, 4. update SQLite, 5. broadcast" | Actual: 1. validate+emit events, 2. apply to state, 3. broadcast. JSONL append is done by background event_persister, NOT inside actor. SQLite is NOT actively synced. | High | Update to match actual architecture |
| 209 | Lists `barnstormer-web` crate | This crate does not exist. Web UI is in barnstormer-server. | Medium | Remove or mark as not implemented |
| 220 | SSE stream at `/ws` | No WebSocket endpoint exists, only SSE at `/api/specs/{id}/events/stream` | Low | Remove WS reference |
| 221-222 | `POST /api/specs/{id}/agents/pause` and `.../resume` | These exist at `/web/specs/{id}/agents/pause|resume`, NOT `/api/specs/` | Medium | Update paths |
| 232-236 | Command names: `AppendTranscriptMessage`, `AskUserQuestion`, `AnswerUserQuestion`, `AgentStep`, `UndoLast` | Actual: `AppendTranscript`, `AskQuestion`, `AnswerQuestion`, `StartAgentStep` + `FinishAgentStep`, `Undo` | Medium | Update to match code |
| 261-263 | Internal tools: `ask_agent`, `read_recent_events` | Neither exists. Actual tools: read_state, write_commands, emit_narration, emit_diff_summary, ask_user (x3) | Medium | Update to match actual tools |
| 316-324 | CLI: `barnstormer stop`, `barnstormer export <spec_id>` | Neither exists. Actual: start, status, import | Medium | Update to match actual CLI |
| 35 | Settings panel in left rail | No settings panel exists. Left rail has: spec list, provider status, new spec button, import button | Low | Update to match actual UI |
| 369 | "Front-end framework choice (Leptos/Dioxus/Yew); keep it Rust end-to-end" | Actual implementation uses Askama templates + HTMX, not a Rust SPA framework | Low | Mark as decided/resolved |

---

## Pattern Summary

| Pattern | Count | Root Cause |
|---------|-------|------------|
| New features not documented | 3 (CLI import, API import, web import) | Import feature added without updating docs |
| SPEC.md design vs implementation | 8 | SPEC is a design document; implementation diverged on details |
| Stale counts/lists | 3 (test count, file tree, card types) | Docs not updated as codebase grew |
| Command name mismatches | 1 (5 command names wrong) | SPEC used placeholder names |
| Missing config documentation | 1 (base URL env vars) | .env.example updated but not README table |

---

## Verified TRUE Claims

| Document | Claim | Status |
|----------|-------|--------|
| README | Architecture: 4 crates in workspace | TRUE |
| README | Binary entrypoint: src/main.rs | TRUE |
| README | Data flow: Command -> Actor -> Event -> State | TRUE |
| README | Agent roles enum includes all 5 (Manager, Brainstormer, Planner, DotGenerator, Critic) | TRUE |
| README | 7 agent tools | TRUE |
| README | Web UI: Askama + HTMX + SSE | TRUE |
| README | Port 7331 default | TRUE |
| README | BARNSTORMER_HOME default ~/.barnstormer | TRUE |
| README | ULID for IDs | TRUE |
| README | Broadcast channel capacity 4096 | TRUE |
| README | Diagram rendered with Viz.js | TRUE (v3.11.0) |
| README | All ABOUTME comment convention | TRUE |
| .env.example | All listed env vars exist in code | TRUE |
| SPEC.md | Snapshot filename state_N.json | TRUE |
| SPEC.md | Event types in Appendix A | TRUE |
| SPEC.md | Question queue one-pending-per-spec | TRUE |
| SPEC.md | BARNSTORMER_ALLOW_REMOTE enforced with auth | TRUE |
| SPEC.md | BARNSTORMER_DEFAULT_PROVIDER/MODEL used | TRUE |

---

## Human Review Queue

- [ ] **Critic agent**: README describes it as a running agent, but it's not in the default swarm. Decide: should docs say "5 available roles, 4 active by default" or should Critic be added to default swarm?
- [ ] **SPEC.md overall**: This is a design spec, not a living doc. Several sections describe aspirational features (stop command, export command, settings panel, ask_agent tool). Decide: update SPEC to match reality, or leave as aspirational design doc with a "status" column?
- [ ] **Actor processing order**: SPEC.md describes synchronous JSONL write inside the actor. Actual architecture uses async event_persister. This is architecturally significant — update SPEC to document the actual (and arguably better) design.
