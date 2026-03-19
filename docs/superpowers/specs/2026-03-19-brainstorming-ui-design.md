# Spec 2: Brainstorming UI (Light Spec — To Be Fully Designed)

**Goal:** Full-width chat layout during brainstorming phase, with dynamic canvas panel, transition animation to tabbed view, and ability to resume brainstorming.

**Depends on:** Spec 1 (Phase Model & Swarm Gating)

## Key Decisions (from brainstorming session)

- **Brainstorming phase:** Full-width chat is the main content area. Tab toggles (Document/Board/Spec) are hidden. Command bar shows "Brainstorming" phase badge.
- **Canvas panel:** Hidden by default. Appears on the right when Manager pushes HTML content via `show_canvas` tool (Spec 3). Chat stays on the left. Collapses when canvas is cleared.
- **"View Board" button:** Visible during brainstorming so user can peek at cards being captured. Doesn't end brainstorming — just shows the board temporarily.
- **Transition to Active:** When user confirms "Ready to build?" (via Spec 3's `propose_transition` tool), UI transitions to tabbed view. Brief "Building your spec..." animation while agents do initial card pass. User can click to the board and watch cards appear live via SSE.
- **Resume brainstorming:** "Resume brainstorming" button available in Active mode. Triggers `POST /web/specs/{id}/phase` with `target=brainstorming`. UI swaps back to full-width chat layout. Non-Manager agents pause.
- **Chat rail in Active mode:** Existing chat rail stays on the right side during Active mode. Agents can still ask questions via the rail.
- **SSE integration:** Listen for `sse:phase_transitioned` to trigger layout swaps without full page reload.

## Components to Build

1. Brainstorming layout template (full-width chat, no tabs)
2. Canvas panel container (hidden by default, SSE-driven show/hide)
3. Phase badge in command bar
4. "View Board" button during brainstorming
5. Transition animation / interstitial
6. "Resume brainstorming" button in Active mode
7. SSE handler for `phase_transitioned` event to swap layouts
8. Update `spec_view.html` to conditionally render based on phase

## Out of Scope

- Agent prompt changes (Spec 3)
- `show_canvas` / `propose_transition` tool implementation (Spec 3)
- Phase model itself (Spec 1)
