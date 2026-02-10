// ABOUTME: Agent system for specd, orchestrating AI-assisted spec refinement.
// ABOUTME: Defines agent traits and step execution for spec exploration workflows.

pub mod client;
pub mod context;
pub mod mux_tools;
pub mod swarm;
pub mod testing;

pub use context::{AgentContext, AgentRole, contexts_from_snapshot_map, contexts_to_snapshot_map};
pub use swarm::{AgentRunner, SwarmOrchestrator, run_loop, system_prompt_for_role};
