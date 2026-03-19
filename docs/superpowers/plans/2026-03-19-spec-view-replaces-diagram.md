# Spec View Replaces Diagram Tab — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Diagram tab (DOT/Viz.js) with a synthesized Spec tab that groups cards by type into semantic sections.

**Architecture:** New `export_spec()` function in `barnstormer-core` groups cards from Plan+Spec lanes by card_type into semantic sections (Requirements, Implementation Plan, Design Decisions, etc.), merging with matching spec core fields. The server gets a new handler/template for the Spec tab, and the Diagram tab/Viz.js dependency are removed from the UI.

**Tech Stack:** Rust, Askama templates, HTMX, pulldown-cmark (already a dependency)

**Preserved (intentionally NOT removed):** The DOT exporter (`export/dot.rs`), its export route (`/web/specs/{id}/export/dot`), and `export_dot` function all remain in the codebase. Only the Diagram *tab* in the UI is replaced — the DOT export capability stays for download and export-to-disk flows.

---

### Task 1: Create `export_spec` module with tests

**Files:**
- Create: `crates/barnstormer-core/src/export/spec.rs`
- Modify: `crates/barnstormer-core/src/export/mod.rs:1-10`

- [ ] **Step 1: Write failing tests for `export_spec`**

Create the test module in `spec.rs` with all unit tests. The function doesn't exist yet so all tests will fail to compile.

```rust
// ABOUTME: Exports a SpecState as a synthesized specification document grouped by card type.
// ABOUTME: Cards from Plan and Spec lanes are organized into semantic sections like Requirements, Design Decisions, etc.

use std::fmt::Write;

use crate::card::Card;
use crate::state::SpecState;

/// Render a SpecState as a synthesized specification Markdown document.
///
/// Cards from the "Plan" and "Spec" lanes are grouped by `card_type` into
/// semantic sections. The "Ideas" lane and `idea` card_type are excluded.
/// Spec core fields are merged with matching card sections where both exist.
/// Sections with no content are omitted entirely.
pub fn export_spec(state: &SpecState) -> String {
    String::new() // stub — will be implemented in step 3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::Card;
    use crate::model::SpecCore;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use ulid::Ulid;

    fn make_state_with_core() -> SpecState {
        let core = SpecCore {
            spec_id: Ulid::new(),
            title: "Test Spec".to_string(),
            one_liner: "A test specification".to_string(),
            goal: "Build the thing".to_string(),
            description: None,
            constraints: None,
            success_criteria: None,
            risks: None,
            notes: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        SpecState {
            core: Some(core),
            cards: BTreeMap::new(),
            transcript: Vec::new(),
            pending_question: None,
            undo_stack: Vec::new(),
            last_event_id: 0,
            lanes: vec!["Ideas".to_string(), "Plan".to_string(), "Spec".to_string()],
        }
    }

    fn make_card(card_type: &str, title: &str, lane: &str, order: f64) -> Card {
        let now = Utc::now();
        Card {
            card_id: Ulid::new(),
            card_type: card_type.to_string(),
            title: title.to_string(),
            body: None,
            lane: lane.to_string(),
            order,
            refs: Vec::new(),
            created_at: now,
            updated_at: now,
            created_by: "human".to_string(),
            updated_by: "human".to_string(),
        }
    }

    #[test]
    fn empty_state_returns_empty_string() {
        let state = SpecState {
            core: None,
            cards: BTreeMap::new(),
            transcript: Vec::new(),
            pending_question: None,
            undo_stack: Vec::new(),
            last_event_id: 0,
            lanes: vec![],
        };
        let result = export_spec(&state);
        assert_eq!(result, "");
    }

    #[test]
    fn core_fields_render_header_and_goal() {
        let state = make_state_with_core();
        let result = export_spec(&state);
        assert!(result.contains("# Test Spec"));
        assert!(result.contains("> A test specification"));
        assert!(result.contains("## Goal"));
        assert!(result.contains("Build the thing"));
    }

    #[test]
    fn task_cards_appear_under_requirements() {
        let mut state = make_state_with_core();
        let mut card = make_card("task", "User login", "Plan", 1.0);
        card.body = Some("Users must be able to log in".to_string());
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(result.contains("## Requirements"));
        assert!(result.contains("### User login"));
        assert!(result.contains("Users must be able to log in"));
    }

    #[test]
    fn plan_cards_appear_under_implementation_plan() {
        let mut state = make_state_with_core();
        let card = make_card("plan", "Phase 1 rollout", "Spec", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(result.contains("## Implementation Plan"));
        assert!(result.contains("### Phase 1 rollout"));
    }

    #[test]
    fn decision_cards_appear_under_design_decisions() {
        let mut state = make_state_with_core();
        let card = make_card("decision", "Use PostgreSQL", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(result.contains("## Design Decisions"));
        assert!(result.contains("### Use PostgreSQL"));
    }

    #[test]
    fn constraint_cards_merge_with_core_constraints() {
        let mut state = make_state_with_core();
        if let Some(ref mut core) = state.core {
            core.constraints = Some("Must run on Linux".to_string());
        }
        let card = make_card("constraint", "Max 100ms latency", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(result.contains("## Constraints"));
        assert!(result.contains("Must run on Linux"));
        assert!(result.contains("### Max 100ms latency"));
        // Core prose should come before the card
        let core_pos = result.find("Must run on Linux").unwrap();
        let card_pos = result.find("### Max 100ms latency").unwrap();
        assert!(core_pos < card_pos);
    }

    #[test]
    fn risk_cards_merge_with_core_risks() {
        let mut state = make_state_with_core();
        if let Some(ref mut core) = state.core {
            core.risks = Some("Scope creep".to_string());
        }
        let card = make_card("risk", "API rate limits", "Spec", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(result.contains("## Risks & Mitigations"));
        assert!(result.contains("Scope creep"));
        assert!(result.contains("### API rate limits"));
    }

    #[test]
    fn assumption_cards_appear_under_assumptions() {
        let mut state = make_state_with_core();
        let card = make_card("assumption", "Users have modern browsers", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(result.contains("## Assumptions"));
        assert!(result.contains("### Users have modern browsers"));
    }

    #[test]
    fn open_question_cards_appear_under_open_questions() {
        let mut state = make_state_with_core();
        let card = make_card("open_question", "Which auth provider?", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(result.contains("## Open Questions"));
        assert!(result.contains("### Which auth provider?"));
    }

    #[test]
    fn ideas_lane_cards_excluded() {
        let mut state = make_state_with_core();
        let card = make_card("task", "Brainstorm idea", "Ideas", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(!result.contains("Brainstorm idea"));
    }

    #[test]
    fn idea_card_type_excluded() {
        let mut state = make_state_with_core();
        let card = make_card("idea", "Random thought", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(!result.contains("Random thought"));
    }

    #[test]
    fn sections_with_no_content_omitted() {
        let state = make_state_with_core();
        let result = export_spec(&state);
        // No cards and no optional core fields set, so these sections should not appear
        assert!(!result.contains("## Requirements"));
        assert!(!result.contains("## Implementation Plan"));
        assert!(!result.contains("## Design Decisions"));
        assert!(!result.contains("## Constraints"));
        assert!(!result.contains("## Assumptions"));
        assert!(!result.contains("## Risks & Mitigations"));
        assert!(!result.contains("## Open Questions"));
        assert!(!result.contains("## Success Criteria"));
        assert!(!result.contains("## Notes"));
    }

    #[test]
    fn core_only_sections_render_without_cards() {
        let mut state = make_state_with_core();
        if let Some(ref mut core) = state.core {
            core.success_criteria = Some("All tests pass".to_string());
            core.notes = Some("Check with stakeholders".to_string());
        }

        let result = export_spec(&state);
        assert!(result.contains("## Success Criteria"));
        assert!(result.contains("All tests pass"));
        assert!(result.contains("## Notes"));
        assert!(result.contains("Check with stakeholders"));
    }

    #[test]
    fn cards_sorted_by_order_within_section() {
        let mut state = make_state_with_core();
        let card_b = make_card("task", "Second task", "Plan", 2.0);
        let card_a = make_card("task", "First task", "Plan", 1.0);
        let card_c = make_card("task", "Third task", "Plan", 3.0);

        state.cards.insert(card_c.card_id, card_c);
        state.cards.insert(card_a.card_id, card_a);
        state.cards.insert(card_b.card_id, card_b);

        let result = export_spec(&state);
        let pos_first = result.find("### First task").unwrap();
        let pos_second = result.find("### Second task").unwrap();
        let pos_third = result.find("### Third task").unwrap();
        assert!(pos_first < pos_second);
        assert!(pos_second < pos_third);
    }

    #[test]
    fn no_metadata_clutter_in_card_output() {
        let mut state = make_state_with_core();
        let card = make_card("task", "Some task", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(!result.contains("Created by:"));
        assert!(!result.contains("human at"));
    }

    #[test]
    fn custom_lane_cards_excluded() {
        let mut state = make_state_with_core();
        let card = make_card("task", "Research task", "Research", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);
        assert!(!result.contains("Research task"));
    }

    #[test]
    fn description_section_renders_when_present() {
        let mut state = make_state_with_core();
        if let Some(ref mut core) = state.core {
            core.description = Some("A detailed description of the project".to_string());
        }

        let result = export_spec(&state);
        assert!(result.contains("## Description"));
        assert!(result.contains("A detailed description of the project"));
    }

    #[test]
    fn section_ordering_is_consistent() {
        let mut state = make_state_with_core();
        if let Some(ref mut core) = state.core {
            core.description = Some("Desc".to_string());
            core.constraints = Some("Constraint".to_string());
            core.success_criteria = Some("Criteria".to_string());
            core.risks = Some("Risk".to_string());
            core.notes = Some("Note".to_string());
        }
        let task = make_card("task", "A task", "Plan", 1.0);
        let plan = make_card("plan", "A plan", "Plan", 1.0);
        let decision = make_card("decision", "A decision", "Plan", 1.0);
        let assumption = make_card("assumption", "An assumption", "Plan", 1.0);
        let question = make_card("open_question", "A question", "Plan", 1.0);

        state.cards.insert(task.card_id, task);
        state.cards.insert(plan.card_id, plan);
        state.cards.insert(decision.card_id, decision);
        state.cards.insert(assumption.card_id, assumption);
        state.cards.insert(question.card_id, question);

        let result = export_spec(&state);

        // Verify section order: Goal, Description, Requirements, Implementation Plan,
        // Design Decisions, Constraints, Assumptions, Risks & Mitigations,
        // Open Questions, Success Criteria, Notes
        let positions: Vec<usize> = vec![
            result.find("## Goal").unwrap(),
            result.find("## Description").unwrap(),
            result.find("## Requirements").unwrap(),
            result.find("## Implementation Plan").unwrap(),
            result.find("## Design Decisions").unwrap(),
            result.find("## Constraints").unwrap(),
            result.find("## Assumptions").unwrap(),
            result.find("## Risks & Mitigations").unwrap(),
            result.find("## Open Questions").unwrap(),
            result.find("## Success Criteria").unwrap(),
            result.find("## Notes").unwrap(),
        ];

        for i in 0..positions.len() - 1 {
            assert!(positions[i] < positions[i + 1],
                "Section at index {} should come before section at index {}", i, i + 1);
        }
    }
}
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `cargo test -p barnstormer-core export::spec --no-run 2>&1 | head -5`
Expected: Compilation error — `export_spec` returns empty string, tests fail.

Actually first we need to register the module. But we'll do that together with the stub.

- [ ] **Step 3: Register the module in `mod.rs`**

Add `pub mod spec;` and `pub use spec::export_spec;` to `crates/barnstormer-core/src/export/mod.rs`.

```rust
// ABOUTME: Module root for spec state exporters (Markdown, YAML, DOT, Spec).
// ABOUTME: Re-exports all export functions for convenient access.

pub mod dot;
pub mod markdown;
pub mod spec;
pub mod yaml;

pub use dot::export_dot;
pub use markdown::export_markdown;
pub use spec::export_spec;
pub use yaml::export_yaml;
```

- [ ] **Step 4: Run tests to confirm they compile but fail**

Run: `cargo test -p barnstormer-core export::spec`
Expected: Most tests FAIL (empty string returned by stub).

- [ ] **Step 5: Implement `export_spec`**

Replace the stub with the real implementation:

```rust
/// Allowed lanes for spec export.
const SPEC_LANES: &[&str] = &["Plan", "Spec"];

/// Render a SpecState as a synthesized specification Markdown document.
///
/// Cards from the "Plan" and "Spec" lanes are grouped by `card_type` into
/// semantic sections. The "Ideas" lane and `idea` card_type are excluded.
/// Spec core fields are merged with matching card sections where both exist.
/// Sections with no content are omitted entirely.
pub fn export_spec(state: &SpecState) -> String {
    let mut out = String::new();

    let core = match &state.core {
        Some(c) => c,
        None => return out,
    };

    // Header
    writeln!(out, "# {}", core.title).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "> {}", core.one_liner).unwrap();

    // Collect and filter cards: only Plan+Spec lanes, exclude idea card_type
    let mut cards: Vec<&Card> = state
        .cards
        .values()
        .filter(|c| SPEC_LANES.contains(&c.lane.as_str()))
        .filter(|c| c.card_type != "idea")
        .collect();
    cards.sort_by(|a, b| {
        a.order
            .partial_cmp(&b.order)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.card_id.cmp(&b.card_id))
    });

    // Group cards by type
    let cards_by_type = group_by_type(&cards);

    // Sections in order, each with: (section_title, card_type_key, core_field)
    let sections: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("Goal", None, Some(&core.goal)),
        ("Description", None, core.description.as_deref()),
        ("Requirements", Some("task"), None),
        ("Implementation Plan", Some("plan"), None),
        ("Design Decisions", Some("decision"), None),
        ("Constraints", Some("constraint"), core.constraints.as_deref()),
        ("Assumptions", Some("assumption"), None),
        ("Risks & Mitigations", Some("risk"), core.risks.as_deref()),
        ("Open Questions", Some("open_question"), None),
        ("Success Criteria", None, core.success_criteria.as_deref()),
        ("Notes", None, core.notes.as_deref()),
    ];

    for (title, card_type_key, core_field) in &sections {
        let type_cards: Vec<&&Card> = card_type_key
            .and_then(|k| cards_by_type.get(k))
            .map(|v| v.iter().collect())
            .unwrap_or_default();

        let has_core = core_field.is_some();
        let has_cards = !type_cards.is_empty();

        if !has_core && !has_cards {
            continue;
        }

        writeln!(out).unwrap();
        writeln!(out, "## {}", title).unwrap();

        if let Some(field) = core_field {
            writeln!(out).unwrap();
            writeln!(out, "{}", field).unwrap();
        }

        for card in type_cards {
            writeln!(out).unwrap();
            writeln!(out, "### {}", card.title).unwrap();
            if let Some(ref body) = card.body {
                writeln!(out).unwrap();
                writeln!(out, "{}", body).unwrap();
            }
        }
    }

    out
}

/// Group card references by their card_type.
fn group_by_type<'a>(cards: &[&'a Card]) -> std::collections::BTreeMap<&'a str, Vec<&'a Card>> {
    let mut map: std::collections::BTreeMap<&str, Vec<&Card>> = std::collections::BTreeMap::new();
    for card in cards {
        map.entry(card.card_type.as_str()).or_default().push(card);
    }
    map
}
```

- [ ] **Step 6: Run tests to verify they all pass**

Run: `cargo test -p barnstormer-core export::spec`
Expected: All tests PASS.

- [ ] **Step 7: Run full test suite to check for regressions**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings`
Expected: All tests pass, no clippy warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/barnstormer-core/src/export/spec.rs crates/barnstormer-core/src/export/mod.rs
git commit -m "feat: add export_spec function for synthesized spec document"
```

---

### Task 2: Create Spec tab template and delete Diagram template

This task has no standalone tests — the template is verified in Task 3 via `SpecTabTemplate` render tests. A build check confirms the template compiles.

**Files:**
- Create: `templates/partials/spec.html`
- Delete: `templates/partials/diagram.html`

- [ ] **Step 1: Create the new spec template**

```html
{# ABOUTME: Spec tab that renders a synthesized specification document from cards grouped by type. #}
{# ABOUTME: Replaces the diagram tab; displays markdown-rendered spec with copy/download toolbar. #}

<div class="document spec-document">
    <div class="document-notice">
        <span>Synthesized from Plan &amp; Spec cards, grouped by type.</span>
        <button class="btn btn-sm btn-regen"
                hx-get="/web/specs/{{ spec_id }}/spec"
                hx-target="#canvas" hx-swap="innerHTML"
                title="Regenerate spec from current data">
            Regenerate
        </button>
        <button class="btn btn-sm btn-copy" id="spec-copy-md" title="Copy Markdown source">Copy Markdown</button>
        <a href="/web/specs/{{ spec_id }}/export/spec" download="spec.md" class="btn btn-sm btn-download">Download</a>
    </div>

    <div class="doc-content">
        {{ spec_html|safe }}
    </div>
</div>

<script>
    (function() {
        var md = {{ spec_markdown|json|safe }};
        var copyBtn = document.getElementById('spec-copy-md');
        if (copyBtn && md) {
            copyBtn.addEventListener('click', function() {
                navigator.clipboard.writeText(md).then(function() {
                    copyBtn.textContent = 'Copied!';
                    setTimeout(function() { copyBtn.textContent = 'Copy Markdown'; }, 2000);
                });
            });
        }
    })();
</script>
```

- [ ] **Step 2: Delete the diagram template**

```bash
rm templates/partials/diagram.html
```

- [ ] **Step 3: Verify build compiles with new template**

Run: `cargo build -p barnstormer-server 2>&1 | tail -5`
Expected: Build succeeds (template file exists for when `SpecTabTemplate` is added in Task 3). If `SpecTabTemplate` doesn't exist yet, this will still pass because no Rust code references the template file yet.

- [ ] **Step 4: Commit**

```bash
git add templates/partials/spec.html
git rm templates/partials/diagram.html
git commit -m "feat: add spec tab template, remove diagram template"
```

---

### Task 3: Wire up Spec handler, remove Diagram handler

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs:1417-1455` (replace DiagramTemplate + diagram handler)
- Modify: `crates/barnstormer-server/src/routes.rs:53` (change route)

- [ ] **Step 1: Write failing test for the spec handler**

In `crates/barnstormer-server/src/web/mod.rs`, **remove** the diagram test block (lines ~3710-3791: the `// ---- Diagram tab tests ----` comment through the `diagram_template_renders_empty_state` test). **Preserve** everything after it — especially the `create_test_spec` helper at line ~3796 and all subsequent tests. Then add the following spec tests in place of the removed diagram tests:

```rust
    // ---- Spec tab tests ----

    #[tokio::test]
    async fn spec_handler_returns_200() {
        let state = test_state();
        let spec_id = create_test_spec(&state).await;

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/spec", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("spec-document"),
            "spec response should contain spec-document class: {}",
            html
        );
    }

    #[tokio::test]
    async fn spec_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/spec", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }

    #[test]
    fn spec_template_renders_with_content() {
        let tmpl = SpecTabTemplate {
            spec_id: "01HTEST".to_string(),
            spec_html: "<h1>Test</h1>".to_string(),
            spec_markdown: "# Test".to_string(),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("spec-document"), "should contain spec-document class");
        assert!(rendered.contains("spec-copy-md"), "should contain copy markdown button");
    }

    #[test]
    fn spec_template_renders_empty_state() {
        let tmpl = SpecTabTemplate {
            spec_id: "01HTEST".to_string(),
            spec_html: String::new(),
            spec_markdown: String::new(),
        };
        let rendered = tmpl.render().unwrap();
        assert!(rendered.contains("spec-document"));
    }
```

- [ ] **Step 2: Run tests to confirm they fail**

Run: `cargo test -p barnstormer-server spec_handler --no-run 2>&1 | head -10`
Expected: Fails — `SpecTabTemplate` doesn't exist, `/spec` route doesn't exist.

- [ ] **Step 3: Replace DiagramTemplate with SpecTabTemplate and handler**

In `crates/barnstormer-server/src/web/mod.rs`, replace lines 1417-1455:

```rust
/// Spec tab template showing a synthesized specification document.
#[derive(Template, AskamaIntoResponse)]
#[template(path = "partials/spec.html")]
pub struct SpecTabTemplate {
    pub spec_id: String,
    pub spec_html: String,
    pub spec_markdown: String,
}

/// GET /web/specs/{id}/spec - Render the synthesized Spec tab.
pub async fn spec(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let spec_markdown = barnstormer_core::export::export_spec(&spec_state);
    let spec_html = render_markdown(&spec_markdown);

    SpecTabTemplate {
        spec_id: id,
        spec_html,
        spec_markdown,
    }
    .into_response()
}
```

- [ ] **Step 4: Add export_spec download handler**

Add after the `export_dot` function (search for `pub async fn export_dot` and place after its closing brace). Line numbers will have shifted from earlier edits in this task:

```rust
/// GET /web/specs/{id}/export/spec - Download synthesized spec as Markdown file.
pub async fn export_spec_download(
    State(state): State<SharedState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let spec_id = match parse_spec_id(&id) {
        Ok(id) => id,
        Err(resp) => return *resp,
    };

    let actors = state.actors.read().await;
    let handle = match actors.get(&spec_id) {
        Some(h) => h,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Html("<p class=\"error-msg\">Spec not found.</p>".to_string()),
            )
                .into_response();
        }
    };

    let spec_state = handle.read_state().await;
    let content = barnstormer_core::export::export_spec(&spec_state);

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/markdown; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"spec.md\"",
            ),
        ],
        content,
    )
        .into_response()
}
```

- [ ] **Step 5: Update routes**

In `crates/barnstormer-server/src/routes.rs`:

**Remove** line 53: `.route("/web/specs/{id}/diagram", get(web::diagram))`

**Add** in its place:
```rust
.route("/web/specs/{id}/spec", get(web::spec))
```

**Add** after the existing export routes (after `.route("/web/specs/{id}/export/dot", ...)`):
```rust
.route("/web/specs/{id}/export/spec", get(web::export_spec_download))
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p barnstormer-server`
Expected: All tests pass. Old diagram tests have been replaced with spec tests.

- [ ] **Step 7: Run full test suite**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings`
Expected: All pass, no warnings.

- [ ] **Step 8: Commit**

```bash
git add crates/barnstormer-server/src/web/mod.rs crates/barnstormer-server/src/routes.rs
git commit -m "feat: replace diagram handler with spec tab handler"
```

---

### Task 4: Update spec_view.html toggle and remove Viz.js

**Files:**
- Modify: `templates/partials/spec_view.html:25-30` (change Diagram button to Spec)
- Modify: `templates/base.html:25` (remove Viz.js script tag)

- [ ] **Step 1: Update the view toggle in spec_view.html**

Replace lines 25-30 (the Diagram toggle button):

```html
        <button class="view-toggle" data-view="spec"
                hx-get="/web/specs/{{ spec_id }}/spec"
                hx-target="#canvas" hx-swap="innerHTML">
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z"/><path d="M14 2v6h6"/><line x1="16" y1="13" x2="8" y2="13"/><line x1="16" y1="17" x2="8" y2="17"/><line x1="10" y1="9" x2="8" y2="9"/></svg>
            <span class="view-toggle-label">Spec</span>
        </button>
```

- [ ] **Step 2: Remove Viz.js from base.html**

Remove line 25 from `templates/base.html`:
```html
    <script src="https://cdn.jsdelivr.net/npm/@viz-js/viz@3.11.0/lib/viz-standalone.js"></script>
```

- [ ] **Step 3: Update the ABOUTME comment in base.html**

Change line 2 from:
```html
<!-- ABOUTME: Loads fonts, HTMX, SSE extension, SortableJS, and Viz.js. -->
```
to:
```html
<!-- ABOUTME: Loads fonts, HTMX, SSE extension, and SortableJS. -->
```

- [ ] **Step 4: Run full test suite**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings`
Expected: All pass. The diagram template tests were already replaced in Task 3.

- [ ] **Step 5: Commit**

```bash
git add templates/partials/spec_view.html templates/base.html
git commit -m "feat: update UI toggle to Spec tab, remove Viz.js dependency"
```

---

### Task 5: Add integration tests for spec export download

**Files:**
- Modify: `crates/barnstormer-server/src/web/mod.rs` (test section, after export_dot tests around line 3888)

- [ ] **Step 1: Write integration tests for the export_spec_download handler**

Add to the test module:

```rust
    #[tokio::test]
    async fn export_spec_returns_200_with_correct_headers() {
        let state = test_state();
        let spec_id = create_test_spec(&state).await;

        let app = create_router(Arc::clone(&state), None);
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/spec", spec_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);

        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("text/markdown"),
            "content-type should be text/markdown, got: {}",
            content_type
        );

        let disposition = resp
            .headers()
            .get("content-disposition")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            disposition.contains("spec.md"),
            "should offer spec.md download, got: {}",
            disposition
        );
    }

    #[tokio::test]
    async fn export_spec_for_nonexistent_spec_returns_404() {
        let state = test_state();
        let app = create_router(state, None);
        let fake_id = ulid::Ulid::new();
        let resp = app
            .oneshot(
                Request::get(format!("/web/specs/{}/export/spec", fake_id))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
    }
```

- [ ] **Step 2: Run the new tests**

Run: `cargo test -p barnstormer-server export_spec`
Expected: All pass.

- [ ] **Step 3: Run full test suite**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings`
Expected: All pass, no warnings.

- [ ] **Step 4: Commit**

```bash
git add crates/barnstormer-server/src/web/mod.rs
git commit -m "test: add integration tests for spec export download"
```

---

### Task 6: Clean up old diagram references

**Files:**
- Potentially: `crates/barnstormer-server/src/web/mod.rs` (any remaining diagram references)
- Potentially: `static/style.css` (diagram-specific CSS classes)

- [ ] **Step 1: Search for remaining diagram references**

Run: `grep -rn "diagram" crates/ templates/ static/ --include='*.rs' --include='*.html' --include='*.css' --include='*.js' | grep -v 'export_dot\|export/dot\|dot\.rs'`

Review results. Remove any dead diagram-specific CSS classes (`.diagram-panel`, `.diagram-container`, `.diagram-toolbar`, `.diagram-empty`, `.diagram-render`) from `static/style.css`.

- [ ] **Step 2: Clean up any found references**

Remove diagram-specific CSS rules. Keep anything still used by other parts of the app.

- [ ] **Step 3: Run full test suite one final time**

Run: `cargo test --all && cargo clippy --all-targets -- -D warnings`
Expected: All pass, no warnings.

- [ ] **Step 4: Commit**

```bash
git add static/style.css
git commit -m "chore: remove leftover diagram CSS and references"
```

Note: Only stage the specific files that were actually modified (e.g., `static/style.css`). If other files were also cleaned up, add them by name.
