# Spec 2: Brainstorming UI

**Goal:** Provide a full-width chat layout during the brainstorming phase, with a dynamic canvas panel, board peeking, and smooth transitions between brainstorming and active modes.

**Depends on:** Spec 1 (Phase Model & Swarm Gating)

**Context:** This is the second of three specs that together build a guided brainstorming flow:
- **Spec 1:** Phase model plumbing — domain types, swarm gating, API endpoint
- **Spec 2 (this):** Brainstorming UI — full-width chat layout, canvas panel container, transition flow
- **Spec 3:** Manager intelligence — reworked prompts, `show_canvas` tool, `propose_transition` tool, canvas SSE events

## Layout Modes

The `spec_view.html` template conditionally renders based on `state.phase`:

### Brainstorming mode (`phase == Brainstorming`)

- Command bar shows: spec title, "Brainstorming" phase badge (purple), "View Board" button
- Tab toggles (Document/Board/Spec) are hidden
- Full-width chat as the main content area (replaces the canvas + chat-rail split)
- Chat input at the bottom
- Hidden `<div id="agent-canvas">` container for future canvas tool (Spec 3)

### Active mode (`phase == Active`)

- Command bar shows: spec title, tab toggles (Document/Board/Spec), "Resume Brainstorming" button
- Normal tabbed layout with canvas on the left, chat rail on the right
- Existing behavior unchanged

The template uses a single `spec_view.html` with conditional blocks based on a `phase` field passed from the handler. Not two separate templates.

### DOM Structure

In both modes, the `<main class="canvas" id="canvas">` element exists. This is the HTMX swap target for all content views.

**Brainstorming mode:** `#canvas` contains the full-width chat. The `<aside class="chat-rail">` is hidden (not rendered). The chat partial loads directly into `#canvas` with a `chat-fullwidth` wrapper class.

**Active mode:** `#canvas` contains the tabbed content (document/board/spec). The `<aside class="chat-rail">` is visible alongside it. Standard two-column layout.

This means `#canvas` is always a valid HTMX target — the "View Board" button, tab toggles, and "Back to Chat" button all target `#canvas`.

## Full-Width Chat Layout

During brainstorming, the chat is the main content — not a sidebar.

### Reuse of existing infrastructure

- The existing `chat_panel.html` and `chat_transcript.html` templates render messages, questions (Boolean/MultipleChoice/Freeform), and the input area
- During brainstorming, these same partials render into `#canvas` instead of the narrow chat rail
- The SSE triggers (`sse:transcript_appended`, `sse:question_asked`, etc.) work the same way

### Chat panel container_id

The `chat_panel.html` template uses `hx-target="#{{ container_id }}"` for form swap targets. The handler passes:
- `container_id: "chat-rail"` in Active mode (current behavior)
- `container_id: "canvas"` in Brainstorming mode (chat lives in the main canvas)

### Styling

- A wrapper class `chat-fullwidth` on the container controls the wider layout
- Chat messages get a comfortable max-width (720px centered) — like ChatGPT's centered column
- Question cards (multiple choice buttons, freeform inputs) get more breathing room

No separate brainstorming chat template. The existing chat partials are reused. The full-width vs sidebar difference is purely CSS.

### Card-creation notifications

When the Manager creates a card during brainstorming, the existing `TranscriptAppended` event renders the Manager's message ("I've captured that as a decision card") in the chat. The card preview shown inline is just the Manager's message content — no new event type or template partial needed. The Manager's message text describes the card it created. Fancier inline card previews (with styled borders, card type badges) are a future enhancement, not in scope for Spec 2.

## Canvas Panel (Agent-Controlled)

The canvas panel is for the Manager's `show_canvas` tool (Spec 3). Spec 2 builds the container only; Spec 3 defines the SSE events and makes it functional.

### Container

- A `<div id="agent-canvas">` exists inside the brainstorming layout, hidden by default (`display:none`)
- CSS for the split layout: when `agent-canvas` is visible, the brainstorming body becomes a flex row — chat on the left (flex: 1), canvas on the right (flex: 1). Approximately 50/50 split, with a minimum width of 300px on each side to prevent crushing on small screens.

### SSE activation (Spec 3 scope)

Spec 3 will define the SSE event names and mechanism for showing/hiding the canvas. Spec 2 only builds the container div and the CSS. The JS that listens for SSE events and toggles visibility is also Spec 3's concern — it requires knowing the event format that `show_canvas` produces.

### Spec 2 deliverable

The `<div id="agent-canvas">` container with its CSS styling. No JS, no SSE listeners. Purely a placeholder that Spec 3 wires up.

## "View Board" Button and Board Peeking

During brainstorming, the user can peek at the board without ending brainstorming.

### "View Board" button

- Sits in the command bar during brainstorming mode
- HTMX: `hx-get="/web/specs/{id}/board" hx-target="#canvas" hx-swap="innerHTML"`
- When clicked, the board partial replaces the full-width chat in `#canvas`
- A "Back to Chat" button appears in the command bar (replacing "View Board")
- The phase stays `Brainstorming` — agents don't change behavior

### "Back to Chat" button

- HTMX: `hx-get="/web/specs/{id}/chat-panel" hx-target="#canvas" hx-swap="innerHTML"`
- Uses the existing `/web/specs/{id}/chat-panel` endpoint that already serves the chat partial
- The handler checks the phase and passes `container_id: "canvas"` + `chat-fullwidth` wrapper for brainstorming mode
- Replaces the board view with the full-width chat in `#canvas`

### Command bar button state

JS in `spec_view.html` tracks which view is active during brainstorming:
- Default: "View Board" button shown, "Back to Chat" hidden
- After clicking "View Board": "View Board" hidden, "Back to Chat" shown
- After clicking "Back to Chat": swap back

This is not a phase transition. It's a client-side view toggle within brainstorming mode.

## Phase Transition UX

### Brainstorming → Active

When the user confirms the Manager's "Ready to build?" prompt (Spec 3), a `POST /web/specs/{id}/phase` with `target=active` fires. The UI handles the `sse:phase_transitioned` event:

1. Command bar swaps: "Brainstorming" badge disappears, tab toggles appear, "Resume Brainstorming" button appears
2. Canvas swaps from full-width chat to the board view — user watches cards populate live via existing SSE card events
3. Chat moves to the right rail
4. No interstitial/loading screen — the user sees the layout shift and the board fill up in real-time

### Active → Brainstorming ("Resume Brainstorming")

1. "Resume Brainstorming" button in the command bar fires `POST /web/specs/{id}/phase` with `target=brainstorming`
2. On `sse:phase_transitioned`, the UI swaps back: tabs hidden, full-width chat appears, badge returns
3. Non-Manager agents pause (Spec 1's swarm gating)
4. The Manager picks up where it left off — reads existing cards for context (Spec 3 concern)

### Implementation

The `sse:phase_transitioned` event triggers a full re-fetch of the spec view partial (`hx-get="/web/specs/{id}" hx-target="#workspace"`). Since the handler passes the phase to the template, it renders the correct layout.

**SSE reconnection:** Replacing `#workspace` drops the existing SSE connection (the `sse-connect` attribute is on the spec-compositor div inside workspace). The new HTML re-establishes the SSE connection. There is a brief reconnection window — events emitted during this window could be missed. This is acceptable because:
- The re-fetched view loads current state on `hx-trigger="load"` (canvas and chat-rail both have load triggers)
- Any cards created during the reconnection window appear when the view loads
- The reconnection window is typically <100ms

**Edge case — transition while peeking at board:** If the user is viewing the board (via "View Board") and a phase transition SSE arrives, the full workspace re-fetch replaces everything with the correct layout for the new phase. The board peek state is lost, which is fine — the user is now in a different mode.

## File Changes

### Modified files

- `templates/partials/spec_view.html` — conditional rendering based on phase (brainstorming vs active layout)
- `templates/partials/chat_panel.html` — accept `container_id` for brainstorming mode, add `chat-fullwidth` wrapper class support
- `static/style.css` — full-width chat styles (`.chat-fullwidth`), canvas panel container styles (`#agent-canvas`), brainstorming badge, "View Board"/"Back to Chat" button styles
- `crates/barnstormer-server/src/web/mod.rs` — spec view handler passes `phase` to template struct; chat panel handler passes appropriate `container_id` based on phase

### No new files

All changes go in existing files.

### Unchanged

- `chat_transcript.html` — message rendering stays the same, just gets more space
- Board/Document/Spec tab handlers — unchanged, still work when toggled to
- Chat handler (`POST /web/specs/{id}/chat`) — unchanged
- Answer handler (`POST /web/specs/{id}/answer`) — unchanged
- SSE stream — unchanged (new events flow through existing infrastructure)

## Testing

### Template tests (barnstormer-server)

- Spec view renders brainstorming layout when `phase == Brainstorming` — no tab toggles, has "Brainstorming" badge, has "View Board" button, no "Resume Brainstorming" button
- Spec view renders active layout when `phase == Active` — has tab toggles, has "Resume Brainstorming" button, no brainstorming badge, no "View Board" button
- Brainstorming layout contains `#canvas` element (HTMX target)
- Brainstorming layout contains hidden `#agent-canvas` container
- Chat in brainstorming mode has `chat-fullwidth` class
- Existing spec view tests continue to pass (Active is the default for existing specs)

### Handler tests (barnstormer-server)

- `GET /web/specs/{id}` for a spec in Brainstorming phase returns HTML containing brainstorming layout markers (`chat-fullwidth`, `data-view="brainstorming"`)
- `GET /web/specs/{id}` for a spec in Active phase returns HTML containing tab toggles (`data-view="document"`)
- Chat panel handler returns `container_id: "canvas"` context in Brainstorming phase
- Chat panel handler returns `container_id: "chat-rail"` context in Active phase
- Board endpoint still returns 200 during Brainstorming phase (for "View Board" peeking)

### Manual testing (not automated)

- Phase transition layout swap via SSE (requires browser)
- "View Board" / "Back to Chat" toggle during brainstorming
- SSE reconnection after phase transition (verify no lost events)
- `show_canvas` tool integration (Spec 3 — not testable until then)

## Out of Scope

- Phase model itself (Spec 1)
- `show_canvas` / `propose_transition` tool implementation (Spec 3)
- Canvas SSE events and JS listeners (Spec 3)
- Manager prompt changes (Spec 3)
- Agent behavior changes (Spec 3)
- Inline card preview styling in chat (future enhancement)
