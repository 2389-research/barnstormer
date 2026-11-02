# Token Budget Cookoff Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bound each in-memory swarm session by a configurable countable-token tranche and require explicit user consent before spending another tranche.

**Architecture:** Count input, output, and cache-write usage at the existing `run_agent_step` boundary, accumulate it at the `run_agent_by_index` dispatch choke point, and create a normal boolean user question before any over-budget dispatch. Extend the existing shared answer drain so a Yes resets the tranche while every other matching answer pauses the swarm. Parse the environment setting in runtime configuration and carry it through shared server state to both swarm construction sites.

**Tech Stack:** Rust, Tokio, mux-rs `Usage`/`SubAgent`, Barnstormer event-sourced commands, Axum, Cargo tests.

---

### Task 1: Represent and count per-step usage

**Files:**
- Modify: `crates/barnstormer-agent/src/testing.rs`
- Modify: `crates/barnstormer-agent/src/swarm.rs`

**Step 1: Write failing tests**

- Add a stub-client test proving a configured `Usage` is returned.
- Add a `countable_tokens` unit test with nonzero input, output, cache-read, and cache-write values; expect cache-read to be excluded.
- Change the existing `run_agent_step_completes_with_stub` test to expect `StepOutcome { did_work: false, countable_tokens: ... }` from nonzero stub usage.

**Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-agent countable_tokens_excludes_cache_reads -- --exact --nocapture`

Expected: FAIL because `countable_tokens` does not exist.

Run: `cargo test -p barnstormer-agent run_agent_step_completes_with_stub -- --exact --nocapture`

Expected: compile FAIL because `run_agent_step` still returns `bool` and the usage-configurable stub API does not exist.

**Step 3: Write minimal implementation**

- Add `StubLlmClient::with_usage(response_text, usage)` while retaining `new`/`done` as zero-usage conveniences.
- Add public `StepOutcome { did_work: bool, countable_tokens: u64 }`.
- Add pure `countable_tokens(&Usage) -> u64` using widened `u64` additions.
- Return the result usage on success and `sub_agent.usage()` on error so partial usage is preserved.

**Step 4: Run tests to verify GREEN**

Run the two focused tests above and the `testing` module tests; expect PASS with clean output.

**Step 5: Commit checkpoint**

After the next dispatch task is green, commit the cohesive agent accounting behavior together.

### Task 2: Enforce the budget at dispatch and process answers

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs`

**Step 1: Write failing tests**

- Construct tiny-budget swarms and prove the first crossing step is counted.
- Prove an already-exhausted budget blocks dispatch and asks exactly one `UserQuestion::Boolean` while pending.
- Prove `None` (unlimited) never asks and still dispatches.
- Exercise both `run_loop` dispatch paths: normal round-robin and human-message manager priority.
- Extend drain tests so matching Yes resets `session_tokens_used` and clears `pending_budget_question`; matching non-Yes pauses and clears it; nonmatching/stale answers do nothing.
- Prove a lost `question_pending` CAS produces no budget question and leaves the exhausted counter intact for retry.

**Step 2: Run tests to verify RED**

Run each new focused test with `cargo test -p barnstormer-agent <test-name> -- --exact --nocapture`.

Expected: compile failures for missing constructor budget argument/fields or assertion failures because dispatch is not gated.

**Step 3: Write minimal implementation**

- Add `token_budget: Option<u64>`, `session_tokens_used: u64`, and `pending_budget_question: Option<Ulid>` to `SwarmOrchestrator`.
- Add the budget argument to `with_defaults` and `with_agents`; update every direct caller found by rename-safety searches.
- In `run_agent_by_index`, refresh the authoritative question state, then before any LLM dispatch short-circuit unlimited; when exhausted, CAS `question_pending`, send `Command::AskQuestion` with default false and tranche-aware wording, store the ID only after success, and restore the untouched agent runner.
- After a dispatched step, use saturating addition to accumulate its countable usage before restoring the runner.
- Extend the existing `drain_transition_answers` loop rather than adding a competing receiver drain. Match transition and budget IDs independently; reset on case-insensitive Yes, otherwise pause, then synchronize the atomic question gate from actor state.
- Preserve counter value after No so manual Resume requires another consent question.

**Step 4: Run tests to verify GREEN**

Run all focused swarm tests, then `cargo test -p barnstormer-agent`; expect PASS and no warnings/errors.

**Step 5: Commit**

Run `git status --short`, stage only the plan and agent files, and commit: `feat: enforce swarm token budgets`.

### Task 3: Parse runtime configuration and wire both spawn paths

**Files:**
- Modify: `crates/barnstormer-runtime/src/config.rs`
- Modify: `crates/barnstormer-runtime/src/server.rs`
- Modify: `crates/barnstormer-server/src/app_state.rs`
- Modify: `crates/barnstormer-server/src/web/mod.rs`
- Modify: direct `AppState`/`SwarmOrchestrator` construction call sites only as required by the signature change

**Step 1: Write failing tests**

- Under the existing environment mutex, test unset defaults to `Some(2_000_000)`, a positive integer is honored, `0` becomes `None`, nonnumeric is an error, and negative is an error.
- Add server-state tests proving the configured budget is retained and handed to a newly constructed swarm on both the explicit-start and auto-start paths where feasible without a real provider.

**Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-runtime token_budget -- --nocapture`

Expected: compile FAIL because `RuntimeConfig::token_budget` does not exist.

Run focused server tests; expected compile/assertion FAIL because `AppState` has no token budget and constructors do not forward it.

**Step 3: Write minimal implementation**

- Parse `BARNSTORMER_TOKEN_BUDGET` in `RuntimeConfig::from_parts`: absent is `Some(2_000_000)`, `0` is `None`, and `u64` parse errors include the variable name and bad value.
- Store `token_budget: Option<u64>` in `RuntimeConfig` and `AppState`.
- Update runtime `build_state` to construct `AppState` with that value.
- Pass `state.token_budget` into both `SwarmOrchestrator::with_defaults` calls in `web/mod.rs`.
- Update direct constructors with the approved default or explicit test value, without adding backward-compatibility APIs.

**Step 4: Run tests to verify GREEN**

Run runtime and server focused tests, then their full crate suites; expect PASS with pristine output.

**Step 5: Commit**

Run `git status --short`, stage only relevant files, and commit: `feat: configure swarm token budget`.

### Task 4: Integration and end-to-end behavior

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs` test module or create a focused integration test only if public APIs suffice
- Modify: `tests/smoke.rs` or runtime integration tests, choosing the smallest test surface that boots the real server path

**Step 1: Write failing tests**

- Run a real `SwarmOrchestrator`/`run_loop` with `StubLlmClient` usage exceeding 100 tokens; observe `QuestionAsked`, pending gating, Yes reset/resume, and No pause through real actor commands/events.
- Boot the runtime server twice under serialized environment mutation: once with `BARNSTORMER_TOKEN_BUDGET=0`, once with a tiny positive budget. Verify startup succeeds and shared state/swarm wiring uses the resolved value through observable behavior.

**Step 2: Run tests to verify RED**

Run each integration/E2E test by exact name; expect failure because the full wiring or behavior is absent.

**Step 3: Write minimal implementation**

Only add production changes exposed by the integration failures. Do not create mock modes or bypass the real command/event flow.

**Step 4: Run tests to verify GREEN**

Re-run exact integration/E2E tests; expect PASS and clean output.

**Step 5: Commit**

Run `git status --short`, stage the specific test and implementation files, and commit: `test: cover token budget lifecycle`.

### Task 5: Rename-safety, verification, and review

**Files:**
- Review all changed files and every caller/test located by searches.

**Step 1: Search all references separately**

Run separate `rg` searches for direct calls, imported/re-exported names, string literals, constructors, and tests involving `run_agent_step`, `run_agent_by_index`, `SwarmOrchestrator`, `AppState::new`, `RuntimeConfig`, and `BARNSTORMER_TOKEN_BUDGET`.

**Step 2: Run formatting and canonical verification**

Run:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

Expected: all exit 0 with pristine output.

**Step 3: Run real usage/E2E verification**

Run the exact runtime/server smoke commands established in Task 4 with unlimited and tiny budgets; expect successful startup and the asserted budget behavior.

**Step 4: Self-review the approved design**

Check every counting, tripping, answering, configuration, edge-case, unit, integration, and E2E requirement. Present both perfectionist criticisms and pragmatic acceptance criteria; fix every concrete defect before completion.

**Step 5: Final commit**

Run `git status --short`, stage only remaining intended files (never `git add -A`), and create a conventional final commit if needed. Confirm the worktree is clean and report commit hashes, exact verification results, changed-file/line counts, concerns, and every observed RED failure.

### Fresh-Eyes Follow-up

- Add a recovered-state regression proving authoritative context refresh cannot dispatch an LLM while a persisted question is pending.
- Re-check the question gate after refresh and before budget checking or LLM dispatch.
- Route both web spawn sites through one production factory and test that it forwards `AppState.token_budget`; keep real-component consent and dispatch-path scenarios in the agent crate with deterministic call-count synchronization.
- Keep paused swarms draining answers without allowing agent dispatch.
