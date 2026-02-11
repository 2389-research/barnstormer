// ABOUTME: Exports a SpecState as a DOT graph for the DOT Runner constrained runtime DSL.
// ABOUTME: Synthesizes cards into a fixed 10-phase pipeline with TDD and scenario testing gates.

use std::fmt::Write;

use crate::card::Card;
use crate::state::SpecState;

/// Maximum character length for synthesized prompts before truncation.
const MAX_PROMPT_LEN: usize = 500;

/// Export the spec state as a DOT graph conforming to the DOT Runner
/// constrained runtime DSL.
///
/// Produces a fixed pipeline of 10 phases with TDD enforcement and
/// scenario-driven validation. Card data is aggregated into each phase's
/// prompt rather than mapped 1:1 to nodes.
///
/// ```text
/// start -> plan -> setup -> tdd -> implement -> verify -> verify_ok
/// verify_ok -> scenario_test [Pass] | implement [Fail]
/// scenario_test -> scenario_ok
/// scenario_ok -> review_gate [Pass] | tdd [Fail]
/// review_gate -> release [Approve] | polish [Fix]
/// polish -> tdd
/// release -> done
/// ```
///
/// Card-type-to-phase mapping:
/// - plan: ideas, constraints, spec_constraints
/// - tdd: tasks, plans (write failing tests first)
/// - implement: tasks, plans (make the tests pass)
/// - verify: decisions, success_criteria (run unit/integration tests)
/// - scenario_test: assumptions, success_criteria (real deps, no mocks)
/// - review_gate: open_questions (human must decide)
/// - polish: risks
pub fn export_dot(state: &SpecState) -> String {
    let mut out = String::new();

    let graph_name = state
        .core
        .as_ref()
        .map(|c| to_snake_case(&c.title))
        .unwrap_or_else(|| "unnamed_spec".to_string());

    let goal = state
        .core
        .as_ref()
        .map(|c| {
            if c.goal.is_empty() {
                format!("{}: {}", c.title, c.one_liner)
            } else {
                c.goal.clone()
            }
        })
        .unwrap_or_default();

    let spec_constraints = state
        .core
        .as_ref()
        .and_then(|c| c.constraints.as_deref())
        .unwrap_or("");

    let success_criteria = state
        .core
        .as_ref()
        .and_then(|c| c.success_criteria.as_deref())
        .unwrap_or("");

    // Collect cards by type
    let cards: Vec<&Card> = state.cards.values().collect();
    let ideas: Vec<&str> = cards
        .iter()
        .filter(|c| c.card_type == "idea" || c.card_type == "inspiration" || c.card_type == "vibes")
        .map(|c| c.title.as_str())
        .collect();
    let tasks: Vec<&str> = cards
        .iter()
        .filter(|c| c.card_type == "task")
        .map(|c| c.title.as_str())
        .collect();
    let plans: Vec<&str> = cards
        .iter()
        .filter(|c| c.card_type == "plan")
        .map(|c| c.title.as_str())
        .collect();
    let decisions: Vec<&str> = cards
        .iter()
        .filter(|c| c.card_type == "decision")
        .map(|c| c.title.as_str())
        .collect();
    let constraints: Vec<&str> = cards
        .iter()
        .filter(|c| c.card_type == "constraint")
        .map(|c| c.title.as_str())
        .collect();
    let risks: Vec<&str> = cards
        .iter()
        .filter(|c| c.card_type == "risk")
        .map(|c| c.title.as_str())
        .collect();
    let assumptions: Vec<&str> = cards
        .iter()
        .filter(|c| c.card_type == "assumption")
        .map(|c| c.title.as_str())
        .collect();
    let open_questions: Vec<&str> = cards
        .iter()
        .filter(|c| c.card_type == "open_question")
        .map(|c| c.title.as_str())
        .collect();

    // Build synthesized prompts for each pipeline phase
    let plan_prompt = build_plan_prompt(&goal, &ideas, &constraints, spec_constraints);
    let setup_prompt = build_setup_prompt(&goal);
    let tdd_prompt = build_tdd_prompt(&goal, &tasks, &plans);
    let implement_prompt = build_implement_prompt(&goal, &tasks, &plans);
    let verify_prompt = build_verify_prompt(&goal, &decisions, success_criteria);
    let scenario_test_prompt = build_scenario_test_prompt(&goal, &assumptions, success_criteria);
    let review_prompt = build_review_prompt(&goal, &open_questions);
    let polish_prompt = build_polish_prompt(&risks);
    let release_prompt = build_release_prompt(&goal);

    // Graph declaration
    writeln!(out, "digraph {} {{", graph_name).unwrap();
    writeln!(out, "graph [").unwrap();
    writeln!(
        out,
        "goal=\"{}\",",
        escape_dot_string(&goal)
    )
    .unwrap();
    writeln!(out, "retry_target=\"implement\",").unwrap();
    writeln!(out, "default_max_retry=2,").unwrap();
    writeln!(out, "rankdir=LR").unwrap();
    writeln!(out, "]").unwrap();
    writeln!(out).unwrap();

    // Sentinel nodes
    writeln!(out).unwrap();
    writeln!(out, "start [shape=Mdiamond, label=\"Start\"]").unwrap();
    writeln!(out, "done  [shape=Msquare, label=\"Done\"]").unwrap();
    writeln!(out).unwrap();

    // Pipeline phase nodes
    writeln!(
        out,
        "plan [shape=box, label=\"Plan\", prompt=\"{}\"]",
        escape_dot_string(&plan_prompt)
    )
    .unwrap();
    writeln!(
        out,
        "setup [shape=box, label=\"Setup\", prompt=\"{}\"]",
        escape_dot_string(&setup_prompt)
    )
    .unwrap();
    writeln!(
        out,
        "tdd [shape=box, label=\"TDD\", prompt=\"{}\"]",
        escape_dot_string(&tdd_prompt)
    )
    .unwrap();
    writeln!(
        out,
        "implement [shape=box, label=\"Implement\", prompt=\"{}\", goal_gate=true, max_retries=3]",
        escape_dot_string(&implement_prompt)
    )
    .unwrap();
    writeln!(
        out,
        "verify [shape=box, label=\"Verify\", prompt=\"{}\"]",
        escape_dot_string(&verify_prompt)
    )
    .unwrap();
    writeln!(
        out,
        "verify_ok [shape=diamond, label=\"Tests passed?\"]"
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "scenario_test [shape=box, label=\"Scenario Test\", prompt=\"{}\"]",
        escape_dot_string(&scenario_test_prompt)
    )
    .unwrap();
    writeln!(
        out,
        "scenario_ok [shape=diamond, label=\"Scenarios passed?\"]"
    )
    .unwrap();
    writeln!(out).unwrap();
    writeln!(
        out,
        "review_gate [shape=hexagon, type=\"wait.human\", label=\"Review\", prompt=\"{}\"]",
        escape_dot_string(&review_prompt)
    )
    .unwrap();
    writeln!(
        out,
        "polish [shape=box, label=\"Polish\", prompt=\"{}\"]",
        escape_dot_string(&polish_prompt)
    )
    .unwrap();
    writeln!(
        out,
        "release [shape=box, label=\"Release\", prompt=\"{}\"]",
        escape_dot_string(&release_prompt)
    )
    .unwrap();
    writeln!(out).unwrap();

    // Edges: main chain (TDD before implement)
    writeln!(
        out,
        "start -> plan -> setup -> tdd -> implement -> verify -> verify_ok"
    )
    .unwrap();
    writeln!(out).unwrap();

    // Conditional gate: verify_ok (unit tests)
    writeln!(
        out,
        "verify_ok -> scenario_test [label=\"Pass\", condition=\"outcome=SUCCESS\"]"
    )
    .unwrap();
    writeln!(
        out,
        "verify_ok -> implement [label=\"Fail\", condition=\"outcome=FAIL\"]"
    )
    .unwrap();
    writeln!(out).unwrap();

    // Scenario test flows into its own diamond gate
    writeln!(out, "scenario_test -> scenario_ok").unwrap();
    writeln!(out).unwrap();

    // Conditional gate: scenario_ok (real-dependency validation)
    writeln!(
        out,
        "scenario_ok -> review_gate [label=\"Pass\", condition=\"outcome=SUCCESS\"]"
    )
    .unwrap();
    writeln!(
        out,
        "scenario_ok -> tdd [label=\"Fail\", condition=\"outcome=FAIL\"]"
    )
    .unwrap();
    writeln!(out).unwrap();

    // Human gate: review_gate
    writeln!(
        out,
        "review_gate -> release [label=\"[A] Approve\", weight=3]"
    )
    .unwrap();
    writeln!(
        out,
        "review_gate -> polish  [label=\"[F] Fix\", weight=1]"
    )
    .unwrap();
    writeln!(out).unwrap();

    // Retry loop and final edge
    writeln!(out, "polish -> tdd").unwrap();
    writeln!(out, "release -> done").unwrap();
    writeln!(out).unwrap();

    writeln!(out).unwrap();
    writeln!(out, "}}").unwrap();
    out
}

/// Build the prompt for the "plan" phase.
/// Aggregates ideas and constraints into a planning directive.
fn build_plan_prompt(goal: &str, ideas: &[&str], constraints: &[&str], spec_constraints: &str) -> String {
    let mut parts = vec![format!("Plan the approach for: {}", goal)];
    if !ideas.is_empty() {
        parts.push(format!("Key ideas: {}", ideas.join("; ")));
    }
    let mut all_constraints: Vec<&str> = constraints.to_vec();
    if !spec_constraints.is_empty() {
        all_constraints.push(spec_constraints);
    }
    if !all_constraints.is_empty() {
        parts.push(format!("Constraints: {}", all_constraints.join("; ")));
    }
    truncate_prompt(&parts.join(". "))
}

/// Build the prompt for the "setup" phase.
fn build_setup_prompt(goal: &str) -> String {
    truncate_prompt(&format!("Set up the project infrastructure for: {}", goal))
}

/// Build the prompt for the "tdd" phase.
/// Aggregates tasks and plans into test-first specifications.
fn build_tdd_prompt(goal: &str, tasks: &[&str], plans: &[&str]) -> String {
    let mut parts = vec![format!("Write failing tests for: {}", goal)];
    if !tasks.is_empty() {
        parts.push(format!("Cover: {}", tasks.join("; ")));
    }
    if !plans.is_empty() {
        parts.push(format!("Following: {}", plans.join("; ")));
    }
    parts.push("Tests must fail before implementation begins.".to_string());
    truncate_prompt(&parts.join(". "))
}

/// Build the prompt for the "implement" phase.
/// Aggregates tasks and plans into implementation directives.
fn build_implement_prompt(goal: &str, tasks: &[&str], plans: &[&str]) -> String {
    let mut parts = vec![format!("Implement: {}", goal)];
    if !tasks.is_empty() {
        parts.push(format!("Deliver: {}", tasks.join("; ")));
    }
    if !plans.is_empty() {
        parts.push(format!("Following: {}", plans.join("; ")));
    }
    parts.push("Write only enough code to make the failing tests pass.".to_string());
    truncate_prompt(&parts.join(". "))
}

/// Build the prompt for the "verify" phase.
/// Aggregates decisions and success criteria into unit/integration test directives.
fn build_verify_prompt(goal: &str, decisions: &[&str], success_criteria: &str) -> String {
    let mut parts = vec![format!("Verify: {}", goal)];
    parts.push("Run typecheck, lint, unit tests, and integration tests.".to_string());
    if !decisions.is_empty() {
        parts.push(format!("Validate: {}", decisions.join("; ")));
    }
    if !success_criteria.is_empty() {
        parts.push(format!("Success criteria: {}", success_criteria));
    }
    parts.push("Report outcome=SUCCESS if all pass, else outcome=FAIL.".to_string());
    truncate_prompt(&parts.join(". "))
}

/// Build the prompt for the "scenario_test" phase.
/// Aggregates assumptions and success criteria into real-dependency validation.
/// Enforces the iron law: no mocks, real dependencies only.
fn build_scenario_test_prompt(goal: &str, assumptions: &[&str], success_criteria: &str) -> String {
    let mut parts = vec![format!(
        "Run scenario tests against real dependencies for: {}",
        goal
    )];
    parts.push("No mocks allowed. Exercise real systems end-to-end.".to_string());
    if !assumptions.is_empty() {
        parts.push(format!("Validate assumptions: {}", assumptions.join("; ")));
    }
    if !success_criteria.is_empty() {
        parts.push(format!("Success criteria: {}", success_criteria));
    }
    parts.push("Report outcome=SUCCESS if all scenarios pass, else outcome=FAIL.".to_string());
    truncate_prompt(&parts.join(". "))
}

/// Build the prompt for the "review_gate" phase (human review).
/// Aggregates open questions for the reviewer.
fn build_review_prompt(goal: &str, open_questions: &[&str]) -> String {
    let mut parts = vec![format!("Human review: {}", goal)];
    if !open_questions.is_empty() {
        parts.push(format!("Open questions: {}", open_questions.join("; ")));
    }
    parts.push("Approve?".to_string());
    truncate_prompt(&parts.join(". "))
}

/// Build the prompt for the "polish" phase.
/// Aggregates risks into fix directives.
fn build_polish_prompt(risks: &[&str]) -> String {
    let mut parts = vec!["Apply fixes based on review feedback.".to_string()];
    if !risks.is_empty() {
        parts.push(format!("Risks: {}", risks.join("; ")));
    }
    truncate_prompt(&parts.join(". "))
}

/// Build the prompt for the "release" phase.
fn build_release_prompt(goal: &str) -> String {
    truncate_prompt(&format!("Prepare release: {}", goal))
}

/// Truncate a prompt string to at most `MAX_PROMPT_LEN` characters,
/// using char-safe indexing.
fn truncate_prompt(s: &str) -> String {
    let end = s
        .char_indices()
        .nth(MAX_PROMPT_LEN)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    s[..end].to_string()
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
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
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
            lanes: vec!["Ideas".to_string(), "Plan".to_string(), "Spec".to_string()],
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

    // -- Pipeline structure tests --

    #[test]
    fn pipeline_has_all_12_nodes() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        // Sentinel nodes
        assert!(dot.contains("start [shape=Mdiamond, label=\"Start\"]"));
        assert!(dot.contains("done  [shape=Msquare, label=\"Done\"]"));

        // All 10 pipeline phase nodes
        assert!(dot.contains("plan [shape=box,"), "Missing plan node in:\n{}", dot);
        assert!(dot.contains("setup [shape=box,"), "Missing setup node in:\n{}", dot);
        assert!(dot.contains("tdd [shape=box,"), "Missing tdd node in:\n{}", dot);
        assert!(dot.contains("implement [shape=box,"), "Missing implement node in:\n{}", dot);
        assert!(dot.contains("verify [shape=box,"), "Missing verify node in:\n{}", dot);
        assert!(dot.contains("verify_ok [shape=diamond,"), "Missing verify_ok node in:\n{}", dot);
        assert!(dot.contains("scenario_test [shape=box,"), "Missing scenario_test node in:\n{}", dot);
        assert!(dot.contains("scenario_ok [shape=diamond,"), "Missing scenario_ok node in:\n{}", dot);
        assert!(dot.contains("review_gate [shape=hexagon,"), "Missing review_gate node in:\n{}", dot);
        assert!(dot.contains("polish [shape=box,"), "Missing polish node in:\n{}", dot);
        assert!(dot.contains("release [shape=box,"), "Missing release node in:\n{}", dot);
    }

    #[test]
    fn main_chain_includes_tdd_before_implement() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("start -> plan -> setup -> tdd -> implement -> verify -> verify_ok"),
            "Missing main chain with tdd in:\n{}", dot
        );
    }

    #[test]
    fn graph_attributes_use_commas_and_fixed_retry_target() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.starts_with("digraph test_spec {"),
            "Expected digraph test_spec {{ in:\n{}", dot
        );
        assert!(
            dot.contains("goal=\"Verify the DOT exporter\","),
            "Expected goal with trailing comma in:\n{}", dot
        );
        assert!(
            dot.contains("retry_target=\"implement\","),
            "Expected retry_target=\"implement\" with comma in:\n{}", dot
        );
        assert!(
            dot.contains("default_max_retry=2,"),
            "Expected default_max_retry=2 with comma in:\n{}", dot
        );
        assert!(
            dot.contains("rankdir=LR"),
            "Expected rankdir=LR in:\n{}", dot
        );
    }

    // -- Gate tests --

    #[test]
    fn verify_ok_routes_to_scenario_test_or_implement() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("verify_ok -> scenario_test [label=\"Pass\", condition=\"outcome=SUCCESS\"]"),
            "Missing verify_ok -> scenario_test SUCCESS edge in:\n{}", dot
        );
        assert!(
            dot.contains("verify_ok -> implement [label=\"Fail\", condition=\"outcome=FAIL\"]"),
            "Missing verify_ok -> implement FAIL edge in:\n{}", dot
        );
    }

    #[test]
    fn scenario_test_feeds_into_scenario_ok_gate() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("scenario_test -> scenario_ok"),
            "Missing scenario_test -> scenario_ok edge in:\n{}", dot
        );
    }

    #[test]
    fn scenario_ok_routes_to_review_or_tdd() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("scenario_ok -> review_gate [label=\"Pass\", condition=\"outcome=SUCCESS\"]"),
            "Missing scenario_ok -> review_gate SUCCESS edge in:\n{}", dot
        );
        assert!(
            dot.contains("scenario_ok -> tdd [label=\"Fail\", condition=\"outcome=FAIL\"]"),
            "Missing scenario_ok -> tdd FAIL edge in:\n{}", dot
        );
    }

    #[test]
    fn human_gate_review_gate_has_weighted_branches() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("review_gate [shape=hexagon, type=\"wait.human\""),
            "Missing hexagon wait.human on review_gate in:\n{}", dot
        );
        assert!(
            dot.contains("review_gate -> release [label=\"[A] Approve\", weight=3]"),
            "Missing Approve edge in:\n{}", dot
        );
        assert!(
            dot.contains("review_gate -> polish  [label=\"[F] Fix\", weight=1]"),
            "Missing Fix edge in:\n{}", dot
        );
    }

    #[test]
    fn polish_loops_back_to_tdd() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("polish -> tdd"),
            "Missing polish -> tdd loop in:\n{}", dot
        );
    }

    #[test]
    fn release_connects_to_done() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("release -> done"),
            "Missing release -> done in:\n{}", dot
        );
    }

    #[test]
    fn implement_has_goal_gate_and_max_retries() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("implement [shape=box, label=\"Implement\","),
            "Missing implement node in:\n{}", dot
        );
        assert!(
            dot.contains("goal_gate=true"),
            "Missing goal_gate=true on implement in:\n{}", dot
        );
        assert!(
            dot.contains("max_retries=3"),
            "Missing max_retries=3 on implement in:\n{}", dot
        );
    }

    // -- TDD prompt tests --

    #[test]
    fn tdd_prompt_includes_test_first_directive() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("Write failing tests for:"),
            "TDD prompt missing test-first directive in:\n{}", dot
        );
        assert!(
            dot.contains("Tests must fail before implementation begins"),
            "TDD prompt missing fail-first requirement in:\n{}", dot
        );
    }

    #[test]
    fn tdd_prompt_aggregates_tasks_and_plans() {
        let mut state = make_state_with_core();

        let task = make_card("task", "Build API", "Spec", 1.0, "human");
        let plan = make_card("plan", "Roadmap", "Plan", 1.0, "human");
        state.cards.insert(task.card_id, task);
        state.cards.insert(plan.card_id, plan);

        let dot = export_dot(&state);

        assert!(
            dot.contains("Cover: Build API"),
            "TDD prompt missing task in:\n{}", dot
        );
        assert!(
            dot.contains("Following: Roadmap"),
            "TDD prompt missing plan in:\n{}", dot
        );
    }

    // -- Scenario test prompt tests --

    #[test]
    fn scenario_test_prompt_enforces_no_mocks() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            dot.contains("No mocks allowed"),
            "Scenario test prompt missing no-mocks directive in:\n{}", dot
        );
        assert!(
            dot.contains("real dependencies"),
            "Scenario test prompt missing real-dependencies in:\n{}", dot
        );
    }

    #[test]
    fn scenario_test_prompt_aggregates_assumptions() {
        let mut state = make_state_with_core();

        let assumption = make_card("assumption", "Fast Network", "Ideas", 1.0, "human");
        state.cards.insert(assumption.card_id, assumption);

        let dot = export_dot(&state);

        assert!(
            dot.contains("Validate assumptions: Fast Network"),
            "Scenario test prompt missing assumption in:\n{}", dot
        );
    }

    #[test]
    fn scenario_test_prompt_includes_success_criteria() {
        let mut state = make_state_with_core();
        state.core.as_mut().unwrap().success_criteria = Some("All endpoints respond < 200ms".to_string());

        let dot = export_dot(&state);

        // scenario_test should include success criteria
        let scenario_line = dot
            .lines()
            .find(|l| l.contains("scenario_test [shape=box"))
            .expect("scenario_test node not found");
        assert!(
            scenario_line.contains("All endpoints respond < 200ms"),
            "Scenario test prompt missing success criteria in:\n{}", scenario_line
        );
    }

    // -- Implement prompt tests --

    #[test]
    fn implement_prompt_references_tdd() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        let implement_line = dot
            .lines()
            .find(|l| l.contains("implement [shape=box"))
            .expect("implement node not found");
        assert!(
            implement_line.contains("make the failing tests pass"),
            "Implement prompt missing TDD reference in:\n{}", implement_line
        );
    }

    #[test]
    fn cards_aggregate_into_implement_prompt() {
        let mut state = make_state_with_core();

        let task1 = make_card("task", "Build API", "Spec", 1.0, "human");
        let task2 = make_card("task", "Add Tests", "Spec", 2.0, "human");
        let plan = make_card("plan", "Roadmap", "Plan", 1.0, "human");
        state.cards.insert(task1.card_id, task1);
        state.cards.insert(task2.card_id, task2);
        state.cards.insert(plan.card_id, plan);

        let dot = export_dot(&state);

        assert!(
            dot.contains("Deliver: Build API; Add Tests") || dot.contains("Deliver: Add Tests; Build API"),
            "Implement prompt missing tasks in:\n{}", dot
        );
        assert!(
            dot.contains("Following: Roadmap"),
            "Implement prompt missing plans in:\n{}", dot
        );
    }

    // -- Verify prompt tests --

    #[test]
    fn verify_prompt_includes_test_directives() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        let verify_line = dot
            .lines()
            .find(|l| l.contains("verify [shape=box"))
            .expect("verify node not found");
        assert!(
            verify_line.contains("typecheck, lint, unit tests"),
            "Verify prompt missing test directives in:\n{}", verify_line
        );
        assert!(
            verify_line.contains("outcome=SUCCESS") || verify_line.contains("outcome=FAIL"),
            "Verify prompt missing outcome reporting in:\n{}", verify_line
        );
    }

    #[test]
    fn cards_aggregate_into_verify_prompt() {
        let mut state = make_state_with_core();
        state.core.as_mut().unwrap().success_criteria = Some("All tests pass".to_string());

        let decision = make_card("decision", "Choose Stack", "Plan", 1.0, "human");
        state.cards.insert(decision.card_id, decision);

        let dot = export_dot(&state);

        assert!(
            dot.contains("Validate: Choose Stack"),
            "Verify prompt missing decision in:\n{}", dot
        );
        assert!(
            dot.contains("Success criteria: All tests pass"),
            "Verify prompt missing success criteria in:\n{}", dot
        );
    }

    // -- Other prompt tests --

    #[test]
    fn cards_aggregate_into_plan_prompt() {
        let mut state = make_state_with_core();

        let idea = make_card("idea", "Fast DB", "Ideas", 1.0, "human");
        let constraint = make_card("constraint", "Must Use Rust", "Plan", 1.0, "human");
        state.cards.insert(idea.card_id, idea);
        state.cards.insert(constraint.card_id, constraint);

        let dot = export_dot(&state);

        assert!(
            dot.contains("Plan the approach for: Verify the DOT exporter"),
            "Plan prompt missing goal in:\n{}", dot
        );
        assert!(
            dot.contains("Key ideas: Fast DB"),
            "Plan prompt missing ideas in:\n{}", dot
        );
        assert!(
            dot.contains("Must Use Rust"),
            "Plan prompt missing constraint card in:\n{}", dot
        );
    }

    #[test]
    fn cards_aggregate_into_review_prompt() {
        let mut state = make_state_with_core();

        let question = make_card("open_question", "What DB", "Plan", 1.0, "human");
        state.cards.insert(question.card_id, question);

        let dot = export_dot(&state);

        assert!(
            dot.contains("Open questions: What DB"),
            "Review prompt missing open question in:\n{}", dot
        );
        assert!(
            dot.contains("Approve?"),
            "Review prompt missing Approve? in:\n{}", dot
        );
    }

    #[test]
    fn cards_aggregate_into_polish_prompt() {
        let mut state = make_state_with_core();

        let risk = make_card("risk", "Data Loss", "Ideas", 1.0, "human");
        state.cards.insert(risk.card_id, risk);

        let dot = export_dot(&state);

        assert!(
            dot.contains("Apply fixes based on review feedback"),
            "Polish prompt missing base text in:\n{}", dot
        );
        assert!(
            dot.contains("Risks: Data Loss"),
            "Polish prompt missing risk in:\n{}", dot
        );
    }

    #[test]
    fn spec_constraints_merged_into_plan_prompt() {
        let mut state = make_state_with_core();
        state.core.as_mut().unwrap().constraints = Some("Budget < $1000".to_string());

        let dot = export_dot(&state);

        assert!(
            dot.contains("Budget < $1000"),
            "Plan prompt missing spec-level constraints in:\n{}", dot
        );
    }

    #[test]
    fn empty_card_types_produce_clean_prompts() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(
            !dot.contains("Key ideas:"),
            "Plan prompt should omit Key ideas when empty in:\n{}", dot
        );
        assert!(
            !dot.contains("Deliver:"),
            "Implement prompt should omit Deliver when empty in:\n{}", dot
        );
        assert!(
            !dot.contains("Validate:"),
            "Verify prompt should omit Validate when empty in:\n{}", dot
        );
        assert!(
            !dot.contains("Open questions:"),
            "Review prompt should omit Open questions when empty in:\n{}", dot
        );
        assert!(
            !dot.contains("Validate assumptions:"),
            "Scenario test prompt should omit Validate assumptions when empty in:\n{}", dot
        );
        assert!(
            !dot.contains("Cover:"),
            "TDD prompt should omit Cover when empty in:\n{}", dot
        );
    }

    // -- Goal and name fallback tests --

    #[test]
    fn goal_fallback_uses_title_and_one_liner() {
        let core = SpecCore {
            spec_id: Ulid::new(),
            title: "Fallback Test".to_string(),
            one_liner: "Uses title and one_liner".to_string(),
            goal: String::new(),
            description: None,
            constraints: None,
            success_criteria: None,
            risks: None,
            notes: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let state = SpecState {
            core: Some(core),
            cards: BTreeMap::new(),
            transcript: Vec::new(),
            pending_question: None,
            undo_stack: Vec::new(),
            last_event_id: 0,
            lanes: vec!["Ideas".to_string(), "Plan".to_string(), "Spec".to_string()],
        };
        let dot = export_dot(&state);

        assert!(
            dot.contains("goal=\"Fallback Test: Uses title and one_liner\""),
            "Expected fallback goal from title: one_liner in:\n{}", dot
        );
    }

    #[test]
    fn none_core_uses_defaults() {
        let state = SpecState {
            core: None,
            cards: BTreeMap::new(),
            transcript: Vec::new(),
            pending_question: None,
            undo_stack: Vec::new(),
            last_event_id: 0,
            lanes: vec!["Ideas".to_string(), "Plan".to_string(), "Spec".to_string()],
        };
        let dot = export_dot(&state);

        assert!(
            dot.starts_with("digraph unnamed_spec {"),
            "Expected unnamed_spec graph in:\n{}", dot
        );
        assert!(
            dot.contains("goal=\"\","),
            "Expected empty goal in:\n{}", dot
        );
    }

    // -- Escaping tests --

    #[test]
    fn escapes_quotes_in_goal() {
        let core = SpecCore {
            spec_id: Ulid::new(),
            title: "Quote Test".to_string(),
            one_liner: "test".to_string(),
            goal: "Say \"hello\" to the world".to_string(),
            description: None,
            constraints: None,
            success_criteria: None,
            risks: None,
            notes: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        let state = SpecState {
            core: Some(core),
            cards: BTreeMap::new(),
            transcript: Vec::new(),
            pending_question: None,
            undo_stack: Vec::new(),
            last_event_id: 0,
            lanes: vec!["Ideas".to_string()],
        };
        let dot = export_dot(&state);

        assert!(
            dot.contains("goal=\"Say \\\"hello\\\" to the world\""),
            "Expected escaped quotes in goal in:\n{}", dot
        );
    }

    #[test]
    fn escapes_newlines_in_card_titles_within_prompts() {
        let mut state = make_state_with_core();

        let card = make_card("idea", "Line one\nLine two", "Ideas", 1.0, "human");
        state.cards.insert(card.card_id, card);

        let dot = export_dot(&state);

        assert!(
            dot.contains("Line one\\nLine two"),
            "Expected escaped newline in prompt in:\n{}", dot
        );
    }

    // -- Prompt truncation test --

    #[test]
    fn long_prompts_are_truncated() {
        let mut state = make_state_with_core();

        for i in 0..50 {
            let card = make_card(
                "task",
                &format!("Very Long Task Name Number {} With Extra Words", i),
                "Spec",
                i as f64,
                "human",
            );
            state.cards.insert(card.card_id, card);
        }

        let dot = export_dot(&state);

        let implement_line = dot
            .lines()
            .find(|l| l.contains("implement [shape=box"))
            .expect("implement node not found");

        assert!(
            !implement_line.contains("Number 49"),
            "Expected truncated prompt, but found last task in:\n{}", implement_line
        );
    }

    // -- Helper unit tests --

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
    fn escape_dot_string_handles_special_chars() {
        assert_eq!(escape_dot_string("hello"), "hello");
        assert_eq!(escape_dot_string("say \"hi\""), "say \\\"hi\\\"");
        assert_eq!(escape_dot_string("a\nb"), "a\\nb");
        assert_eq!(escape_dot_string("a\\b"), "a\\\\b");
        assert_eq!(escape_dot_string("a\rb"), "a\\rb");
    }

    #[test]
    fn truncate_prompt_respects_limit() {
        let short = "hello";
        assert_eq!(truncate_prompt(short), "hello");

        let exact = "x".repeat(MAX_PROMPT_LEN);
        assert_eq!(truncate_prompt(&exact), exact);

        let long = "x".repeat(MAX_PROMPT_LEN + 100);
        let truncated = truncate_prompt(&long);
        assert_eq!(truncated.len(), MAX_PROMPT_LEN);
    }

    // -- Multiple card types coexist --

    #[test]
    fn all_card_types_contribute_to_their_phases() {
        let mut state = make_state_with_core();

        let idea = make_card("idea", "Brainstorm", "Ideas", 1.0, "human");
        let task = make_card("task", "Build API", "Spec", 1.0, "human");
        let plan = make_card("plan", "Roadmap", "Plan", 1.0, "human");
        let decision = make_card("decision", "Choose DB", "Plan", 2.0, "human");
        let constraint = make_card("constraint", "Budget Cap", "Plan", 3.0, "human");
        let risk = make_card("risk", "Data Loss", "Ideas", 2.0, "human");
        let assumption = make_card("assumption", "Fast Network", "Ideas", 3.0, "human");
        let open_q = make_card("open_question", "What Stack", "Plan", 4.0, "human");

        state.cards.insert(idea.card_id, idea);
        state.cards.insert(task.card_id, task);
        state.cards.insert(plan.card_id, plan);
        state.cards.insert(decision.card_id, decision);
        state.cards.insert(constraint.card_id, constraint);
        state.cards.insert(risk.card_id, risk);
        state.cards.insert(assumption.card_id, assumption);
        state.cards.insert(open_q.card_id, open_q);

        let dot = export_dot(&state);

        // plan phase: ideas + constraints
        assert!(dot.contains("Key ideas: Brainstorm"), "Missing idea in plan prompt:\n{}", dot);
        assert!(dot.contains("Budget Cap"), "Missing constraint in plan prompt:\n{}", dot);

        // tdd phase: tasks + plans
        assert!(dot.contains("Cover: Build API"), "Missing task in tdd prompt:\n{}", dot);

        // implement phase: tasks + plans
        assert!(dot.contains("Deliver: Build API"), "Missing task in implement prompt:\n{}", dot);
        assert!(dot.contains("Following: Roadmap"), "Missing plan in implement prompt:\n{}", dot);

        // verify phase: decisions
        assert!(dot.contains("Validate: Choose DB"), "Missing decision in verify prompt:\n{}", dot);

        // scenario_test phase: assumptions
        assert!(dot.contains("Validate assumptions: Fast Network"), "Missing assumption in scenario_test prompt:\n{}", dot);

        // review phase: open questions
        assert!(dot.contains("Open questions: What Stack"), "Missing open_q in review prompt:\n{}", dot);

        // polish phase: risks
        assert!(dot.contains("Risks: Data Loss"), "Missing risk in polish prompt:\n{}", dot);
    }

    #[test]
    fn node_ids_are_all_snake_case() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        let fixed_nodes = [
            "start", "done", "plan", "setup", "tdd", "implement", "verify",
            "verify_ok", "scenario_test", "scenario_ok", "review_gate",
            "polish", "release",
        ];
        for node in &fixed_nodes {
            assert!(
                node.chars().all(|c| c.is_lowercase() || c == '_'),
                "Node '{}' is not snake_case",
                node
            );
        }

        // Verify the actual DOT output contains these node identifiers
        for node in &fixed_nodes {
            let found = dot.lines().any(|line| {
                let trimmed = line.trim();
                trimmed.starts_with(&format!("{} [", node))
                    || trimmed.starts_with(&format!("{}  [", node))
                    || trimmed.contains(&format!("{} ->", node))
                    || trimmed.contains(&format!("-> {}", node))
            });
            assert!(
                found,
                "Expected node '{}' in DOT output:\n{}",
                node,
                dot
            );
        }
    }

    #[test]
    fn valid_dot_syntax_braces_match() {
        let state = make_state_with_core();
        let dot = export_dot(&state);

        assert!(dot.contains('{'), "Missing opening brace");
        assert!(dot.trim().ends_with('}'), "Missing closing brace");

        let opens = dot.chars().filter(|&c| c == '{').count();
        let closes = dot.chars().filter(|&c| c == '}').count();
        assert_eq!(opens, closes, "Mismatched braces: {} opens, {} closes", opens, closes);
    }

    #[test]
    fn inspiration_and_vibes_cards_count_as_ideas() {
        let mut state = make_state_with_core();

        let vibes = make_card("vibes", "Good Energy", "Ideas", 1.0, "human");
        let inspiration = make_card("inspiration", "Cool Pattern", "Ideas", 2.0, "human");
        state.cards.insert(vibes.card_id, vibes);
        state.cards.insert(inspiration.card_id, inspiration);

        let dot = export_dot(&state);

        assert!(
            dot.contains("Good Energy"),
            "Missing vibes card in plan prompt:\n{}", dot
        );
        assert!(
            dot.contains("Cool Pattern"),
            "Missing inspiration card in plan prompt:\n{}", dot
        );
    }

    // -- Prompt builder unit tests --

    #[test]
    fn build_plan_prompt_with_no_cards() {
        let prompt = build_plan_prompt("Build a thing", &[], &[], "");
        assert_eq!(prompt, "Plan the approach for: Build a thing");
    }

    #[test]
    fn build_plan_prompt_with_ideas_and_constraints() {
        let prompt = build_plan_prompt(
            "Build a thing",
            &["Fast DB", "Cool UI"],
            &["Budget Cap"],
            "Must be done by Friday",
        );
        assert!(prompt.contains("Key ideas: Fast DB; Cool UI"));
        assert!(prompt.contains("Constraints: Budget Cap; Must be done by Friday"));
    }

    #[test]
    fn build_tdd_prompt_with_no_cards() {
        let prompt = build_tdd_prompt("Build a thing", &[], &[]);
        assert!(prompt.starts_with("Write failing tests for: Build a thing"));
        assert!(prompt.contains("Tests must fail before implementation begins"));
    }

    #[test]
    fn build_tdd_prompt_with_tasks() {
        let prompt = build_tdd_prompt("Build a thing", &["Auth", "API"], &["Roadmap"]);
        assert!(prompt.contains("Cover: Auth; API"));
        assert!(prompt.contains("Following: Roadmap"));
    }

    #[test]
    fn build_implement_prompt_with_no_cards() {
        let prompt = build_implement_prompt("Build a thing", &[], &[]);
        assert!(prompt.starts_with("Implement: Build a thing"));
        assert!(prompt.contains("make the failing tests pass"));
    }

    #[test]
    fn build_verify_prompt_with_no_cards() {
        let prompt = build_verify_prompt("Build a thing", &[], "");
        assert!(prompt.starts_with("Verify: Build a thing"));
        assert!(prompt.contains("typecheck, lint, unit tests"));
    }

    #[test]
    fn build_scenario_test_prompt_with_no_cards() {
        let prompt = build_scenario_test_prompt("Build a thing", &[], "");
        assert!(prompt.contains("real dependencies"));
        assert!(prompt.contains("No mocks allowed"));
    }

    #[test]
    fn build_scenario_test_prompt_with_assumptions() {
        let prompt = build_scenario_test_prompt(
            "Build a thing",
            &["Users are online", "DB is fast"],
            "Response < 100ms",
        );
        assert!(prompt.contains("Validate assumptions: Users are online; DB is fast"));
        assert!(prompt.contains("Success criteria: Response < 100ms"));
    }

    #[test]
    fn build_review_prompt_always_ends_with_approve() {
        let prompt = build_review_prompt("Build a thing", &[]);
        assert!(prompt.ends_with("Approve?"));
    }

    #[test]
    fn build_polish_prompt_with_no_risks() {
        let prompt = build_polish_prompt(&[]);
        assert_eq!(prompt, "Apply fixes based on review feedback.");
    }

    #[test]
    fn build_release_prompt_includes_goal() {
        let prompt = build_release_prompt("Ship the widget");
        assert_eq!(prompt, "Prepare release: Ship the widget");
    }
}
