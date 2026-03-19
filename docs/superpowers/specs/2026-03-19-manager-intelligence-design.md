# Spec 3: Manager Intelligence & Canvas Tool (Light Spec — To Be Fully Designed)

**Goal:** Rework the Manager agent's behavior during brainstorming to drive structured Q&A, show visuals on a canvas, capture decisions as cards, and propose transitions when ready.

**Depends on:** Spec 1 (Phase Model), Spec 2 (Brainstorming UI)

## Key Decisions (from brainstorming session)

- **Manager drives Q&A via LLM** — not scripted. The Manager reads the user's idea, decides what questions to ask, adapts to responses. Inspired by superpowers brainstorming skill pattern.
- **One question at a time.** Multiple choice preferred. Boolean for yes/no. Freeform when needed.
- **Manager reads cards for context** — in subsequent brainstorming rounds (after "Resume brainstorming"), the Manager reads existing cards to know what's already been decided and asks smarter follow-ups.
- **Manager creates cards during Q&A** — as decisions emerge, the Manager captures them as cards (decision, constraint, task, risk, etc.) in the Plan/Spec lanes. Cards are the shared memory between brainstorming and the agent swarm.
- **Manager proposes transition** — after gathering enough context, Manager asks "I think we have enough to start building. Ready to move on?" User confirms before phase transitions to Active.

## New Agent Tools

### `show_canvas`

Pushes HTML content to the brainstorming canvas panel (Spec 2's UI). Used for:
- Architecture diagrams
- Mockups and wireframes
- Side-by-side comparisons
- Any visual that helps the user make a decision

Implementation: Manager calls tool with HTML string → server emits SSE event `canvas_updated` with the HTML → client renders it in the canvas panel.

### `propose_transition`

Asks the user if they're ready to move from brainstorming to active mode. Renders as a special question card in the chat with "Yes, let's build" / "Not yet, I have more to discuss" buttons.

When user confirms: sends `Command::TransitionPhase { target: Active }`.

## Manager System Prompt Changes

During `Brainstorming` phase, the Manager gets a different system prompt emphasizing:
- Understand the idea before creating cards
- Ask one question at a time
- Prefer multiple choice questions
- Capture decisions as cards as they emerge
- Read existing cards for context (especially after "Resume brainstorming")
- Use `show_canvas` when a visual would help the user decide
- Propose transition when you have enough context to build a spec
- DO NOT rush to create many cards — focus on understanding first

During `Active` phase, the Manager uses its existing system prompt (coordination, parsing human input from chat rail).

## Out of Scope

- Phase model itself (Spec 1)
- UI layout changes (Spec 2)
- Changes to other agent roles (Brainstormer, Planner, DotGenerator prompts)
