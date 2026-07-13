# Project Gotchas

- `question_pending` is a cache of the actor's event-sourced `pending_question`, not an independent authority. Any swarm-level consumer that drains `QuestionAnswered` must synchronize the atomic after the actor reducer clears the question; otherwise dispatch gating can prevent the context refresh that normally self-heals it.
