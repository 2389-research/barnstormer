// ABOUTME: Exports a SpecState as a DOT graph for the DOT Runner constrained runtime DSL.
// ABOUTME: Generates digraph with lane-based flow edges and card-type-based node shapes.

use std::collections::BTreeMap;
use std::fmt::Write;

use crate::card::Card;
use crate::state::SpecState;

/// Export the spec state as a DOT graph conforming to the DOT Runner
/// constrained runtime DSL (spec Section 9.3).
///
/// Lane flow: Cards in "Ideas" get edges from start. Cards in "Done" get
/// edges to done. Cards in "Plan" connect between Ideas cards and Done cards.
/// Other lanes are placed between Plan and Done in alphabetical order.
pub fn export_dot(state: &SpecState) -> String {
    let mut out = String::new();

    let graph_name = state
        .core
        .as_ref()
        .map(|c| to_snake_case(&c.title))
        .unwrap_or_else(|| "unnamed_spec".to_string());

    let goal_label = state
        .core
        .as_ref()
        .map(|c| format!("{}: {}", c.title, c.one_liner))
        .unwrap_or_default();

    writeln!(out, "digraph {} {{", graph_name).unwrap();
    writeln!(
        out,
        "    graph [goal=\"{}\" rankdir=LR]",
        escape_dot_string(&goal_label)
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(out, "    start [shape=Mdiamond label=\"Start\"]").unwrap();
    writeln!(out, "    done [shape=Msquare label=\"Done\"]").unwrap();

    // Group cards by lane
    let cards_by_lane = group_cards_by_lane(state);

    // Emit nodes for all cards
    let all_cards: Vec<&Card> = state.cards.values().collect();
    if !all_cards.is_empty() {
        writeln!(out).unwrap();
    }

    // Emit nodes grouped by lane for readability
    let ordered_lanes = ordered_lane_names(state, &cards_by_lane);
    for lane in &ordered_lanes {
        if let Some(cards) = cards_by_lane.get(lane.as_str()) {
            for card in cards {
                let node_id = to_snake_case(&card.title);
                let shape = shape_for_card_type(&card.card_type);
                let mut attrs = format!(
                    "shape={} label=\"{}\"",
                    shape,
                    escape_dot_string(&card.title)
                );
                // Add type attribute for wait.human types
                if matches!(card.card_type.as_str(), "assumption" | "open_question") {
                    attrs.push_str(" type=\"wait.human\"");
                }
                writeln!(out, "    {} [{}]", node_id, attrs).unwrap();
            }
        }
    }

    // Emit edges based on lane flow
    writeln!(out).unwrap();

    let ideas_cards: Vec<String> = cards_by_lane
        .get("Ideas")
        .map(|cards| cards.iter().map(|c| to_snake_case(&c.title)).collect())
        .unwrap_or_default();

    let plan_cards: Vec<String> = cards_by_lane
        .get("Plan")
        .map(|cards| cards.iter().map(|c| to_snake_case(&c.title)).collect())
        .unwrap_or_default();

    let done_cards: Vec<String> = cards_by_lane
        .get("Done")
        .map(|cards| cards.iter().map(|c| to_snake_case(&c.title)).collect())
        .unwrap_or_default();

    // start -> Ideas cards
    for node_id in &ideas_cards {
        writeln!(out, "    start -> {}", node_id).unwrap();
    }

    // Ideas cards -> Plan cards (if both exist)
    if !ideas_cards.is_empty() && !plan_cards.is_empty() {
        for idea_id in &ideas_cards {
            for plan_id in &plan_cards {
                writeln!(out, "    {} -> {}", idea_id, plan_id).unwrap();
            }
        }
    }

    // Plan cards -> Done cards (if both exist)
    if !plan_cards.is_empty() && !done_cards.is_empty() {
        for plan_id in &plan_cards {
            for done_id in &done_cards {
                writeln!(out, "    {} -> {}", plan_id, done_id).unwrap();
            }
        }
    }

    // If there are no Plan cards, connect Ideas directly to Done
    if plan_cards.is_empty() && !ideas_cards.is_empty() && !done_cards.is_empty() {
        for idea_id in &ideas_cards {
            for done_id in &done_cards {
                writeln!(out, "    {} -> {}", idea_id, done_id).unwrap();
            }
        }
    }

    // Done cards -> done sentinel
    for node_id in &done_cards {
        writeln!(out, "    {} -> done", node_id).unwrap();
    }

    // Handle cards in custom lanes (non-default): connect between Plan and Done
    let custom_lane_cards: Vec<String> = ordered_lanes
        .iter()
        .filter(|l| !["Ideas", "Plan", "Done"].contains(&l.as_str()))
        .flat_map(|l| {
            cards_by_lane
                .get(l.as_str())
                .map(|cards| {
                    cards
                        .iter()
                        .map(|c| to_snake_case(&c.title))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        })
        .collect();

    // Custom lanes connect from Plan (or Ideas if no Plan) and to Done
    if !custom_lane_cards.is_empty() {
        let sources = if !plan_cards.is_empty() {
            &plan_cards
        } else {
            &ideas_cards
        };
        for src in sources {
            for custom in &custom_lane_cards {
                writeln!(out, "    {} -> {}", src, custom).unwrap();
            }
        }
        for custom in &custom_lane_cards {
            for done_id in &done_cards {
                writeln!(out, "    {} -> {}", custom, done_id).unwrap();
            }
        }
    }

    writeln!(out, "}}").unwrap();
    out
}

/// Map card type to DOT node shape per spec Section 9.3.
fn shape_for_card_type(card_type: &str) -> &'static str {
    match card_type {
        "idea" => "box",
        "plan" => "box",
        "task" => "box",
        "decision" => "diamond",
        "assumption" => "hexagon",
        "open_question" => "hexagon",
        "inspiration" | "vibes" => "parallelogram",
        _ => "box",
    }
}

/// Convert a string to snake_case for use as a DOT node identifier.
/// Strips non-alphanumeric characters (except underscores), lowercases,
/// and replaces spaces with underscores.
fn to_snake_case(s: &str) -> String {
    let mut result = String::new();
    let mut prev_was_separator = false;

    for ch in s.chars() {
        if ch.is_alphanumeric() {
            if ch.is_uppercase() {
                // Insert underscore before uppercase if not at start and previous wasn't separator
                if !result.is_empty()
                    && !prev_was_separator
                    && result.chars().last().is_some_and(|p| p.is_lowercase())
                {
                    result.push('_');
                }
                result.push(ch.to_lowercase().next().unwrap());
            } else {
                result.push(ch);
            }
            prev_was_separator = false;
        } else if (ch == ' ' || ch == '-' || ch == '_') && !result.is_empty() && !prev_was_separator
        {
            result.push('_');
            prev_was_separator = true;
        }
        // Skip other characters
    }

    // Trim trailing underscore
    if result.ends_with('_') {
        result.pop();
    }

    if result.is_empty() {
        "node".to_string()
    } else {
        result
    }
}

/// Escape a string for use within DOT quoted attributes.
fn escape_dot_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Group cards by lane name, sorting each group by (order, card_id).
fn group_cards_by_lane(state: &SpecState) -> BTreeMap<&str, Vec<&Card>> {
    let mut by_lane: BTreeMap<&str, Vec<&Card>> = BTreeMap::new();
    for card in state.cards.values() {
        by_lane.entry(card.lane.as_str()).or_default().push(card);
    }
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

    for dl in &default_lanes {
        let has_cards = cards_by_lane.contains_key(*dl);
        let is_default = state.lanes.contains(&dl.to_string());
        if has_cards || is_default {
            lanes.push(dl.to_string());
        }
    }

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

    fn make_state_with_core() -> SpecState {
        let core = SpecCore {
            spec_id: Ulid::new(),
            title: "Test Spec".to_string(),
            one_liner: "A test specification".to_string(),
            goal: "Verify the DOT exporter".to_string(),
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
            lanes: vec!["Ideas".to_string(), "Plan".to_string(), "Done".to_string()],
        }
    }

    fn make_card(card_type: &str, title: &str, lane: &str, order: f64, created_by: &str) -> Card {
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
    fn export_dot_has_start_and_done() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(dot.contains("start [shape=Mdiamond label=\"Start\"]"));
        assert!(dot.contains("done [shape=Msquare label=\"Done\"]"));
    }

    #[test]
    fn export_dot_cards_become_nodes() {
        let mut state = make_state_with_core();

        let card_a = make_card("idea", "My Idea", "Ideas", 1.0, "human");
        let card_b = make_card("task", "Build Feature", "Plan", 1.0, "human");
        let card_c = make_card("decision", "Choose Stack", "Ideas", 2.0, "human");

        state.cards.insert(card_a.card_id, card_a);
        state.cards.insert(card_b.card_id, card_b);
        state.cards.insert(card_c.card_id, card_c);

        let dot = export_dot(&state);

        // Each card should become a node with the correct shape
        assert!(
            dot.contains("my_idea [shape=box label=\"My Idea\"]"),
            "Expected my_idea node in:\n{}",
            dot
        );
        assert!(
            dot.contains("build_feature [shape=box label=\"Build Feature\"]"),
            "Expected build_feature node in:\n{}",
            dot
        );
        assert!(
            dot.contains("choose_stack [shape=diamond label=\"Choose Stack\"]"),
            "Expected choose_stack node in:\n{}",
            dot
        );
    }

    #[test]
    fn export_dot_edges_follow_lane_flow() {
        let mut state = make_state_with_core();

        let card_idea = make_card("idea", "Brainstorm", "Ideas", 1.0, "human");
        let card_plan = make_card("plan", "Roadmap", "Plan", 1.0, "human");
        let card_done = make_card("task", "Shipped", "Done", 1.0, "human");

        state.cards.insert(card_idea.card_id, card_idea);
        state.cards.insert(card_plan.card_id, card_plan);
        state.cards.insert(card_done.card_id, card_done);

        let dot = export_dot(&state);

        // start -> Ideas cards
        assert!(
            dot.contains("start -> brainstorm"),
            "Expected start -> brainstorm in:\n{}",
            dot
        );

        // Ideas -> Plan
        assert!(
            dot.contains("brainstorm -> roadmap"),
            "Expected brainstorm -> roadmap in:\n{}",
            dot
        );

        // Plan -> Done
        assert!(
            dot.contains("roadmap -> shipped"),
            "Expected roadmap -> shipped in:\n{}",
            dot
        );

        // Done -> done sentinel
        assert!(
            dot.contains("shipped -> done"),
            "Expected shipped -> done in:\n{}",
            dot
        );
    }

    #[test]
    fn export_dot_conforms_to_dsl() {
        let mut state = make_state_with_core();

        let card_assumption = make_card("assumption", "Users Want Speed", "Ideas", 1.0, "human");
        let card_vibes = make_card("vibes", "Good Energy", "Ideas", 2.0, "human");
        let card_open_q = make_card("open_question", "What Stack", "Plan", 1.0, "human");

        state.cards.insert(card_assumption.card_id, card_assumption);
        state.cards.insert(card_vibes.card_id, card_vibes);
        state.cards.insert(card_open_q.card_id, card_open_q);

        let dot = export_dot(&state);

        // Verify digraph declaration uses snake_case of title
        assert!(
            dot.starts_with("digraph test_spec {"),
            "Expected digraph test_spec {{ in:\n{}",
            dot
        );

        // Verify graph goal attribute
        assert!(
            dot.contains("goal=\"Test Spec: A test specification\""),
            "Expected goal attribute in:\n{}",
            dot
        );

        // Verify rankdir
        assert!(
            dot.contains("rankdir=LR"),
            "Expected rankdir=LR in:\n{}",
            dot
        );

        // Verify shapes: assumption -> hexagon with wait.human
        assert!(
            dot.contains(
                "users_want_speed [shape=hexagon label=\"Users Want Speed\" type=\"wait.human\"]"
            ),
            "Expected hexagon shape with wait.human for assumption in:\n{}",
            dot
        );

        // Verify shapes: vibes -> parallelogram
        assert!(
            dot.contains("good_energy [shape=parallelogram label=\"Good Energy\"]"),
            "Expected parallelogram shape for vibes in:\n{}",
            dot
        );

        // Verify shapes: open_question -> hexagon with wait.human
        assert!(
            dot.contains("what_stack [shape=hexagon label=\"What Stack\" type=\"wait.human\"]"),
            "Expected hexagon shape with wait.human for open_question in:\n{}",
            dot
        );

        // Verify valid DOT: opens and closes with braces
        assert!(dot.contains('{'));
        assert!(dot.trim().ends_with('}'));

        // All node IDs should be snake_case (no spaces, no uppercase in IDs)
        for line in dot.lines() {
            let trimmed = line.trim();
            // Skip non-node lines
            if trimmed.starts_with("digraph")
                || trimmed.starts_with("graph")
                || trimmed.starts_with("//")
                || trimmed.is_empty()
                || trimmed == "}"
                || trimmed.contains("->")
            {
                continue;
            }
            // For node definition lines, extract the node ID (first word)
            if let Some(node_id) = trimmed.split_whitespace().next() {
                // Node IDs should be lowercase with underscores only
                assert!(
                    node_id
                        .chars()
                        .all(|c| c.is_lowercase() || c == '_' || c.is_ascii_digit()),
                    "Node ID '{}' should be snake_case, found in line: {}",
                    node_id,
                    trimmed
                );
            }
        }
    }

    #[test]
    fn export_dot_direct_ideas_to_done_when_no_plan() {
        let mut state = make_state_with_core();

        let card_idea = make_card("idea", "Spark", "Ideas", 1.0, "human");
        let card_done = make_card("task", "Complete", "Done", 1.0, "human");

        state.cards.insert(card_idea.card_id, card_idea);
        state.cards.insert(card_done.card_id, card_done);

        let dot = export_dot(&state);

        // With no Plan cards, Ideas should connect directly to Done
        assert!(
            dot.contains("spark -> complete"),
            "Expected spark -> complete in:\n{}",
            dot
        );
        assert!(
            dot.contains("start -> spark"),
            "Expected start -> spark in:\n{}",
            dot
        );
        assert!(
            dot.contains("complete -> done"),
            "Expected complete -> done in:\n{}",
            dot
        );
    }

    #[test]
    fn to_snake_case_handles_various_inputs() {
        assert_eq!(to_snake_case("Hello World"), "hello_world");
        assert_eq!(to_snake_case("My Cool Idea"), "my_cool_idea");
        assert_eq!(to_snake_case("already_snake"), "already_snake");
        assert_eq!(to_snake_case("CamelCase"), "camel_case");
        assert_eq!(to_snake_case("with-dashes"), "with_dashes");
        assert_eq!(to_snake_case("  spaces  "), "spaces");
        assert_eq!(to_snake_case("Special!@#Chars"), "special_chars");
    }

    #[test]
    fn shape_for_card_type_maps_correctly() {
        assert_eq!(shape_for_card_type("idea"), "box");
        assert_eq!(shape_for_card_type("plan"), "box");
        assert_eq!(shape_for_card_type("task"), "box");
        assert_eq!(shape_for_card_type("decision"), "diamond");
        assert_eq!(shape_for_card_type("assumption"), "hexagon");
        assert_eq!(shape_for_card_type("open_question"), "hexagon");
        assert_eq!(shape_for_card_type("inspiration"), "parallelogram");
        assert_eq!(shape_for_card_type("vibes"), "parallelogram");
        assert_eq!(shape_for_card_type("unknown"), "box");
    }
}
