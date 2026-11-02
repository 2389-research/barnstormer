# Token Budget Cookoff Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Bound each spec's in-memory swarm session with a configurable countable-token tranche and require explicit consent before further dispatch.

**Architecture:** Parse `BARNSTORMER_TOKEN_BUDGET` once in the runtime startup configuration, store the resolved `Option<u64>` in `AppState`, and pass it to every swarm constructor. The swarm accumulates `SubAgent` usage after each step, enforces the ceiling at `run_agent_by_index`, and extends the existing transition-answer drain so the single broadcast receiver handles both transition and budget answers.

**Tech Stack:** Rust, Tokio, Axum, mux-rs `Usage`/`SubAgent`, Barnstormer's event-sourced actor, Cargo tests.

---

### Task 1: Runtime token-budget configuration

**Files:**
- Modify: `crates/barnstormer-runtime/src/config.rs`

**Step 1: Write the failing tests**

Add serialized environment tests proving that an unset variable resolves to `Some(2_000_000)`, a positive integer is honored, `0` resolves to `None`, and non-numeric or negative values return a startup error mentioning `BARNSTORMER_TOKEN_BUDGET`. Reuse `ENV_LOCK` and restore the prior environment value in every test.

**Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-runtime config::tests::token_budget -- --nocapture`

Expected: compile failure because `RuntimeConfig::token_budget` does not exist.

**Step 3: Write minimal implementation**

Add `pub token_budget: Option<u64>` to `RuntimeConfig`. In `from_parts`, parse `BARNSTORMER_TOKEN_BUDGET` as `u64`, default to `2_000_000` when absent, map zero to `None`, and return `anyhow!` with the variable name and invalid value on parse failure.

**Step 4: Run tests to verify GREEN**

Run: `cargo test -p barnstormer-runtime config::tests::token_budget -- --nocapture`

Expected: all token-budget config tests pass with pristine output.

### Task 2: Count step usage on success and failure

**Files:**
- Modify: `crates/barnstormer-agent/src/testing.rs`
- Modify: `crates/barnstormer-agent/src/swarm.rs`
- Modify: all `SwarmOrchestrator::with_agents` test call sites in `crates/barnstormer-agent/src/swarm.rs`

**Step 1: Write the failing tests**

Add a configurable-usage constructor to `StubLlmClient` under test-first coverage, then add swarm tests proving `countable_tokens` sums input, output, and cache-write while excluding cache-read; `run_agent_step` returns the expected token count on success; and a client failure after recorded mux usage returns best-effort partial usage.

**Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-agent countable_tokens -- --nocapture`

Expected: compile failure because `countable_tokens` and `StepOutcome` do not exist.

Run the focused step tests after they compile enough to execute; expected assertion failure because `run_agent_step` only returns `bool` and discards usage.

**Step 3: Write minimal implementation**

Add `StepOutcome { did_work, countable_tokens }` and pure `countable_tokens(&Usage)`. Capture `result.usage` on success and `sub_agent.usage()` after failure. Update the public return type and all direct callers.

**Step 4: Run tests to verify GREEN**

Run: `cargo test -p barnstormer-agent countable_tokens -- --nocapture`

Run: `cargo test -p barnstormer-agent run_agent_step -- --nocapture`

Expected: all focused usage tests pass.

### Task 3: Enforce the budget before every dispatch

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs`

**Step 1: Write the failing tests**

Add tests with a tiny budget proving: a completed step increments `session_tokens_used`; an unlimited budget dispatches without asking; an exhausted budget emits exactly one Boolean `QuestionAsked` while pending and does not run the client; a lost `question_pending` CAS emits no second question; and direct calls through the common dispatch choke point cover both ordinary and manager-priority dispatch semantics.

**Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-agent token_budget -- --nocapture`

Expected: compile failures for absent budget state, followed by assertion failures because exhausted swarms still dispatch.

**Step 3: Write minimal implementation**

Add `token_budget: Option<u64>`, `session_tokens_used: u64`, and `pending_budget_question: Option<Ulid>` to `SwarmOrchestrator`; accept the budget in both constructors. At the top of `run_agent_by_index`, short-circuit unlimited budgets; when exhausted, perform the existing `question_pending` false-to-true CAS, send a Boolean `AskQuestion` with default false and a human-readable tranche summary, save its ULID, and return without extracting an agent. After a dispatched step, saturating-add its countable usage under the swarm mutex.

**Step 4: Run tests to verify GREEN**

Run: `cargo test -p barnstormer-agent token_budget -- --nocapture`

Expected: all focused dispatch and one-question tests pass.

### Task 4: Handle budget answers in the shared drain

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs`

**Step 1: Write the failing tests**

Add drain tests proving matching `Yes` resets `session_tokens_used` and clears the pending budget ID; matching non-Yes pauses and clears the ID without resetting usage; non-matching and stale IDs are ignored; manual pause remains after Yes; and No followed by Resume re-asks before dispatch.

**Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-agent budget_answer -- --nocapture`

Expected: assertions fail because the existing drain only handles transition questions.

**Step 3: Write minimal implementation**

Extend `drain_transition_answers` in place. Preserve transition behavior, then match `pending_budget_question`: recognize only an exact case-insensitive `Yes`, reset usage on Yes, otherwise call the existing pause breaker, and clear the matching ID. Do not add a second receiver or manually clear `question_pending`.

**Step 4: Run tests to verify GREEN**

Run: `cargo test -p barnstormer-agent budget_answer -- --nocapture`

Expected: all focused drain tests pass and existing transition-answer tests remain green.

### Task 5: Wire runtime configuration into both swarm spawn sites

**Files:**
- Modify: `crates/barnstormer-server/src/app_state.rs`
- Modify: `crates/barnstormer-runtime/src/server.rs`
- Modify: `crates/barnstormer-server/src/web/mod.rs`
- Modify: server and smoke test constructors that call `AppState::new`

**Step 1: Write the failing tests**

Add an `AppState` construction test (or extend existing runtime lifecycle coverage) showing a resolved `Some(100)` budget reaches state and a swarm created by the web path receives it. Add a complementary unlimited case.

**Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-server token_budget -- --nocapture`

Expected: compile failure because `AppState` has no token-budget field and swarm constructors have no budget argument.

**Step 3: Write minimal implementation**

Store `token_budget` in `AppState`, provide it from `RuntimeConfig` in `build_state`, and pass it into both `SwarmOrchestrator::with_defaults` calls in `web/mod.rs`. Update test state construction explicitly, without adding hidden compatibility behavior.

**Step 4: Run tests to verify GREEN**

Run: `cargo test -p barnstormer-server token_budget -- --nocapture`

Expected: focused wiring tests pass.

### Task 6: End-to-end smoke coverage

**Files:**
- Modify: `tests/smoke.rs`
- Modify: `crates/barnstormer-runtime/tests/server_lifecycle.rs` if runtime startup is the cleaner real-server seam

**Step 1: Write the failing tests**

Add serialized startup smoke cases that boot with `BARNSTORMER_TOKEN_BUDGET=0` and with a tiny positive budget, then verify the resolved budget is wired into runtime/server state or observable swarm behavior. Use the real actor/event stream and `StubLlmClient` for budget consent flow: `QuestionAsked`, dispatch freeze, Yes reset/resume, and No pause.

**Step 2: Run tests to verify RED**

Run: `cargo test --test smoke token_budget -- --nocapture`

Expected: compile or assertion failure because the environment value is not wired and budget questions are absent.

**Step 3: Write minimal implementation/test harness support**

Expose only the smallest existing-style inspection seam needed by the smoke test; do not add mock modes to application code. Complete the real swarm + stub flow using actor commands and events.

**Step 4: Run tests to verify GREEN**

Run: `cargo test --test smoke token_budget -- --nocapture`

Expected: both unlimited and tiny-budget smoke scenarios pass with pristine output.

### Task 7: Rename-safety audit, full verification, self-review, and commit

**Files:**
- Review all changed files and all references found by searches.

**Step 1: Audit references**

Run separate searches for direct calls, type-level uses, string literals, dynamic imports, re-exports/barrels, and tests for `run_agent_step`, `with_defaults`, `with_agents`, `RuntimeConfig`, `AppState::new`, `pending_budget_question`, and `BARNSTORMER_TOKEN_BUDGET`.

**Step 2: Run canonical verification**

Run: `cargo fmt --check`

Run: `cargo clippy --all-targets -- -D warnings`

Run: `cargo test --all`

Run the real usage/E2E commands from Task 6 again with `--nocapture`.

Expected: every command exits zero with no warnings or stray error output.

**Step 3: Review every approved-design requirement**

Check counting semantics, one-step overshoot, both dispatch paths, CAS loss, answer behavior, restart-stale answer, pause/resume behavior, unlimited fast path, per-spec/session scope, both server spawn sites, config defaults/errors, and absence of out-of-scope import/summarizer changes. Present perfectionist and pragmatist views in the report.

**Step 4: Commit**

Run `git status --short`, stage only the plan and task-related files by explicit path, and commit with conventional imperative messages. Never bypass hooks.

## Execution status

Implemented config, accounting, dispatch guarding, answer handling, and server wiring. Doctor Biz approved one correction to the original design after a RED regression exposed a lifecycle deadlock: after draining `QuestionAnswered`, synchronize `question_pending` from the reducer's pending-question state. This lets a non-Yes answer pause the swarm while ensuring manual Resume re-asks before dispatch instead of remaining frozen behind a stale atomic.

Fresh-eyes follow-up added a second RED regression for manually paused swarms: the paused branch now drains the shared answer receiver before sleeping, so Yes resets the tranche and No preserves it while both remain paused and neither dispatches. Real-component smoke scenarios now carry the runtime env value through `AppState` into a mux-backed swarm, exercise round-robin overshoot/freeze/Yes continuation, and prove the human-priority path trips the same choke before No pauses.

The same review added a lagged-receiver RED regression using the actor's real 4,096-event broadcast buffer. The shared drain now logs lag, continues from the oldest retained event, and processes a retained budget answer in the same pass instead of deferring it.

The final corrective wave covers irretrievable answers that precede broadcast overflow: the drain reconciles pending IDs against the authoritative reducer question ID, safely pauses on a lost budget decision with usage preserved, and clears stale transition IDs without advancing. Completed-phase answers no longer skip atomic synchronization. Both web spawn sites now use one production `build_swarm` helper, timing-sensitive tests use explicit run-loop readiness checkpoints, and the tracked `scripts/check-token-budget` command is the scenario runbook.
