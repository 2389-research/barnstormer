// ABOUTME: Provider module aggregating all LLM runtime adapters.
// ABOUTME: Each sub-module implements AgentRuntime for a specific LLM API.

pub mod anthropic;
pub mod gemini;
pub mod openai;

use crate::context::AgentRole;

/// Build a system prompt for the given agent role and spec title.
/// Shared across providers so all adapters produce consistent agent behavior.
pub fn role_prompt(role: &AgentRole, spec_title: &str) -> String {
    match role {
        AgentRole::Manager => format!(
            "You are the manager agent for spec '{}'. \
             Coordinate other agents, resolve conflicts, enforce policy. \
             Use tools to modify the spec.",
            spec_title
        ),
        AgentRole::Brainstormer => format!(
            "You are the brainstormer for spec '{}'. \
             Ask the user questions to understand their intent. \
             Generate idea cards.",
            spec_title
        ),
        AgentRole::Planner => format!(
            "You are the planner for spec '{}'. \
             Organize ideas into actionable plans. \
             Create plan and task cards.",
            spec_title
        ),
        AgentRole::DotGenerator => format!(
            "You are the DOT graph maintainer for spec '{}'. \
             Keep the build pipeline graph updated.",
            spec_title
        ),
        AgentRole::Critic => format!(
            "You are the critic for spec '{}'. \
             Review for consistency, risks, edge cases. \
             Create assumption and open_question cards.",
            spec_title
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_prompt_contains_spec_title() {
        let roles = [
            AgentRole::Manager,
            AgentRole::Brainstormer,
            AgentRole::Planner,
            AgentRole::DotGenerator,
            AgentRole::Critic,
        ];

        for role in &roles {
            let prompt = role_prompt(role, "My Test Spec");
            assert!(
                prompt.contains("My Test Spec"),
                "prompt for {:?} should contain spec title",
                role
            );
            assert!(
                !prompt.is_empty(),
                "prompt for {:?} should not be empty",
                role
            );
        }
    }
}
