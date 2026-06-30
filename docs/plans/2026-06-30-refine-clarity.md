# Refine-Step Clarity Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make barnstormer's Refine step legible: you land on the board, the board shows two honest columns (Parked / In the spec), cards drop the robotic author line, and there's one output (the Spec) instead of two look-alike documents.

**Architecture:** All four are view-layer changes — no agent work, no event/command changes. The key trick for two columns: `export_spec` already treats **Plan ∪ Spec** as the deliverable and **Ideas** as parked, so we keep the internal lane *values* (Ideas/Plan/Spec) and present **two columns** by decoupling a display **label** from the lane **key** the drag writes. Export and the agent swarm are untouched.

**Tech Stack:** Rust, Axum, Askama, HTMX, SortableJS (board drag).

**Decisions captured (from the design pass with Sophie):** (1) land on board; (2) two columns Parked/In-the-spec with a visible ship line; (3) drop only the "by manager-…" card line (fuller designer-voice rewrite parked); (4) one output = Spec, Document demoted to a download.

---

## Conventions

- TDD where there's logic (the lane grouping); render assertions for templates; `cargo test --all` + `cargo clippy --all-targets -- -D warnings` green before each commit.
- Commit per task; messages end with the `Co-Authored-By: Claude Opus 4.8 (1M context)` trailer.

---

## Task 1 — Refine lands on the Board; one output (drop the Document tab)

**Files:** `templates/partials/spec_view.html` (the `refining` branch: view-toggles capsule + `#canvas` default), test in `crates/barnstormer-server/src/web/mod.rs` (the `spec_view`/refining render test).

**What:** In the refining branch:
- `#canvas` currently `hx-get`s `/document` on load → change to `/board`.
- The three toggles `Document (active) | Board | Spec` → become `Board (active) | Spec`. Remove the **Document** toggle button entirely. (The `/web/specs/{id}/document` route stays — it still backs the "Download .md" affordance and the export.)

**Step 1 (test):** add/extend a render test asserting the refining view (a) loads `/board` into `#canvas` by default, (b) has no Document view-toggle, (c) still has Board + Spec toggles. Run, watch fail.

**Step 2 (impl):**
- Change `<main class="canvas" id="canvas" hx-get="/web/specs/{{ spec_id }}/document" ...>` → `.../board`.
- Delete the `<button class="view-toggle active" data-view="document" ...>…Document…</button>` block.
- Put `active` on the Board toggle (`data-view="board"`).

**Step 3:** `cargo test --all`, `cargo clippy` green. **Commit:** `feat(ui): refine opens on the board; drop the Document tab`.

---

## Task 2 — Two columns: Parked / In the spec

**Files:** `crates/barnstormer-server/src/web/mod.rs` (`cards_by_lane` ~771, `LaneData` ~857), `templates/partials/board.html`. **No change to `export_spec` or `board.js`.**

**Design:** `LaneData` gains a `key` (the lane value the drag writes) separate from `name` (now the display label). The board renders exactly two lanes:
- **Parked** — `key="Ideas"`, holds cards whose lane is `Ideas`.
- **In the spec** — `key="Spec"`, holds cards whose lane is `Plan`, `Spec`, or any other non-Ideas value (so existing Plan cards and stragglers still show, and stay in the export since `export_spec` = Plan ∪ Spec).

Dragging a card to **Parked** sets its lane to `Ideas` (drops it from the deliverable); dragging to **In the spec** sets `Spec` (adds it). `board.js` already posts `MoveCard { lane: <data-lane> }`, so pointing `data-lane` at `key` is all that's needed.

**Step 1 (test):** in `mod.rs` tests, build a `SpecState` with cards in Ideas, Plan, and Spec lanes; assert `cards_by_lane` returns exactly **2** `LaneData` — `name=="Parked"` (the Ideas card) and `name=="In the spec"` (the Plan + Spec cards), with `key` `"Ideas"` and `"Spec"`. Run, watch fail.

**Step 2 (impl):**
- Add `pub key: String,` to `LaneData`.
- Rewrite `cards_by_lane` to return two lanes:
```rust
fn cards_by_lane(spec_state: &SpecState) -> Vec<LaneData> {
    let mut sort = |pred: &dyn Fn(&str) -> bool| -> Vec<CardData> {
        let mut v: Vec<CardData> = spec_state
            .cards
            .values()
            .filter(|c| pred(&c.lane))
            .map(CardData::from_card)
            .collect();
        v.sort_by(|a, b| a.order.partial_cmp(&b.order).unwrap_or(std::cmp::Ordering::Equal));
        v
    };
    vec![
        LaneData { name: "Parked".to_string(), key: "Ideas".to_string(), cards: sort(&|l| l == "Ideas") },
        LaneData { name: "In the spec".to_string(), key: "Spec".to_string(), cards: sort(&|l| l != "Ideas") },
    ]
}
```
- Fix every other `LaneData { … }` construction site the compiler flags (board/test fixtures) by adding a `key`.
- `board.html`: `data-lane="{{ lane.key }}"`; update the `<h3>` tooltip conditionals to the new labels:
```html
<h3 title="{% if lane.name == "Parked" %}Ideas you're not building — kept out of the spec.{% else %}Items that go into the spec handed to the build.{% endif %}">{{ lane.name }}</h3>
```

**Step 3:** `cargo test --all`, `cargo clippy` green. Manual: drag a card across columns on a running server and confirm it lands in the right column and the Spec output updates. **Commit:** `feat(ui): board shows two columns (Parked / In the spec)`.

---

## Task 3 — Drop the "by manager-…" line from cards

**Files:** `templates/partials/card.html:7`, `templates/partials/cards_feed.html:24`, `templates/partials/document.html:66`.

**What:** Remove the per-card author line:
- `card.html`: delete `<div class="card-meta">by {{ card.created_by }}</div>`.
- `cards_feed.html`: delete `<div class="card-feed-meta">by {{ card.created_by }} &middot; {{ card.updated_at }}</div>` (keep the timestamp only if a render test needs it — otherwise drop the whole line).
- `document.html`: delete `<p class="card-meta">by {{ card.created_by }}</p>`.

**Step 1:** if any render test asserts `"by "`/`created_by` text, update it to assert the line is **absent**. **Step 2:** make the deletions. **Step 3:** `cargo test --all` + `cargo clippy` green. **Commit:** `feat(ui): drop the by-agent line from cards`.

---

## Task 4 — Team mockup of the changes

After the build is green and runtime-verified, produce a shareable **"what changed in the refine step"** deck (same approach as `mockups/home-screen/shipped.html`): Before/After of landing-on-board, two columns, the cleaner cards, and one output. Publish as an artifact for Sophie to share.

---

## Final verification

- `cargo test --all` green; `cargo clippy --all-targets -- -D warnings` clean.
- Runtime: create a spec via API → open it in refining phase → confirm it opens on the **board**, shows **two columns**, cards have **no author line**, and the toggles are **Board + Spec** (no Document).
