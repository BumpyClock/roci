//! Sub-agent supervisor system.
//!
//! Provides named profiles, model fallback, context propagation,
//! and parent-facing lifecycle management for child agent runtimes.

pub mod config;
pub mod context;
pub mod events;
pub mod handle;
pub(crate) mod launcher;
pub mod profiles;
pub mod prompt;
pub mod routing;
pub mod routing_tools;
pub mod supervisor;
pub mod types;

pub use config::TomlProfileFile;
pub use context::{build_child_initial_messages, default_child_input, materialize_context};
pub use handle::SubagentHandle;
pub use profiles::{project_main_agent_profile, project_subagent_profile, SubagentProfileRegistry};
pub use prompt::SubagentPromptPolicy;
pub use routing::SubagentRoutingController;
pub use routing_tools::SubagentRoutingTools;
pub use supervisor::SubagentSupervisor;
pub use types::{
    DelegateSubagentRequest, DelegateSubagentResult, MainAgentProjection, McpServerProjection,
    ModelCandidate, NativeToolProjection, SendSubagentMessageResult, SnapshotMode,
    SubagentArtifact, SubagentCaller, SubagentCancelResult, SubagentCompletion, SubagentContext,
    SubagentEvent, SubagentId, SubagentInput, SubagentKind, SubagentKnownChild, SubagentOverrides,
    SubagentProfile, SubagentProfileRef, SubagentProfileSummary, SubagentProjection,
    SubagentRoutingMetadata, SubagentRunResult, SubagentSnapshot, SubagentSpec, SubagentStatus,
    SubagentSummary, SubagentSupervisorConfig, ToolPolicy,
};
