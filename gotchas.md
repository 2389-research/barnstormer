# Project Gotchas

- `question_pending` is a cache of the actor's event-sourced `pending_question`, not an independent authority. Any swarm-level consumer that drains `QuestionAnswered` must synchronize the atomic after the actor reducer clears the question; otherwise dispatch gating can prevent the context refresh that normally self-heals it.
- A paused swarm must still drain user answers. Pause blocks agent dispatch, not control-plane events; skipping answer drains while paused strands budget and phase consent.
- Startup `/health` tests prove parsing and boot only. Configuration wiring needs a real runtime-to-state-to-orchestrator scenario that exercises the resulting behavior with local components.
