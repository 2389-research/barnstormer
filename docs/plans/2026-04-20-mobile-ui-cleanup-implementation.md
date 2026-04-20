# Mobile UI Cleanup Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make the barnstormer web UI fully usable on mobile (≤900px) with hamburger sidebar, mobile tabs, responsive stepper, subtitle popover, and active view label.

**Architecture:** CSS-first with minimal JS. All changes gate behind the existing `@media (max-width: 900px)` breakpoint. Interactive behaviors (hamburger, subtitle popover, mobile tab switcher) use vanilla JS with class toggles. No new server endpoints or Askama template restructuring needed.

**Tech Stack:** CSS media queries, vanilla JS, Askama templates, HTMX

---

### Task 1: Hamburger menu + sidebar drawer

Add a hamburger button (mobile only) to the command bar. Clicking opens the nav-rail as a slide-in overlay from the left with a backdrop. The existing horizontal-strip mobile nav is replaced with a hidden drawer.

**Files:**
- Modify: `templates/base.html:27-30`
- Modify: `static/style.css:1689-1714` (mobile media query)
- Modify: `static/style.css:72-85` (app-layout, nav-rail base styles)

**Step 1: Add hamburger button and backdrop to base.html**

In `templates/base.html`, add a hamburger button inside `.app-layout` before the nav-rail, and a backdrop div after the nav-rail:

```html
<div class="app-layout">
    <button class="hamburger" onclick="document.querySelector('.app-layout').classList.toggle('nav-open')" aria-label="Toggle navigation">
        <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><line x1="3" y1="6" x2="21" y2="6"/><line x1="3" y1="12" x2="21" y2="12"/><line x1="3" y1="18" x2="21" y2="18"/></svg>
    </button>
    <nav class="nav-rail">{% block nav %}{% endblock %}</nav>
    <div class="nav-backdrop" onclick="document.querySelector('.app-layout').classList.remove('nav-open')"></div>
    <div class="workspace" id="workspace">{% block workspace %}{% endblock %}</div>
</div>
```

**Step 2: Add hamburger and drawer CSS**

Add to `static/style.css` — base styles (hidden on desktop):

```css
/* Hamburger menu — hidden on desktop, shown on mobile */
.hamburger {
    display: none;
    position: fixed;
    top: 12px;
    left: 12px;
    z-index: 51;
    border: none;
    background: var(--bg-card);
    border-radius: 8px;
    padding: 8px;
    cursor: pointer;
    color: var(--text-primary);
    box-shadow: 0 1px 4px rgba(0, 0, 0, 0.1);
}

/* Backdrop for mobile nav drawer */
.nav-backdrop {
    display: none;
}
```

Inside `@media (max-width: 900px)`:

```css
.hamburger {
    display: flex;
}

.nav-rail {
    position: fixed;
    top: 0;
    left: 0;
    bottom: 0;
    width: 80vw;
    max-width: 320px;
    z-index: 50;
    transform: translateX(-100%);
    transition: transform 0.25s ease;
    border-right: 1px solid var(--border);
    border-bottom: none;
    max-height: none;
}

.app-layout.nav-open .nav-rail {
    transform: translateX(0);
}

.nav-backdrop {
    display: none;
    position: fixed;
    inset: 0;
    z-index: 49;
    background: rgba(0, 0, 0, 0.3);
}

.app-layout.nav-open .nav-backdrop {
    display: block;
}

/* Offset command bar for hamburger button */
.command-bar {
    padding-left: 52px;
}
```

Remove the old mobile nav-rail rules (the horizontal strip behavior: `border-bottom`, `max-height: 200px`).

**Step 3: Update mobile app-layout grid**

The mobile grid no longer needs a row for the nav. Change to:

```css
.app-layout {
    grid-template-columns: 1fr;
    grid-template-rows: 1fr;
}
```

**Step 4: Run tests**

```bash
cargo test --all
```

All existing tests should still pass (template rendering tests don't test mobile CSS).

**Step 5: Visual verification**

Open http://127.0.0.1:7331 in a 390px-wide viewport. Verify:
- Hamburger icon visible top-left
- Clicking opens sidebar drawer from left with backdrop
- Clicking backdrop closes drawer
- Sidebar takes ~80% width, full height
- Desktop view unchanged (hamburger hidden)

**Step 6: Commit**

```bash
git add templates/base.html static/style.css
git commit -m "feat: add hamburger menu and sidebar drawer for mobile"
```

---

### Task 2: Subtitle popover (mobile only)

Make the command bar title tappable on mobile. Clicking shows a floating popover with the subtitle text. Tap outside or tap title again to dismiss.

**Files:**
- Modify: `templates/partials/spec_view.html:9-14,100-105,222-227` (all three command-bar instances)
- Modify: `static/style.css` (add popover styles)

**Step 1: Add popover markup to command bar**

In each of the three `<header class="command-bar">` blocks in `spec_view.html`, wrap the title in a clickable container with a popover:

```html
<div class="command-bar-left">
    <span class="command-bar-title" onclick="this.parentElement.classList.toggle('popover-open')">{{ title }}</span>
    <span class="command-bar-chevron">&#8250;</span>
    <span class="command-bar-subtitle">{{ one_liner }}</span>
    <div class="command-bar-popover">{{ one_liner }}</div>
</div>
```

**Step 2: Add popover CSS**

Add base styles (hidden by default):

```css
.command-bar-popover {
    display: none;
}
```

Inside `@media (max-width: 900px)`:

```css
.command-bar-title {
    cursor: pointer;
}

.command-bar-popover {
    display: none;
    position: absolute;
    top: 100%;
    left: 0;
    right: 0;
    padding: 8px 16px;
    background: var(--bg-card);
    border-bottom: 1px solid var(--border);
    font-size: 13px;
    color: var(--text-secondary);
    z-index: 5;
}

.command-bar-left {
    position: relative;
}

.command-bar-left.popover-open .command-bar-popover {
    display: block;
}
```

**Step 3: Add click-outside dismiss**

Add a small script in `spec_view.html` (can be added to any of the existing `<script>` blocks):

```javascript
document.addEventListener('click', function(e) {
    if (!e.target.closest('.command-bar-left')) {
        document.querySelectorAll('.command-bar-left.popover-open').forEach(function(el) {
            el.classList.remove('popover-open');
        });
    }
});
```

**Step 4: Run tests**

```bash
cargo test --all
```

**Step 5: Visual verification**

At 390px width: tap title, popover appears below with subtitle text. Tap outside, it closes. On desktop, title is not clickable, popover hidden, subtitle visible inline as before.

**Step 6: Commit**

```bash
git add templates/partials/spec_view.html static/style.css
git commit -m "feat: add subtitle popover for mobile command bar"
```

---

### Task 3: Mobile tab switcher for refining mode

On mobile, replace the side-by-side canvas + chat rail with two tabs: the active view name and "Chat". CSS toggles visibility; a small tab bar sits below the view toggles row.

**Files:**
- Modify: `templates/partials/spec_view.html:96-155` (refining phase block)
- Modify: `static/style.css` (add mobile tab styles)

**Step 1: Add mobile tab bar markup**

In the refining phase block of `spec_view.html`, add a tab bar between the view-toggles-row and spec-body:

```html
<div class="mobile-content-tabs">
    <button class="mobile-tab active" data-target="canvas" onclick="switchMobileTab(this, 'canvas')">Content</button>
    <button class="mobile-tab" data-target="chat" onclick="switchMobileTab(this, 'chat')">Chat</button>
</div>
```

**Step 2: Add tab switcher JS**

Add to the refining phase `<script>` block:

```javascript
function switchMobileTab(btn, target) {
    document.querySelectorAll('.mobile-tab').forEach(function(t) { t.classList.remove('active'); });
    btn.classList.add('active');
    var body = document.querySelector('.spec-body');
    if (target === 'chat') {
        body.classList.add('show-chat');
    } else {
        body.classList.remove('show-chat');
    }
}
```

**Step 3: Add mobile tab CSS**

Base styles (hidden on desktop):

```css
.mobile-content-tabs {
    display: none;
}
```

Inside `@media (max-width: 900px)`:

```css
.mobile-content-tabs {
    display: flex;
    border-bottom: 1px solid var(--border);
    background: var(--bg-card);
    flex-shrink: 0;
}

.mobile-tab {
    flex: 1;
    padding: 10px;
    border: none;
    background: none;
    font-size: 13px;
    font-weight: 500;
    font-family: var(--font-body);
    color: var(--text-muted);
    cursor: pointer;
    border-bottom: 2px solid transparent;
    transition: all 0.2s;
}

.mobile-tab.active {
    color: var(--text-primary);
    border-bottom-color: var(--text-primary);
}

/* Canvas/chat toggle on mobile */
.spec-body .canvas {
    display: flex;
}
.spec-body .chat-rail {
    display: none;
}
.spec-body.show-chat .canvas {
    display: none;
}
.spec-body.show-chat .chat-rail {
    display: flex;
    width: 100%;
    border-left: none;
}
```

**Step 4: Run tests**

```bash
cargo test --all
```

**Step 5: Visual verification**

At 390px: two tabs visible below view toggles — "Content" (active, showing document) and "Chat". Tap Chat to see the chat rail full-width. Tap Content to go back. Desktop: tabs hidden, side-by-side layout unchanged.

**Step 6: Commit**

```bash
git add templates/partials/spec_view.html static/style.css
git commit -m "feat: add mobile content/chat tab switcher for refining mode"
```

---

### Task 4: Responsive phase stepper

On mobile, hide connector lines and non-active step labels. Show only step numbers for inactive steps and number + label for the active step.

**Files:**
- Modify: `static/style.css` (add mobile stepper rules inside media query)

**Step 1: Add mobile stepper CSS**

Inside `@media (max-width: 900px)`:

```css
.phase-stepper {
    gap: 0;
    padding: 8px 16px;
}

.phase-step-connector {
    width: 24px;
}

.phase-step {
    padding: 4px 8px;
}

.phase-step-label {
    display: none;
}

.phase-step.step-active .phase-step-label {
    display: inline;
}
```

**Step 2: Run tests**

```bash
cargo test --all
```

**Step 3: Visual verification**

At 390px: stepper shows `(✓) —— Refine —— (3)` — compact, no overflow. Active step shows label, inactive steps show only the number circle. On desktop, unchanged.

**Step 4: Commit**

```bash
git add static/style.css
git commit -m "style: responsive phase stepper — compact labels on mobile"
```

---

### Task 5: View toggles — show active label

On mobile, keep icons-only for inactive toggles but show the label for the active toggle.

**Files:**
- Modify: `static/style.css:210-217` (view-toggle-label rules)

**Step 1: Update view toggle label CSS**

Replace the current toggle-label rules:

```css
.view-toggle-label {
    display: none;
}
@media (min-width: 901px) {
    .view-toggle-label {
        display: inline;
    }
}
```

With:

```css
.view-toggle-label {
    display: none;
}
.view-toggle.active .view-toggle-label {
    display: inline;
}
@media (min-width: 901px) {
    .view-toggle-label {
        display: inline;
    }
}
```

This shows the label for the active toggle at all screen sizes, while keeping inactive labels desktop-only.

**Step 2: Run tests**

```bash
cargo test --all
```

**Step 3: Visual verification**

At 390px: active toggle shows icon + label (e.g., `[doc-icon Document]`), inactive show icon only. On desktop, all labels visible (unchanged).

**Step 4: Commit**

```bash
git add static/style.css
git commit -m "style: show active view toggle label on mobile"
```
