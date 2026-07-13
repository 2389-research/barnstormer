# Swarm Token Budget Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Bound each in-memory swarm session by a configurable countable-token tranche and require explicit user consent before further dispatch.

**Architecture:** Runtime configuration parses `BARNSTORMER_TOKEN_BUDGET` into `Option<u64>` and carries it through `AppState` to each per-spec `SwarmOrchestrator`. The orchestrator accumulates `input + output + cache_write` usage after each step, checks the ceiling at the single dispatch choke point, asks one existing boolean user question when over budget, and drains budget answers alongside transition answers.

**Tech Stack:** Rust, Tokio, Axum, mux-rs `Usage`, Barnstormer's event-sourced actor and existing `StubLlmClient`.

## Global Constraints

- The approved design in `docs/plans/2026-07-06-token-budget-design.md` is binding.
- Default tranche is exactly `2_000_000` countable tokens; env value `0` means unlimited; malformed or negative values fail startup.
- Cache reads are excluded; input, output, and cache-write tokens are included.
- Budget state is per spec, per process session, in memory only.
- Every behavior follows RED → GREEN; all Rust code retains two leading `ABOUTME:` lines.
- No backward-compatibility constructor is added; every existing constructor caller is updated to the new required budget argument.

---

### Task 1: Runtime configuration and wiring

**Files:**
- Modify: `crates/barnstormer-runtime/src/config.rs`
- Modify: `crates/barnstormer-runtime/src/server.rs`
- Modify: `crates/barnstormer-runtime/tests/server_lifecycle.rs`
- Modify: `crates/barnstormer-server/src/app_state.rs`
- Modify: `crates/barnstormer-server/src/web/mod.rs`
- Modify: all `AppState::new` callers found by rename-safety search

**Interfaces:**
- Produces: `RuntimeConfig::token_budget: Option<u64>` and `AppState::token_budget: Option<u64>`.
- Consumes: `BARNSTORMER_TOKEN_BUDGET`; `None` means unlimited.

- [x] **Step 1: Write failing config tests**

Add serialized env tests that remove/set/restore `BARNSTORMER_TOKEN_BUDGET` and assert:

```rust
assert_eq!(config.token_budget, Some(2_000_000));
assert_eq!(configured.token_budget, Some(123_456));
assert_eq!(unlimited.token_budget, None);
assert!(invalid.unwrap_err().to_string().contains("BARNSTORMER_TOKEN_BUDGET"));
```

Add server-lifecycle cases that launch with `0`, launch with a tiny numeric value, and reject an invalid value before binding.

- [x] **Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-runtime config -- --nocapture`

Expected: compile failure because `RuntimeConfig` has no `token_budget` field.

- [x] **Step 3: Implement the minimal parser and state wiring**

Parse the env value once in the runtime path:

```rust
let token_budget = match std::env::var("BARNSTORMER_TOKEN_BUDGET") {
    Ok(value) => match value.parse::<u64>() {
        Ok(0) => None,
        Ok(value) => Some(value),
        Err(err) => return Err(anyhow::anyhow!(
            "BARNSTORMER_TOKEN_BUDGET is not a valid non-negative integer ({value}): {err}"
        )),
    },
    Err(std::env::VarError::NotPresent) => Some(2_000_000),
    Err(err) => return Err(err.into()),
};
```

Carry that value into `AppState::new(home, provider_status, token_budget)` and pass `state.token_budget` to both `SwarmOrchestrator::with_defaults` calls. Update every constructor caller found by separate direct/type/string/test searches.

- [x] **Step 4: Run focused tests GREEN**

Run: `cargo test -p barnstormer-runtime -- --nocapture`

Expected: all runtime unit and lifecycle tests pass with pristine output.

- [x] **Step 5: Commit**

Run `git status --short`, stage only Task 1 paths, then `git commit -m "feat: configure swarm token budget"`.

### Task 2: Usage accounting and step outcome

**Files:**
- Modify: `crates/barnstormer-agent/src/testing.rs`
- Modify: `crates/barnstormer-agent/src/swarm.rs`

**Interfaces:**
- Produces: `pub struct StepOutcome { pub did_work: bool, pub countable_tokens: u64 }` and `pub fn countable_tokens(usage: &Usage) -> u64`.
- Consumes: mux `Usage` and existing `SubAgent::usage()` for best-effort error usage.

- [x] **Step 1: Write failing accounting tests**

Add a pure test proving cache reads are excluded and a stub-driven step test proving returned usage is counted:

```rust
let usage = Usage { input_tokens: 11, output_tokens: 7,
    cache_read_tokens: 999, cache_write_tokens: 13 };
assert_eq!(countable_tokens(&usage), 31);
```

Extend `StubLlmClient` with a usage-bearing constructor so the real mux runner returns nonzero usage without mocking orchestrator behavior. Add an error client test that proves `run_agent_step` reads partial accumulated usage from `SubAgent::usage()` after a later error.

- [x] **Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-agent countable_tokens -- --nocapture`

Expected: compile failure because `countable_tokens` and `StepOutcome` do not exist.

- [x] **Step 3: Implement the minimal outcome**

Use saturating conversion/addition:

```rust
pub fn countable_tokens(usage: &Usage) -> u64 {
    u64::from(usage.input_tokens)
        .saturating_add(u64::from(usage.output_tokens))
        .saturating_add(u64::from(usage.cache_write_tokens))
}
```

Return `StepOutcome` on success and error, taking error-path usage from `sub_agent.usage()` before it is dropped. Update all direct callers and assertions for the public signature change.

- [x] **Step 4: Run focused tests GREEN**

Run: `cargo test -p barnstormer-agent countable_tokens run_agent_step -- --nocapture`

Expected: focused tests pass and existing step tests remain green.

- [x] **Step 5: Commit**

Run `git status --short`, stage the two agent files, then `git commit -m "feat: report countable swarm tokens"`.

### Task 3: Budget preflight, consent, and answer handling

**Files:**
- Modify: `crates/barnstormer-agent/src/swarm.rs`
- Modify: all `SwarmOrchestrator::{with_defaults,with_agents}` callers found by rename-safety search

**Interfaces:**
- Produces orchestrator fields `token_budget: Option<u64>`, `session_tokens_used: u64`, and `pending_budget_question: Option<Ulid>`.
- Consumes existing `question_pending` CAS, `Command::AskQuestion`, `UserQuestion::Boolean`, `pause()`, and the shared transition-answer receiver.

- [x] **Step 1: Write failing behavior tests**

Add real actor/stub tests for: unlimited never asks; ceiling asks exactly once while pending; preflight blocks the normal indexed dispatch; the human-notify manager priority path cannot dispatch over budget; Yes resets usage and clears the budget ID; No and any non-Yes pause and clear it; a nonmatching/stale answer changes nothing; a manual resume while still over budget asks again before work; tiny-budget integration emits `QuestionAsked`, freezes agents, resumes after Yes, and pauses after No.

Tests observe actor state/events and stub request counts rather than internal fake behavior.

- [x] **Step 2: Run tests to verify RED**

Run: `cargo test -p barnstormer-agent budget -- --nocapture`

Expected: compile failures for the new constructor arguments/fields and behavior assertions failing because dispatch is not gated.

- [x] **Step 3: Implement the preflight and accounting**

At the top of `run_agent_by_index`, short-circuit unlimited; when `session_tokens_used >= budget`, never take the runner. On first trip, win `question_pending.compare_exchange(false, true, ...)`, send one false-default boolean `AskQuestion`, and store its ULID. If the CAS loses, return without dispatch. After an allowed step, restore the runner and saturating-add `outcome.countable_tokens` under the swarm mutex.

- [x] **Step 4: Extend the one shared answer drain**

Rename the drain to reflect both responsibilities. For matching budget IDs, clear the ID and either reset `session_tokens_used` for an exact Yes-shaped answer or call `pause()` for every other answer. Preserve transition behavior and ignore stale IDs. Do not independently clear `question_pending`.

- [x] **Step 5: Run focused tests GREEN**

Run: `cargo test -p barnstormer-agent budget -- --nocapture`

Expected: all budget unit/integration tests pass with no warnings or leaked tasks.

- [x] **Step 6: Commit**

Run `git status --short`, stage only changed agent/caller paths, then `git commit -m "feat: require consent for additional swarm tokens"`.

### Task 4: End-to-end verification and review

**Files:**
- Modify: `tests/smoke.rs` only if runtime lifecycle coverage does not exercise both required startup configurations.
- Modify: `docs/plans/token-budget-cookoff/impl-3/plan.md` to check completed steps and record RED evidence.

- [x] **Step 1: Run real startup usage**

Run server lifecycle tests under serialized env mutation for `BARNSTORMER_TOKEN_BUDGET=0` and a tiny budget, probe `/health`, and shut down cleanly. Run an invalid-value startup and assert the clear error.

- [x] **Step 2: Run complete quality gate**

Run exactly:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

Expected: exit 0 and pristine output for all commands.

- [x] **Step 3: Self-review every design requirement**

Verify count source, cache-read exclusion, error usage, both dispatch paths, one-question CAS behavior, Yes/no/stale/manual-resume edge cases, default/set/zero/invalid config, per-spec in-memory state, both server spawn sites, and E2E startup wiring. Perform separate searches for changed signatures in direct references, type references, string literals, dynamic imports/re-exports, and tests.

- [x] **Step 4: Commit final plan/evidence**

Run `git status --short`, stage the plan and any final test-only paths explicitly, then `git commit -m "test: verify swarm token budget end to end"`.

## Plan Self-Review

- Spec coverage: every approved mechanics, config, edge-case, unit, integration, and E2E requirement maps to Tasks 1–4.
- Placeholder scan: no TBD/TODO/future implementation placeholders remain.
- Type consistency: `Option<u64>` means unlimited only when `None`; `StepOutcome.countable_tokens` and `session_tokens_used` are `u64`; budget question identity is `Option<Ulid>`.

## Execution Evidence

- Runtime RED: E0609 for the missing `RuntimeConfig.token_budget` field.
- Accounting RED: E0425/E0610 for missing `countable_tokens` and structured `StepOutcome`.
- Partial-error RED: expected 48 accumulated countable tokens, observed 0.
- Manager-priority RED: E0425 for the not-yet-extracted human-notification dispatch helper.
- Resume RED: after a real budget-question atomic lifecycle, No → pause → Resume failed to re-ask because the cached question flag remained true.
- Approved design correction: synchronize `question_pending` from actor state after every drained `QuestionAnswered`; the Resume regression then passed.
- Real-loop integration: a 100-token budget froze the swarm after a 120-token step, Yes resumed work with a fresh tranche, and No paused it.
- Final gates: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --all` pass.
