// ABOUTME: Agent system for specd, orchestrating AI-assisted spec refinement.
// ABOUTME: Defines agent traits and step execution for spec exploration workflows.

pub mod context;
pub mod providers;
pub mod runtime;
pub mod swarm;
pub mod tools;

pub use context::{AgentContext, AgentRole, contexts_from_snapshot_map, contexts_to_snapshot_map};
pub use runtime::{AgentAction, AgentError, AgentRuntime};
pub use swarm::{AgentRunner, SwarmOrchestrator, create_runtime};
pub use tools::all_tool_definitions;
