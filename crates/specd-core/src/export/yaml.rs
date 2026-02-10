// ABOUTME: Exports a SpecState as a structured YAML document matching spec.yaml format.
// ABOUTME: Uses serde_yaml for serialization with deterministic ordering.

use std::collections::BTreeMap;

use serde::Serialize;
use serde::ser::Error as SerError;

use crate::card::Card;
use crate::state::SpecState;

/// A serializable YAML representation of a single card within a lane.
#[derive(Debug, Serialize)]
struct YamlCard {
    id: String,
    #[serde(rename = "type")]
    card_type: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    body: Option<String>,
    order: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    refs: Vec<String>,
    created_by: String,
}

/// A serializable YAML representation of a lane containing cards.
#[derive(Debug, Serialize)]
struct YamlLane {
    name: String,
    cards: Vec<YamlCard>,
}

/// The top-level serializable YAML representation of the spec state.
#[derive(Debug, Serialize)]
struct YamlSpec {
    name: String,
    version: String,
    one_liner: String,
    goal: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    constraints: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    success_criteria: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    risks: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    notes: Option<String>,
    lanes: Vec<YamlLane>,
}

/// Export the spec state as structured YAML matching the spec.yaml format.
///
/// Uses the same deterministic ordering as the Markdown exporter: Ideas, Plan,
/// Spec first, then extra lanes alphabetically. Cards within lanes sorted by
/// order then card_id.
pub fn export_yaml(state: &SpecState) -> Result<String, serde_yaml::Error> {
    let core = state
        .core
        .as_ref()
        .ok_or_else(|| serde_yaml::Error::custom("SpecState must have a core to export YAML"))?;

    let cards_by_lane = group_cards_by_lane(state);
    let ordered_lanes = ordered_lane_names(state, &cards_by_lane);

    let yaml_lanes: Vec<YamlLane> = ordered_lanes
        .iter()
        .map(|lane_name| {
            let cards = cards_by_lane
                .get(lane_name.as_str())
                .map(|cards| {
                    cards
                        .iter()
                        .map(|card| YamlCard {
                            id: card.card_id.to_string(),
                            card_type: card.card_type.clone(),
                            title: card.title.clone(),
                            body: card.body.clone(),
                            order: card.order,
                            refs: card.refs.clone(),
                            created_by: card.created_by.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            YamlLane {
                name: lane_name.clone(),
                cards,
            }
        })
        .collect();

    let spec = YamlSpec {
        name: core.title.clone(),
        version: "0.1".to_string(),
        one_liner: core.one_liner.clone(),
        goal: core.goal.clone(),
        description: core.description.clone(),
        constraints: core.constraints.clone(),
        success_criteria: core.success_criteria.clone(),
        risks: core.risks.clone(),
        notes: core.notes.clone(),
        lanes: yaml_lanes,
    };

    serde_yaml::to_string(&spec)
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

/// Produce the ordered list of lane names: Ideas, Plan, Spec first,
/// then any additional lanes sorted alphabetically.
fn ordered_lane_names(
    state: &SpecState,
    cards_by_lane: &BTreeMap<&str, Vec<&Card>>,
) -> Vec<String> {
    let default_lanes = ["Ideas", "Plan", "Spec"];
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
            goal: "Verify the YAML exporter".to_string(),
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

    #[test]
    fn export_yaml_round_trip() {
        let mut state = make_state_with_core();

        let card = make_card("idea", "Test Card", "Ideas", 1.0, "human");
        state.cards.insert(card.card_id, card);

        let yaml_str = export_yaml(&state).expect("export should succeed");

        // Parse back as generic YAML value to verify structure
        let value: serde_yaml::Value =
            serde_yaml::from_str(&yaml_str).expect("should parse as valid YAML");

        let mapping = value.as_mapping().expect("top level should be mapping");

        // Verify required fields exist and match
        assert_eq!(
            mapping
                .get(serde_yaml::Value::String("name".to_string()))
                .unwrap()
                .as_str()
                .unwrap(),
            "Test Spec"
        );
        assert_eq!(
            mapping
                .get(serde_yaml::Value::String("version".to_string()))
                .unwrap()
                .as_str()
                .unwrap(),
            "0.1"
        );
        assert_eq!(
            mapping
                .get(serde_yaml::Value::String("one_liner".to_string()))
                .unwrap()
                .as_str()
                .unwrap(),
            "A test specification"
        );
        assert_eq!(
            mapping
                .get(serde_yaml::Value::String("goal".to_string()))
                .unwrap()
                .as_str()
                .unwrap(),
            "Verify the YAML exporter"
        );

        // Verify lanes structure
        let lanes = mapping
            .get(serde_yaml::Value::String("lanes".to_string()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert!(!lanes.is_empty());

        // Verify the Ideas lane has the card
        let ideas_lane = &lanes[0];
        let lane_name = ideas_lane
            .as_mapping()
            .unwrap()
            .get(serde_yaml::Value::String("name".to_string()))
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(lane_name, "Ideas");

        let cards = ideas_lane
            .as_mapping()
            .unwrap()
            .get(serde_yaml::Value::String("cards".to_string()))
            .unwrap()
            .as_sequence()
            .unwrap();
        assert_eq!(cards.len(), 1);

        let card_title = cards[0]
            .as_mapping()
            .unwrap()
            .get(serde_yaml::Value::String("title".to_string()))
            .unwrap()
            .as_str()
            .unwrap();
        assert_eq!(card_title, "Test Card");
    }

    #[test]
    fn export_yaml_deterministic() {
        let mut state = make_state_with_core();

        let card_a = make_card("idea", "Alpha", "Ideas", 1.0, "human");
        let card_b = make_card("task", "Beta", "Plan", 2.0, "agent");
        state.cards.insert(card_a.card_id, card_a);
        state.cards.insert(card_b.card_id, card_b);

        let yaml1 = export_yaml(&state).expect("export 1");
        let yaml2 = export_yaml(&state).expect("export 2");

        assert_eq!(yaml1, yaml2, "YAML export must be deterministic");
    }

    #[test]
    fn export_yaml_includes_all_cards() {
        let mut state = make_state_with_core();

        let card_a = make_card("idea", "Card A", "Ideas", 1.0, "human");
        let card_b = make_card("plan", "Card B", "Plan", 1.0, "human");
        let card_c = make_card("task", "Card C", "Spec", 1.0, "human");

        state.cards.insert(card_a.card_id, card_a);
        state.cards.insert(card_b.card_id, card_b);
        state.cards.insert(card_c.card_id, card_c);

        let yaml_str = export_yaml(&state).expect("export should succeed");

        // All three cards should appear in the YAML
        assert!(yaml_str.contains("Card A"));
        assert!(yaml_str.contains("Card B"));
        assert!(yaml_str.contains("Card C"));

        // Parse and count cards across all lanes
        let value: serde_yaml::Value = serde_yaml::from_str(&yaml_str).unwrap();
        let lanes = value
            .as_mapping()
            .unwrap()
            .get(serde_yaml::Value::String("lanes".to_string()))
            .unwrap()
            .as_sequence()
            .unwrap();

        let total_cards: usize = lanes
            .iter()
            .map(|lane| {
                lane.as_mapping()
                    .unwrap()
                    .get(serde_yaml::Value::String("cards".to_string()))
                    .unwrap()
                    .as_sequence()
                    .unwrap()
                    .len()
            })
            .sum();

        assert_eq!(total_cards, 3);
    }

    #[test]
    fn export_yaml_omits_optional_fields_when_none() {
        let state = make_state_with_core();
        let yaml_str = export_yaml(&state).expect("export should succeed");

        // Optional fields that are None should not appear
        assert!(!yaml_str.contains("description:"));
        assert!(!yaml_str.contains("constraints:"));
        assert!(!yaml_str.contains("success_criteria:"));
        assert!(!yaml_str.contains("risks:"));
        assert!(!yaml_str.contains("notes:"));
    }

    #[test]
    fn export_yaml_returns_err_when_core_is_none() {
        let state = SpecState::new();
        assert!(state.core.is_none());
        let result = export_yaml(&state);
        assert!(result.is_err(), "export_yaml should return Err when core is None");
    }

    #[test]
    fn export_yaml_includes_optional_fields_when_present() {
        let mut state = make_state_with_core();
        if let Some(ref mut core) = state.core {
            core.description = Some("A description".to_string());
            core.constraints = Some("Must be fast".to_string());
        }

        let yaml_str = export_yaml(&state).expect("export should succeed");

        assert!(yaml_str.contains("description:"));
        assert!(yaml_str.contains("A description"));
        assert!(yaml_str.contains("constraints:"));
        assert!(yaml_str.contains("Must be fast"));
    }
}
