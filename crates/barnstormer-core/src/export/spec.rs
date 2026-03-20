// ABOUTME: Exports a SpecState as a synthesized Markdown specification document.
// ABOUTME: Groups cards from Plan+Spec lanes by card_type into semantic sections, excluding Ideas lane and idea card_type.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::card::Card;
use crate::state::SpecState;

/// Render a SpecState as a synthesized specification document in Markdown.
///
/// Cards from Plan and Spec lanes are grouped by `card_type` into semantic
/// sections (Requirements, Implementation Plan, etc.). Cards in the Ideas lane
/// and cards with `card_type` "idea" are excluded. Sections with no content
/// are omitted entirely. No metadata (timestamps, authors) is included.
pub fn export_spec(state: &SpecState) -> String {
    let mut out = String::new();

    let core = match state.core {
        Some(ref c) => c,
        None => return out,
    };

    // Header
    writeln!(out, "# {}", core.title).unwrap();
    writeln!(out).unwrap();
    writeln!(out, "> {}", core.one_liner).unwrap();

    // Collect eligible cards: only Plan and Spec lanes, excluding idea card_type
    let mut eligible: Vec<&Card> = state
        .cards
        .values()
        .filter(|c| (c.lane == "Plan" || c.lane == "Spec") && c.card_type != "idea")
        .collect();

    // Sort by order, then card_id as tiebreaker
    eligible.sort_by(|a, b| {
        a.order
            .partial_cmp(&b.order)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.card_id.cmp(&b.card_id))
    });

    let grouped = group_by_type(&eligible);

    // Section definitions: (section_title, core_field, card_type_key)
    let sections: Vec<(&str, Option<&str>, Option<&str>)> = vec![
        ("Goal", Some(&core.goal), None),
        ("Description", core.description.as_deref(), None),
        ("Requirements", None, Some("task")),
        ("Implementation Plan", None, Some("plan")),
        ("Design Decisions", None, Some("decision")),
        ("Constraints", core.constraints.as_deref(), Some("constraint")),
        ("Assumptions", None, Some("assumption")),
        ("Risks & Mitigations", core.risks.as_deref(), Some("risk")),
        ("Open Questions", None, Some("open_question")),
        ("Success Criteria", core.success_criteria.as_deref(), None),
        ("Notes", core.notes.as_deref(), None),
    ];

    for (title, core_field, card_type_key) in &sections {
        let cards_for_section: Vec<&&Card> = card_type_key
            .and_then(|key| grouped.get(key))
            .map(|v| v.iter().collect())
            .unwrap_or_default();

        let has_core = core_field.is_some();
        let has_cards = !cards_for_section.is_empty();

        // Special case: Goal always renders (it's a required field)
        if *title == "Goal" {
            writeln!(out).unwrap();
            writeln!(out, "## {}", title).unwrap();
            writeln!(out).unwrap();
            writeln!(out, "{}", core.goal).unwrap();
            continue;
        }

        if !has_core && !has_cards {
            continue;
        }

        writeln!(out).unwrap();
        writeln!(out, "## {}", title).unwrap();

        if let Some(field_text) = core_field {
            writeln!(out).unwrap();
            writeln!(out, "{}", field_text).unwrap();
        }

        for card in cards_for_section {
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

/// Group a slice of cards by their `card_type`, preserving the input order within each group.
fn group_by_type<'a>(cards: &[&'a Card]) -> BTreeMap<&'a str, Vec<&'a Card>> {
    let mut map: BTreeMap<&'a str, Vec<&'a Card>> = BTreeMap::new();
    for card in cards {
        map.entry(card.card_type.as_str()).or_default().push(card);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::Card;
    use crate::model::SpecCore;
    use crate::state::SpecPhase;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use ulid::Ulid;

    /// Build a minimal SpecState with a SpecCore and no cards.
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
            phase: SpecPhase::Active,
        }
    }

    /// Create a Card with specific fields for testing.
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
            created_by: "test".to_string(),
            updated_by: "test".to_string(),
        }
    }

    #[test]
    fn empty_state_returns_empty_string() {
        let state = SpecState::new();
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
        let mut card = make_card("task", "Must support auth", "Plan", 1.0);
        card.body = Some("OAuth2 required".to_string());
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(result.contains("## Requirements"));
        assert!(result.contains("### Must support auth"));
        assert!(result.contains("OAuth2 required"));
    }

    #[test]
    fn plan_cards_appear_under_implementation_plan() {
        let mut state = make_state_with_core();
        let card = make_card("plan", "Phase 1 rollout", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(result.contains("## Implementation Plan"));
        assert!(result.contains("### Phase 1 rollout"));
    }

    #[test]
    fn decision_cards_appear_under_design_decisions() {
        let mut state = make_state_with_core();
        let card = make_card("decision", "Use Postgres", "Spec", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(result.contains("## Design Decisions"));
        assert!(result.contains("### Use Postgres"));
    }

    #[test]
    fn constraint_cards_merge_with_core_constraints() {
        let mut state = make_state_with_core();
        if let Some(ref mut core) = state.core {
            core.constraints = Some("Must run on Linux".to_string());
        }
        let card = make_card("constraint", "Max 100ms latency", "Spec", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(result.contains("## Constraints"));
        assert!(result.contains("Must run on Linux"));
        assert!(result.contains("### Max 100ms latency"));

        // Core text should appear before the card
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
        let card = make_card("risk", "Vendor lock-in", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(result.contains("## Risks & Mitigations"));
        assert!(result.contains("Scope creep"));
        assert!(result.contains("### Vendor lock-in"));
    }

    #[test]
    fn assumption_cards_appear_under_assumptions() {
        let mut state = make_state_with_core();
        let card = make_card("assumption", "Users have internet", "Spec", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(result.contains("## Assumptions"));
        assert!(result.contains("### Users have internet"));
    }

    #[test]
    fn open_question_cards_appear_under_open_questions() {
        let mut state = make_state_with_core();
        let card = make_card("open_question", "Which DB engine?", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(result.contains("## Open Questions"));
        assert!(result.contains("### Which DB engine?"));
    }

    #[test]
    fn ideas_lane_cards_excluded() {
        let mut state = make_state_with_core();
        let card = make_card("task", "A task in Ideas", "Ideas", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(!result.contains("A task in Ideas"));
    }

    #[test]
    fn idea_card_type_excluded() {
        let mut state = make_state_with_core();
        // An idea card in the Plan lane should still be excluded
        let card = make_card("idea", "Random brainstorm", "Plan", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(!result.contains("Random brainstorm"));
    }

    #[test]
    fn sections_with_no_content_omitted() {
        let state = make_state_with_core();
        let result = export_spec(&state);

        // Only Goal should be present since core has no optional fields and no cards
        assert!(result.contains("## Goal"));
        assert!(!result.contains("## Description"));
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
            core.description = Some("Detailed description here".to_string());
            core.success_criteria = Some("All tests green".to_string());
            core.notes = Some("Remember to ship it".to_string());
        }

        let result = export_spec(&state);

        assert!(result.contains("## Description"));
        assert!(result.contains("Detailed description here"));
        assert!(result.contains("## Success Criteria"));
        assert!(result.contains("All tests green"));
        assert!(result.contains("## Notes"));
        assert!(result.contains("Remember to ship it"));
    }

    #[test]
    fn cards_sorted_by_order_within_section() {
        let mut state = make_state_with_core();

        let card_c = make_card("task", "Third task", "Plan", 3.0);
        let card_a = make_card("task", "First task", "Plan", 1.0);
        let card_b = make_card("task", "Second task", "Plan", 2.0);

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
        let card = make_card("task", "Clean card", "Spec", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(!result.contains("created_by"));
        assert!(!result.contains("Created by"));
        assert!(!result.contains("updated_by"));
        assert!(!result.contains("created_at"));
        assert!(!result.contains("updated_at"));
        assert!(!result.contains("card_id"));
        assert!(!result.contains("Refs:"));
    }

    #[test]
    fn custom_lane_cards_excluded() {
        let mut state = make_state_with_core();
        let card = make_card("task", "Task in custom lane", "Review", 1.0);
        state.cards.insert(card.card_id, card);

        let result = export_spec(&state);

        assert!(!result.contains("Task in custom lane"));
    }

    #[test]
    fn description_section_renders_when_present() {
        let mut state = make_state_with_core();
        if let Some(ref mut core) = state.core {
            core.description = Some("This is the full description.".to_string());
        }

        let result = export_spec(&state);

        assert!(result.contains("## Description"));
        assert!(result.contains("This is the full description."));

        // Description should come after Goal
        let goal_pos = result.find("## Goal").unwrap();
        let desc_pos = result.find("## Description").unwrap();
        assert!(goal_pos < desc_pos);
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

        let card_task = make_card("task", "A task", "Plan", 1.0);
        let card_plan = make_card("plan", "A plan", "Plan", 1.0);
        let card_decision = make_card("decision", "A decision", "Spec", 1.0);
        let card_assumption = make_card("assumption", "An assumption", "Spec", 1.0);
        let card_question = make_card("open_question", "A question", "Plan", 1.0);

        state.cards.insert(card_task.card_id, card_task);
        state.cards.insert(card_plan.card_id, card_plan);
        state.cards.insert(card_decision.card_id, card_decision);
        state.cards.insert(card_assumption.card_id, card_assumption);
        state.cards.insert(card_question.card_id, card_question);

        let result = export_spec(&state);

        let positions: Vec<usize> = [
            "## Goal",
            "## Description",
            "## Requirements",
            "## Implementation Plan",
            "## Design Decisions",
            "## Constraints",
            "## Assumptions",
            "## Risks & Mitigations",
            "## Open Questions",
            "## Success Criteria",
            "## Notes",
        ]
        .iter()
        .map(|s| result.find(s).expect(&format!("Section '{}' not found", s)))
        .collect();

        // Verify each section appears after the previous one
        for window in positions.windows(2) {
            assert!(
                window[0] < window[1],
                "Section ordering violated: position {} should be before {}",
                window[0],
                window[1]
            );
        }
    }
}
