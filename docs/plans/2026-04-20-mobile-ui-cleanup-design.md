# Mobile UI Cleanup Design

## Goal

Make the barnstormer web UI fully usable on mobile devices (≤900px), addressing layout overflow, missing navigation, and awkward content splits.

## Current State

Single breakpoint at 900px. On mobile: sidebar collapses to a cramped horizontal strip, phase stepper overflows, refining mode keeps the 380px chat rail side-by-side with the canvas (unusable on phones), view toggles become anonymous icons, and the subtitle disappears with no way to see it.

## Design

### 1. Subtitle Popover (mobile only)

On mobile, the command bar title becomes tappable. Clicking shows a small floating popover below with the subtitle text. Tap outside or tap title again to dismiss. Desktop behavior unchanged (subtitle always visible inline).

### 2. Hamburger Menu + Sidebar Drawer

Add a hamburger icon to the left side of the command bar on mobile (hidden on desktop). Tapping opens the nav rail as a slide-in overlay from the left with a semi-transparent backdrop. Sidebar takes ~80% of screen width. Dismiss by tapping backdrop or hamburger. The current horizontal strip layout for the nav rail is removed on mobile.

### 3. Refining Mode: Tabbed Layout (mobile only)

Replace the side-by-side canvas + chat rail split with a tab switcher on mobile. Two tabs at the bottom of the toolbar area: the active view name (Document/Board/Spec) and "Chat". Default to Document tab on load. The view toggles (Document/Board/Spec) remain in the Document tab context. CSS-only toggle using a class on the spec-body to show/hide canvas vs chat-rail.

### 4. Phase Stepper Responsiveness

On mobile, hide connector lines and non-active step labels. Show compact format: step numbers for inactive steps, number + label for the active step. Example: `(1) — Refine — (3)` instead of full `Brainstorm ------- Refine ------- Complete`.

### 5. View Toggles: Show Active Label

On mobile, keep icons-only for inactive toggles but show the label for the currently active toggle. Example: `[doc-icon Document] [board-icon] [spec-icon]` — gives context for which view is active without taking too much space.

## Breakpoint

All changes apply at the existing `@media (max-width: 900px)` breakpoint. No new breakpoints needed.

## Approach

CSS-first where possible, minimal JS for interactive behaviors (hamburger toggle, subtitle popover, mobile tab switcher). No new endpoints or template restructuring — use classes and display toggling.
