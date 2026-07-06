# Token Budget for the Swarm Run Loop — Design

Date: 2026-07-06
Roadmap item: M1.1 (docs/ROADMAP.md) · Issue: #55
Status: approved design, pre-implementation

## Problem

The swarm's `run_loop` (`crates/barnstormer-agent/src/swarm.rs`) has no outer
bound. Each agent step is capped (`max_iterations(10)` on the mux
`AgentDefinition`), but the outer loop cycles agents forever. A runaway or
merely long session can burn unbounded tokens with no visibility and no
consent checkpoint.

## Decisions

Settled with Doctor Biz during brainstorming:

| Decision | Choice |
|----------|--------|
| Budget unit | Tokens (not steps, not dollars) |
| Countable tokens | `input + output + cache_write`; cache reads excluded |
| At the ceiling | Ask the user via the existing question mechanism |
| Yes answer | Grants a fresh tranche (counter resets to zero) |
| No / non-Yes answer | Trip the existing pause breaker |
| Default tranche | 2,000,000 countable tokens |
| Config | `BARNSTORMER_TOKEN_BUDGET` env var; `0` = unlimited; invalid value = startup error |
| Scope | Per spec, per session, in-memory (approach 1 of 3 considered) |

Cache reads are excluded because they are roughly 10× cheaper than uncached
input and dominate raw token counts on well-cached sessions; counting them
raw would trip the ceiling on cheap sessions and train the user to click
through the question. Runaway steps still burn uncached input and output, so
exclusion does not weaken the guard.

Approaches considered and rejected: durable per-step usage events with
state-derived budgets (that is M1.2's accounting job, not a safety breaker;
grows the event schema while M2.4 forward-compat is still open) and a budget
middleware inside mux-rs's `LlmClient` (broadest coverage but enforces by
erroring mid-step, cannot host a consent flow, and is a cross-repo change).

## Assumptions

- The counter is in-memory and session-scoped: a server restart resets it.
  This is a runaway guard, not accounting — durable usage tracking arrives
  with M1.2 and can layer on top without conflict (same source:
  `SubAgentResult.usage`).
- Only the swarm loop is covered. `barnstormer import` and attachment
  summarizer LLM calls are out of scope.
- Each active spec's swarm has its own independent budget.

## Mechanics

### Counting

- New pure helper `countable_tokens(usage: &Usage) -> u64` returning
  `input_tokens + output_tokens + cache_write_tokens`.
- `run_agent_step` returns `StepOutcome { did_work: bool, countable_tokens: u64 }`
  instead of `bool`. (`run_agent_step` is `pub` — rename-safety search for all
  callers, including tests, when changing the signature.)
- `run_agent_by_index` adds the step's countable tokens into a new
  `session_tokens_used: u64` field on `SwarmOrchestrator` (mutated under the
  existing swarm mutex).
- Error paths count best-effort partial usage. mux's runner keeps a running
  `usage()` accessor for exactly this; if `SubAgent` does not expose usage
  after a failed `run()`, add a small additive accessor to mux-rs.

### Tripping

The check lives at the top of `run_agent_by_index` — the single dispatch
choke point. `run_loop` calls it from two places (the round-robin for-loop
and the human-message notify arm that priority-dispatches the manager);
putting the check inside the callee covers both paths. Invariant: **over
budget ⇒ never dispatch; ask when able.**

1. If budget is unlimited (`None` internally), proceed — zero-cost.
2. If `session_tokens_used >= budget`, do not dispatch the agent. Then, if
   `pending_budget_question` is `None`:
   - CAS `question_pending` false→true (same discipline as the `ask_user`
     tools). If the CAS loses to an agent's question, just skip — agents are
     already frozen by that question; the ask re-fires on a later pass.
   - On CAS win: send `Command::AskQuestion` with
     `UserQuestion::Boolean { question, default: Some(false), .. }` — wording
     along the lines of "This session has used ~2.0M countable tokens
     (budget: 2.0M). Spend another 2.0M?" — and store the question id in
     `pending_budget_question: Option<Ulid>` on the swarm.
3. Question gating (`has_pending_question`) freezes all agents while the
   question is open; no new gating code is needed.

Checking before every dispatch bounds overshoot at one step (the step that
crossed the line) on all paths, and guarantees a paused-then-resumed
over-budget session re-asks before spending anything.

### Answering

`drain_transition_answers` is extended (single function; both of its call
sites share one `phase_rx` broadcast receiver, and a sibling drain function
would steal events from it). For each `QuestionAnswered`:

- Matches `pending_transition_question` → existing phase-transition behavior,
  unchanged.
- Matches `pending_budget_question`:
  - Answer "Yes" → `session_tokens_used = 0`, clear `pending_budget_question`.
  - Any other answer → `pause()` (existing breaker, existing UI), clear
    `pending_budget_question`.

The `question_pending` atomic clears through the same lifecycle
`propose_transition` questions use (reducer clears state; the flag self-heals
via context refresh). Mirror that exactly; do not invent a new clearing path.

### Config path

- `BARNSTORMER_TOKEN_BUDGET` parsed in the runtime crate's config (the real
  startup path — not the server crate's dead `BarnstormerConfig::from_env`).
- Unset → 2,000,000. `"0"` → unlimited. Non-numeric / negative → startup
  error, matching the contract the `BARNSTORMER_BIND` fix established.
- Threaded into `SwarmOrchestrator` construction; both swarm spawn sites in
  `web/mod.rs` receive it from config.

## Edge cases

- **CAS lost to an agent question**: benign; agents frozen either way; budget
  check retries after the agent question resolves.
- **Restart with budget question pending**: question survives in the event
  log; the new session has a zeroed counter and empty
  `pending_budget_question`. A late answer to the stale question matches
  nothing and is ignored.
- **"No" then manual Resume**: counter still ≥ budget, so the pre-dispatch
  check re-asks before any spend. Deliberate double-consent.
- **Manual pause while budget question pending**: orthogonal; a Yes resets
  the counter but the swarm stays paused until resumed.
- **Unlimited**: short-circuits before any bookkeeping conversation.
- **Overshoot**: bounded at one step (~10–25k countable tokens typical).

## Testing

TDD throughout; StubLlmClient (`barnstormer-agent/src/testing.rs`) with
nonzero `Usage` where needed.

Unit:
- `countable_tokens` excludes cache reads.
- Trip fires exactly once while a budget question is pending.
- Drain: Yes resets counter and clears pending; non-Yes pauses; non-matching
  question ids are ignored.
- Unlimited budget never trips.
- Over budget blocks dispatch on both paths: the round-robin pass and the
  human-message manager-priority path.
- Stale (restart-orphaned) budget question answer is ignored.

Integration:
- Real swarm + stub client + tiny budget (e.g. 100 tokens): `QuestionAsked`
  appears in the event stream; agents freeze; Yes → work resumes, counter
  zeroed; No → `is_paused()` true.
- Config: unset → 2M; set → honored; `0` → unlimited; invalid → error.
  Serialize env-var tests against parallel env writers, following the
  `BARNSTORMER_BIND` test pattern (commits b4f92dd, 3dbf75a).

End-to-end:
- Smoke test grows cases booting the server with `BARNSTORMER_TOKEN_BUDGET=0`
  and with a tiny budget, verifying the wiring.

## Size estimate

~200–300 LOC implementation + ~300–400 LOC tests. Touches:
`crates/barnstormer-agent/src/swarm.rs` (most of it), the runtime crate's
config, `crates/barnstormer-server/src/web/mod.rs` (construction only),
possibly a few additive lines in mux-rs (post-error usage accessor).

## Follow-ups enabled (not in scope)

- M1.2 token/cost display reads the same `SubAgentResult.usage` source and
  can add durable usage events without conflicting with this counter.
- A dollar-denominated budget could layer a pricing table on top later.
