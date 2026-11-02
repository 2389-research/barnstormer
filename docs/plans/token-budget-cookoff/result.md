# Token Budget Cookoff Results

## Design

`docs/plans/2026-07-06-token-budget-design.md`

Approved correction discovered during implementation: after processing a
question answer, synchronize `question_pending` from authoritative actor state.
Paused swarms must continue draining control-plane answers without dispatching
agents.

## Implementations

| Impl | Plan | Final branch head | Verification | Fresh-eyes | Added lines | Result |
|------|------|-------------------|--------------|------------|-------------|--------|
| impl-1 | `impl-1/plan.md` | `c5a03c8` | fmt, full and targeted clippy, full tests, 9 scenarios | clean | 1,751 | eliminated |
| impl-2 | `impl-2/plan.md` | `a978b01` | fmt, full and targeted clippy, full tests | 1 Minor | 1,406 | eliminated |
| impl-3 | `impl-3/plan.md` | `6b844f5` | fmt, full and targeted clippy, full tests | clean | 1,256 | **winner** |

All three finalists passed the approved design gate with no Critical or
Important review findings.

## Judge Scorecard

| Criterion | impl-1 | impl-2 | impl-3 | Best |
|-----------|--------|--------|--------|------|
| Fitness for Purpose | 5 | 5 | 5 | Tie |
| Justified Complexity | 3 | 4 | 5 | impl-3 |
| Readability | 3 | 4 | 5 | impl-3 |
| Robustness & Scale | 5 | 4 | 4 | impl-1 |
| Maintainability | 4 | 4 | 4 | Tie |
| **Total** | **20/25** | **21/25** | **23/25** | **impl-3** |

Hard gates were not triggered: Fitness delta was less than two and no
implementation received a score of one.

## Winner Selection

Impl-3 fully implements the approved design and correction with the leanest
final diff, deterministic receiver ownership, accurate answer-drain naming,
panic-safe environment tests, and one production swarm factory shared by both
web spawn paths. Its lower complexity and clearer core flow outweigh impl-1's
additional broadcast-loss safeguards, which are valuable but extend beyond the
approved scope.

Impl-1 offers the strongest fail-closed behavior for irretrievably lost answer
broadcasts and the broadest tracked scenario gate. Impl-2 correctly re-gates
recovered pending questions before dispatch and is close in scope, but retained
one cosmetic readability finding and a larger total diff than impl-3.

## Branches and Worktrees

- Winner: `feat/token-budget-cookoff-impl-3` at
  `.worktrees/token-budget-impl-3`
- Loser branches and worktrees remain preserved until Doctor Biz chooses the
  integration option.
