# Phase Wayfinding — Information Architecture Redesign

## Problem

The command bar crams identity, phase status, phase transitions, view toggles, and agent controls into a single 56px row with no visual hierarchy. A new user sees 8+ clickable elements at the same level and has no idea what's structural vs. what's dangerous (phase transitions).

## Design

### Layered Layout

Four concerns, four layers:

```
┌──────────────────────────────────────────────────────────┐
│  Title › One-liner                        [● Agents active] │  Command bar (identity + status)
├──────────────────────────────────────────────────────────┤
│    ① Brainstorm ────── ② Refine ────── ③ Complete         │  Phase stepper (wayfinding)
├──────────────────────────────────────────────────────────┤
│                [Document] [Board] [Workflow]                │  View toggles (contextual)
├──────────────────────────────────────────────────────────┤
│                                                            │
│                     Content area                           │
│                                                            │
└──────────────────────────────────────────────────────────┘
```

**Row 1 — Command bar:** Spec identity on the left, agent status pill on the right. Nothing else.

**Row 2 — Phase stepper:** Always visible. Shows Brainstorm → Refine → Complete as a linear progression with numbered circles connected by lines (like a checkout flow).

**Row 3 — View toggles:** Only rendered when the active phase has toggles. Currently only Refining (Document / Board / Workflow). Designed as a reusable component so other phases can add toggles later without structural changes.

**Row 4 — Content area:** Unchanged from current behavior.

### Phase Stepper Behavior

The stepper replaces all phase transition buttons ("Finalize Spec", "Resume Brainstorming", "Keep Refining") with direct navigation.

**Step states:**
- **Completed:** Filled/checked circle, solid connector line, clickable — navigates to that phase
- **Active:** Highlighted/accent circle, bold label — you are here
- **Upcoming (disabled):** Muted/gray circle, faded connector, not clickable — tooltip on hover explains prerequisite (e.g., "Complete brainstorming to unlock refining")

**Transitions:** Clicking a step POSTs to the existing `/web/specs/{id}/phase` endpoint. The stepper is a better UI for the same backend action.

**Onboarding:** A new user starting a spec sees Brainstorm active, Refine and Complete grayed out with tooltips. They immediately understand the lifecycle without explanation.

### View Toggles Component

A reusable pill-capsule partial that takes a list of toggle definitions (label, icon, endpoint) and the active view. Each phase template decides whether to include it and with what options.

Currently only Refining populates it with Document / Board / Workflow. Brainstorming may add Chat / Context / Code in the future.

### What Gets Removed

- Phase badge markup and CSS (`REFINING` / `BRAINSTORMING` / `COMPLETE`)
- "Finalize Spec" button
- "Resume Brainstorming" button
- "Keep Refining" button
- "View Board" / "Back to Chat" toggle hack in brainstorming
- Hardcoded view toggles in the command bar

### What Gets Added

- `templates/partials/phase_stepper.html` — stepper partial, takes current phase
- `templates/partials/view_toggles.html` — toggle partial, takes toggle list + active view
- Stepper and toggle CSS in `static/style.css`

### What Moves

- Download .md button moves from command bar into the document canvas content (Complete phase)
- Agent status pill stays in command bar but is now the only control there

### What's Preserved

- All existing API endpoints (phase transitions, view loading, agent control)
- SSE event handling and phase polling fallback
- Agent status pill behavior and offline overlay
- Chat rail in Refining phase
- Content rendering for all phases
