// ABOUTME: Agent system for barnstormer, orchestrating AI-assisted spec refinement.
// ABOUTME: Defines agent traits and step execution for spec exploration workflows.

pub mod attachment_summarizer;
pub mod card_body_writer;
pub mod card_decomposer;
pub mod client;
pub mod context;
pub mod import;
pub mod mux_tools;
pub mod narration_renderer;
pub mod streaming_hook;
pub mod swarm;
pub mod testing;

pub use attachment_summarizer::AttachmentSummarizer;
pub use card_body_writer::{CardBodyOutput, CardBodyRequest, CardBodyWriter, CardKind};
pub use card_decomposer::{CardDecomposer, DecomposedCard, DecomposerOutput, DecomposerUsage};
pub use context::{AgentContext, AgentRole, contexts_from_snapshot_map, contexts_to_snapshot_map};
pub use narration_renderer::{NarrationIntent, NarrationRenderer};
pub use swarm::{
    AgentRunner, SwarmOrchestrator, render_context_files_section, run_loop, system_prompt_for_role,
};
