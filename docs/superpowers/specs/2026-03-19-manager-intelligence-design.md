# Spec 3: Manager Intelligence & Canvas Tool

**Goal:** Rework the Manager agent's behavior during brainstorming to drive structured Q&A, show visuals on a canvas, capture decisions as cards, and propose transitions when ready.

**Depends on:** Spec 1 (Phase Model & Swarm Gating), Spec 2 (Brainstorming UI)

**Context:** This is the third of three specs that together build a guided brainstorming flow:
- **Spec 1:** Phase model plumbing — domain types, swarm gating, API endpoint
- **Spec 2:** Brainstorming UI — full-width chat layout, canvas panel container, transition flow
- **Spec 3 (this):** Manager intelligence — reworked prompts, `show_canvas` tool, `propose_transition` tool, canvas SSE events

## Manager System Prompt — Brainstorming Mode

During `Brainstorming` phase, the Manager gets a different system prompt. The swarm already builds the system prompt per-agent via `full_system_prompt(&AgentRole, &str)` at `swarm.rs:117`. This function gains a new `phase: &SpecPhase` parameter: `full_system_prompt(&AgentRole, &str, &SpecPhase)`. When phase is `Brainstorming` and role is `Manager`, it returns the brainstorming variant. For all other combinations, it returns the existing prompt. The publicly exported `system_prompt_for_role` is unchanged — the phase-awareness is only in `full_system_prompt`, which is internal to the swarm.

### Current behavior

General-purpose coordination — parses input, creates cards, asks occasional questions.

### Brainstorming prompt priorities

1. **Understand the idea before creating cards** — don't rush to populate the board
2. **Ask one question at a time** — never multiple questions in one message
3. **Prefer multiple choice questions** — easier for the user to answer, faster iteration
4. **Use Boolean questions for yes/no decisions**
5. **Use Freeform only when the answer can't be anticipated**
6. **Capture decisions as cards as they emerge** — but sparingly, only when something is clearly decided
7. **Read existing cards for context** — especially after "Resume brainstorming", know what's already been decided
8. **Use `show_canvas` when a visual would help the user decide** — architecture diagrams, comparisons, mockups
9. **Call `propose_transition` when you have enough context** to build a full spec — typically after 3-8 questions depending on complexity

## `show_canvas` Tool

### Purpose

Let the Manager push HTML content to the canvas panel during brainstorming, for diagrams, mockups, and comparisons.

### Tool interface

```
show_canvas(content: String)
```

- `content` is an HTML string (fragment, not full document)
- Empty string clears the canvas
- If called when phase is `Active`, returns an error string to the LLM: "Canvas is only available during brainstorming." The tool validates phase before sending the command.

### Execution flow

1. Manager calls `show_canvas` with HTML content
2. Tool handler sends `Command::UpdateCanvas { content }` to the actor
3. Actor emits `EventPayload::CanvasUpdated { content }`
4. Event broadcasts via SSE as `canvas_updated` with the HTML payload
5. Client JS receives it, populates `#agent-canvas`, makes it visible
6. If content is empty: client hides `#agent-canvas`, chat goes back to full-width

### New command/event pair

```rust
Command::UpdateCanvas { content: String }
EventPayload::CanvasUpdated { content: String }
```

### State

`SpecState` gets a new field:

```rust
#[serde(default)]
pub canvas_content: Option<String>,
```

Reducer:
- `CanvasUpdated` with non-empty content: sets `canvas_content` to `Some(content)`
- `CanvasUpdated` with empty content: sets `canvas_content` to `None`

This persists the canvas content so it survives page refreshes — the brainstorming chat view renders `#agent-canvas` visible on load if there's content.

Not undoable. Canvas updates are transient visual aids, not content mutations. When an `UndoApplied` event fires, `canvas_content` is cleared to `None` to avoid showing stale content that references undone decisions. The Manager will re-populate the canvas if needed on its next step.

Existing snapshots without `canvas_content` deserialize with `None` via `#[serde(default)]`.

## `propose_transition` Tool

### Purpose

Let the Manager ask the user if they're ready to move from brainstorming to active mode.

### Tool interface

```
propose_transition(summary: String)
```

- `summary` is a brief recap of what the Manager has learned — "Here's what I understand so far: [summary]. Ready to build the spec?"

### Execution flow

1. Manager calls `propose_transition` with a summary string
2. Tool handler sends `Command::AskQuestion` with a Boolean question:
   ```rust
   UserQuestion::Boolean {
       question_id: Ulid::new(),
       question: format!("{summary}\n\nReady to move on and build the spec?"),
       default: Some(true),
   }
   ```
3. The existing question infrastructure handles rendering in chat, pausing agents, waiting for answer
4. When user answers "Yes": the swarm sends `Command::TransitionPhase { target: Active }`
5. When user answers "No": agents resume, Manager continues brainstorming

### Reuses existing infrastructure

This is NOT a new command/event pair. It reuses `AskQuestion` / `AnswerQuestion`. The only new piece is the logic that fires `TransitionPhase` when a transition-proposal question is answered affirmatively.

### Identifying a transition question

The `propose_transition` tool stores the `question_id` it generated via a shared `Arc<Mutex<Option<Ulid>>>` called `pending_transition_question`. This arc is created by `SwarmOrchestrator` and passed to `build_registry` alongside the existing `question_pending: Arc<AtomicBool>`. The tool writes the question ID into this arc after sending the `AskQuestion` command.

### Question-pending guard

`propose_transition` checks `question_pending` (the existing `AtomicBool`) before acting. If a question is already pending, the tool returns an error string to the LLM: "A question is already pending. Wait for the user to answer before proposing a transition." This is consistent with existing ask_user tool behavior.

### Answer-watching mechanism

The swarm's `run_loop` already subscribes to the broadcast channel via `tokio::select!`. Add a new processing branch: after each event batch is drained, check if the event is `QuestionAnswered` and the question ID matches the value in `pending_transition_question`. If it matches with a "yes" answer, the swarm sends `Command::TransitionPhase { target: Active }` and clears the stored ID. If answered "no", it clears the stored ID and the Manager continues brainstorming. This check lives in `run_loop` at the swarm level, not inside individual agent steps.

### Tool return value

`propose_transition` returns a string to the LLM: "Transition proposal sent to the user. They will see a confirmation prompt. Wait for their response before continuing." This tells the Manager not to send additional messages while the question is pending.

### Re-proposal after rejection

If the user answers "No", the stored ID is cleared and the Manager can call `propose_transition` again in a subsequent step. There is no cooldown or limit on re-proposals.

## Canvas SSE Events and Client JS

### SSE event

Event name: `canvas_updated`. Data payload: JSON with `content` field (HTML string).

No separate `canvas_cleared` event — a `canvas_updated` with empty content means clear. Single event type, simple client logic.

### Client JS

Added to `spec_view.html` inside the brainstorming conditional block:

```javascript
compositor.addEventListener('sse:canvas_updated', function(evt) {
    var data = JSON.parse(evt.detail.data);
    var canvas = document.getElementById('agent-canvas');
    if (data.content && data.content.trim() !== '') {
        canvas.innerHTML = data.content;
        canvas.style.display = 'block';
    } else {
        canvas.innerHTML = '';
        canvas.style.display = 'none';
    }
});
```

### HTML sanitization

The `show_canvas` tool handler strips `<script>` tags and `on*` event attributes from the HTML content before sending the `UpdateCanvas` command. This prevents XSS via prompt injection causing the LLM to emit malicious HTML. The sanitization is server-side in the tool handler, not client-side.

### On page load

If `state.canvas_content` is `Some(content)`, the brainstorming view renders `#agent-canvas` visible with the content pre-populated. The handler passes `canvas_content` to the template.

## File Changes

### New files

- `crates/barnstormer-agent/src/mux_tools/show_canvas.rs` — `show_canvas` tool implementation
- `crates/barnstormer-agent/src/mux_tools/propose_transition.rs` — `propose_transition` tool implementation

### Modified files

- `crates/barnstormer-core/src/command.rs` — add `UpdateCanvas` command variant
- `crates/barnstormer-core/src/event.rs` — add `CanvasUpdated` event variant
- `crates/barnstormer-core/src/state.rs` — add `canvas_content: Option<String>` field, reducer logic
- `crates/barnstormer-core/src/actor.rs` — handle `UpdateCanvas` command
- `crates/barnstormer-agent/src/swarm.rs` — brainstorming prompt switch, `pending_transition_question` tracking, register new tools
- `crates/barnstormer-agent/src/mux_tools/mod.rs` — register new tool modules, pass `pending_transition_question` arc to `build_registry`
- `crates/barnstormer-agent/src/context.rs` — add `CanvasUpdated` arm to `describe_event_payload` match
- `crates/barnstormer-server/src/api/stream.rs` — add `CanvasUpdated => "canvas_updated"` arm to `event_type_name` match
- `templates/partials/spec_view.html` — canvas SSE listener JS, pre-populate canvas on load
- `crates/barnstormer-server/src/web/mod.rs` — pass `canvas_content` to brainstorming template

## Testing

### Unit tests (barnstormer-core)

- `UpdateCanvas { content: "html" }` produces `CanvasUpdated { content: "html" }` event
- `CanvasUpdated` with content sets `state.canvas_content` to `Some(content)`
- `CanvasUpdated` with empty content sets `state.canvas_content` to `None`
- `CanvasUpdated` does not push an undo entry
- `UndoApplied` clears `canvas_content` to `None`
- `canvas_content` serde round-trip
- Existing snapshots without `canvas_content` deserialize with `None`
- `UpdateCanvas` command serde round-trip
- `CanvasUpdated` event serde round-trip

### Unit tests (barnstormer-agent)

- `show_canvas` tool sends `UpdateCanvas` command when called with content
- `show_canvas` tool sends `UpdateCanvas` with empty string to clear
- `show_canvas` tool returns error when phase is `Active`
- `show_canvas` tool strips `<script>` tags and `on*` attributes from content
- `propose_transition` tool sends `AskQuestion` with Boolean question type
- `propose_transition` tool returns error when question is already pending
- `propose_transition` stores question_id in swarm's `pending_transition_question`
- When transition question answered "yes", swarm sends `TransitionPhase { Active }`
- When transition question answered "no", swarm clears `pending_transition_question` and continues
- After transition question answered "no", Manager can call `propose_transition` again and a new question is asked
- Manager gets brainstorming system prompt when phase is `Brainstorming`
- Manager gets standard system prompt when phase is `Active`

### Integration tests (barnstormer-server)

- SSE stream includes `canvas_updated` event after `UpdateCanvas` command
- State API includes `canvas_content` field

### Manual testing

- Canvas panel appears/disappears in browser when Manager uses `show_canvas`
- `propose_transition` renders as Boolean question in chat
- Answering "yes" triggers phase transition and layout swap
- Manager asks structured questions during brainstorming (prompt quality)

## Out of Scope

- Phase model (Spec 1)
- UI layout changes (Spec 2)
- Changes to other agent roles (Brainstormer, Planner, DotGenerator prompts)
- Fancy inline card previews in chat (future enhancement)
