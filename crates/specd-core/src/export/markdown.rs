// ABOUTME: Exports a SpecState as a deterministic Markdown document.
// ABOUTME: Sections follow spec Section 9.1 ordering: header, optional fields, then lanes with cards.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::card::Card;
use crate::state::SpecState;

/// Render a SpecState as a Markdown string with deterministic ordering.
///
/// Lane ordering: Ideas, Plan, Done first (in that order), then any other
/// lanes sorted alphabetically. Cards within each lane are ordered by their
/// `order` field (f64), with `card_id` as a tiebreaker.
pub fn export_markdown(state: &SpecState) -> String {
    let mut out = String::new();

    if let Some(ref core) = state.core {
        writeln!(out, "# {}", core.title).unwrap();
        writeln!(out).unwrap();
        writeln!(out, "> {}", core.one_liner).unwrap();
        writeln!(out).unwrap();
        writeln!(out, "## Goal").unwrap();
        writeln!(out).unwrap();
        writeln!(out, "{}", core.goal).unwrap();

        if let Some(ref description) = core.description {
            writeln!(out).unwrap();
            writeln!(out, "## Description").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "{}", description).unwrap();
        }

        if let Some(ref constraints) = core.constraints {
            writeln!(out).unwrap();
            writeln!(out, "## Constraints").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "{}", constraints).unwrap();
        }

        if let Some(ref success_criteria) = core.success_criteria {
            writeln!(out).unwrap();
            writeln!(out, "## Success Criteria").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "{}", success_criteria).unwrap();
        }

        if let Some(ref risks) = core.risks {
            writeln!(out).unwrap();
            writeln!(out, "## Risks").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "{}", risks).unwrap();
        }

        if let Some(ref notes) = core.notes {
            writeln!(out).unwrap();
            writeln!(out, "## Notes").unwrap();
            writeln!(out).unwrap();
            writeln!(out, "{}", notes).unwrap();
        }
    }

    // Group cards by lane
    let cards_by_lane = group_cards_by_lane(state);

    // Determine which lanes to show: default lanes always, plus any lane that has cards
    let ordered_lanes = ordered_lane_names(state, &cards_by_lane);

    if !ordered_lanes.is_empty() {
        writeln!(out).unwrap();
        writeln!(out, "---").unwrap();

        for lane in &ordered_lanes {
            writeln!(out).unwrap();
            writeln!(out, "## {}", lane).unwrap();

            if let Some(cards) = cards_by_lane.get(lane.as_str()) {
                for card in cards {
                    writeln!(out).unwrap();
                    writeln!(out, "### {} ({})", card.title, card.card_type).unwrap();

                    if let Some(ref body) = card.body {
                        writeln!(out).unwrap();
                        writeln!(out, "{}", body).unwrap();
                    }

                    if !card.refs.is_empty() {
                        writeln!(out).unwrap();
                        writeln!(out, "Refs: {}", card.refs.join(", ")).unwrap();
                    }

                    writeln!(
                        out,
                        "Created by: {} at {}",
                        card.created_by,
                        card.created_at.format("%Y-%m-%dT%H:%M:%SZ")
                    )
                    .unwrap();
                }
            }
        }
    }

    out
}

/// Group cards by lane name, sorting each group by (order, card_id).
fn group_cards_by_lane(state: &SpecState) -> BTreeMap<&str, Vec<&Card>> {
    let mut by_lane: BTreeMap<&str, Vec<&Card>> = BTreeMap::new();
    for card in state.cards.values() {
        by_lane.entry(card.lane.as_str()).or_default().push(card);
    }
    // Sort each lane's cards by order, then card_id as tiebreaker
    for cards in by_lane.values_mut() {
        cards.sort_by(|a, b| {
            a.order
                .partial_cmp(&b.order)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.card_id.cmp(&b.card_id))
        });
    }
    by_lane
}

/// Produce the ordered list of lane names: Ideas, Plan, Done first,
/// then any additional lanes sorted alphabetically.
fn ordered_lane_names(
    state: &SpecState,
    cards_by_lane: &BTreeMap<&str, Vec<&Card>>,
) -> Vec<String> {
    let default_lanes = ["Ideas", "Plan", "Done"];
    let mut lanes: Vec<String> = Vec::new();

    // Add default lanes that either are in state.lanes or have cards
    for dl in &default_lanes {
        let has_cards = cards_by_lane.contains_key(*dl);
        let is_default = state.lanes.contains(&dl.to_string());
        if has_cards || is_default {
            lanes.push(dl.to_string());
        }
    }

    // Collect additional non-default lanes that have cards, sorted alphabetically
    let mut extra_lanes: Vec<String> = cards_by_lane
        .keys()
        .filter(|k| !default_lanes.contains(k))
        .map(|k| k.to_string())
        .collect();
    extra_lanes.sort();

    lanes.extend(extra_lanes);
    lanes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::card::Card;
    use crate::model::SpecCore;
    use chrono::Utc;
    use std::collections::BTreeMap;
    use ulid::Ulid;

    /// Build a minimal SpecState with a SpecCore and no cards.
    fn make_state_with_core() -> SpecState {
        let core = SpecCore {
            spec_id: Ulid::new(),
            title: "Test Spec".to_string(),
            one_liner: "A test specification".to_string(),
            goal: "Verify the markdown exporter".to_string(),
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
            lanes: vec![
                "Ideas".to_string(),
                "Plan".to_string(),
                "Done".to_string(),
            ],
        }
    }

    /// Create a Card with specific fields for testing.
    fn make_card(
        card_type: &str,
        title: &str,
        lane: &str,
        order: f64,
        created_by: &str,
    ) -> Card {
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
            created_by: created_by.to_string(),
            updated_by: created_by.to_string(),
        }
    }

    #[test]
    fn export_markdown_includes_title_and_goal() {
        let state = make_state_with_core();
        let md = export_markdown(&state);

        assert!(md.contains("# Test Spec"));
        assert!(md.contains("> A test specification"));
        assert!(md.contains("## Goal"));
        assert!(md.contains("Verify the markdown exporter"));
    }

    #[test]
    fn export_markdown_groups_cards_by_lane() {
        let mut state = make_state_with_core();

        let card_ideas = make_card("idea", "Brainstorm", "Ideas", 1.0, "human");
        let card_plan = make_card("plan", "Roadmap", "Plan", 1.0, "human");
        let card_done = make_card("task", "Shipped", "Done", 1.0, "human");

        state.cards.insert(card_ideas.card_id, card_ideas);
        state.cards.insert(card_plan.card_id, card_plan);
        state.cards.insert(card_done.card_id, card_done);

        let md = export_markdown(&state);

        // Verify lane sections exist
        assert!(md.contains("## Ideas"));
        assert!(md.contains("## Plan"));
        assert!(md.contains("## Done"));

        // Verify cards are under the correct lane by checking ordering in the output
        let ideas_pos = md.find("## Ideas").unwrap();
        let plan_pos = md.find("## Plan").unwrap();
        let done_pos = md.find("## Done").unwrap();
        let brainstorm_pos = md.find("### Brainstorm (idea)").unwrap();
        let roadmap_pos = md.find("### Roadmap (plan)").unwrap();
        let shipped_pos = md.find("### Shipped (task)").unwrap();

        assert!(brainstorm_pos > ideas_pos && brainstorm_pos < plan_pos);
        assert!(roadmap_pos > plan_pos && roadmap_pos < done_pos);
        assert!(shipped_pos > done_pos);
    }

    #[test]
    fn export_markdown_orders_cards_by_order_field() {
        let mut state = make_state_with_core();

        let card_b = make_card("idea", "Second Idea", "Ideas", 2.0, "human");
        let card_a = make_card("idea", "First Idea", "Ideas", 1.0, "human");
        let card_c = make_card("idea", "Third Idea", "Ideas", 3.0, "human");

        // Insert in non-sorted order
        state.cards.insert(card_c.card_id, card_c);
        state.cards.insert(card_a.card_id, card_a);
        state.cards.insert(card_b.card_id, card_b);

        let md = export_markdown(&state);

        let pos_first = md.find("### First Idea").unwrap();
        let pos_second = md.find("### Second Idea").unwrap();
        let pos_third = md.find("### Third Idea").unwrap();

        assert!(pos_first < pos_second);
        assert!(pos_second < pos_third);
    }

    #[test]
    fn export_markdown_includes_optional_fields() {
        let mut state = make_state_with_core();
        if let Some(ref mut core) = state.core {
            core.description = Some("A detailed description".to_string());
            core.constraints = Some("Must be fast".to_string());
            core.success_criteria = Some("All tests pass".to_string());
            core.risks = Some("Scope creep".to_string());
            core.notes = Some("Remember to review".to_string());
        }

        let md = export_markdown(&state);

        assert!(md.contains("## Description"));
        assert!(md.contains("A detailed description"));
        assert!(md.contains("## Constraints"));
        assert!(md.contains("Must be fast"));
        assert!(md.contains("## Success Criteria"));
        assert!(md.contains("All tests pass"));
        assert!(md.contains("## Risks"));
        assert!(md.contains("Scope creep"));
        assert!(md.contains("## Notes"));
        assert!(md.contains("Remember to review"));
    }

    #[test]
    fn export_markdown_omits_empty_optional_fields() {
        let state = make_state_with_core();
        let md = export_markdown(&state);

        // Optional fields are None, so their sections should not appear
        assert!(!md.contains("## Description"));
        assert!(!md.contains("## Constraints"));
        assert!(!md.contains("## Success Criteria"));
        assert!(!md.contains("## Risks"));
        assert!(!md.contains("## Notes"));
    }

    #[test]
    fn export_markdown_deterministic() {
        let mut state = make_state_with_core();

        let card_a = make_card("idea", "Alpha", "Ideas", 1.0, "human");
        let card_b = make_card("task", "Beta", "Plan", 2.0, "agent");

        state.cards.insert(card_a.card_id, card_a);
        state.cards.insert(card_b.card_id, card_b);

        let md1 = export_markdown(&state);
        let md2 = export_markdown(&state);

        assert_eq!(md1, md2, "Markdown export must be deterministic");
    }

    #[test]
    fn export_markdown_extra_lanes_appear_alphabetically_after_defaults() {
        let mut state = make_state_with_core();

        let card_z = make_card("idea", "Zulu Card", "Zulu", 1.0, "human");
        let card_a = make_card("idea", "Alpha Card", "Alpha", 1.0, "human");
        let card_ideas = make_card("idea", "Idea Card", "Ideas", 1.0, "human");

        state.cards.insert(card_z.card_id, card_z);
        state.cards.insert(card_a.card_id, card_a);
        state.cards.insert(card_ideas.card_id, card_ideas);

        let md = export_markdown(&state);

        let ideas_pos = md.find("## Ideas").unwrap();
        let plan_pos = md.find("## Plan").unwrap();
        let done_pos = md.find("## Done").unwrap();
        let alpha_pos = md.find("## Alpha").unwrap();
        let zulu_pos = md.find("## Zulu").unwrap();

        // Default lanes first in order
        assert!(ideas_pos < plan_pos);
        assert!(plan_pos < done_pos);
        // Extra lanes alphabetically after defaults
        assert!(done_pos < alpha_pos);
        assert!(alpha_pos < zulu_pos);
    }

    #[test]
    fn export_markdown_card_with_body_and_refs() {
        let mut state = make_state_with_core();

        let mut card = make_card("idea", "Rich Card", "Ideas", 1.0, "human");
        card.body = Some("This card has a body.".to_string());
        card.refs = vec!["ref-1".to_string(), "ref-2".to_string()];
        state.cards.insert(card.card_id, card);

        let md = export_markdown(&state);

        assert!(md.contains("This card has a body."));
        assert!(md.contains("Refs: ref-1, ref-2"));
        assert!(md.contains("Created by: human at"));
    }
}
