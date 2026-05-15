//! Sub-agent supervisor system.
//!
//! Provides named profiles, model fallback, context propagation,
//! and parent-facing lifecycle management for child agent runtimes.
//!
//! # API location
//!
//! - [`SubagentSupervisor`] is the entry point: create, spawn, wait, abort, shutdown.
//! - [`SubagentProfileRegistry`] holds named profiles; call
//!   [`SubagentProfileRegistry::with_builtins`] for the built-in set or
//!   register TOML-defined profiles via
//!   [`SubagentProfileRegistry::load_from_roots`].
//! - Profile inheritance, model fallback, and tool policy are resolved at
//!   spawn time inside the supervisor (see `supervisor::spawn_with_context`).
//!
//! # Orchestration
//!
//! Start with [`SubagentSupervisor::spawn`] for explicit handle control.
//! Use [`SubagentSupervisor::run_parallel`] for fan-out/fan-in,
//! [`SubagentSupervisor::race`] for first-result-wins work, and
//! [`SubagentSupervisor::watch_all`] / [`SubagentSupervisor::watch_any`] for
//! snapshot streams over the currently active child set.
//!
//! # Human interaction
//!
//! All children share a [`crate::agent::runtime::HumanInteractionCoordinator`]
//! (taken from `AgentConfig` or freshly allocated). When any child emits
//! `AgentEvent::HumanInteractionRequested`, the supervisor wraps it in
//! `SubagentEvent::AgentEvent` and forwards it to the parent subscriber.
//! The parent calls [`SubagentSupervisor::submit_user_input`]; `request_id`
//! correlation routes the response to the correct child without a generic bus.
//!
//! # Peer-bus seam (v1 deferred)
//!
//! v1 ships a supervisor-first model: parent-to-child lifecycle and event
//! routing live in one place. Child-to-child messaging and a generic peer bus
//! are explicitly deferred. The `SubagentLauncher` trait, stable `SubagentId`,
//! and event-wrapper shape preserve seams for a future routing layer without
//! paying for the bus today. See `docs/ARCHITECTURE.md` §"Sub-Agent Supervisor"
//! for the full design rationale and deferred-feature list.

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
