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
pub mod supervisor;
pub mod types;

pub use config::TomlProfileFile;
pub use context::{build_child_initial_messages, default_child_input, materialize_context};
pub use handle::SubagentHandle;
pub use profiles::SubagentProfileRegistry;
pub use prompt::SubagentPromptPolicy;
pub use supervisor::SubagentSupervisor;
pub use types::{
    ModelCandidate, SnapshotMode, SubagentCompletion, SubagentContext, SubagentEvent, SubagentId,
    SubagentInput, SubagentKind, SubagentOverrides, SubagentProfile, SubagentProfileRef,
    SubagentRunResult, SubagentSnapshot, SubagentSpec, SubagentStatus, SubagentSummary,
    SubagentSupervisorConfig, ToolPolicy,
};
