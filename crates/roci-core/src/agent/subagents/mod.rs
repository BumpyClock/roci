//! Sub-agent supervisor system.
//!
//! Provides named profiles, model fallback, context propagation,
//! and parent-facing lifecycle management for child agent runtimes.

pub mod prompt;
pub mod types;

pub use prompt::SubagentPromptPolicy;
pub use types::{
    ModelCandidate, SnapshotMode, SubagentCompletion, SubagentContext, SubagentEvent, SubagentId,
    SubagentInput, SubagentKind, SubagentOverrides, SubagentProfile, SubagentProfileRef,
    SubagentRunResult, SubagentSnapshot, SubagentSpec, SubagentStatus, SubagentSummary,
    SubagentSupervisorConfig, ToolPolicy,
};
