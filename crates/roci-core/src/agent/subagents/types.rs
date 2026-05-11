//! Core types for the sub-agent supervisor system.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::runtime::chat::ThreadId;
use crate::agent_loop::AgentEvent;
use crate::models::LanguageModel;
use crate::types::ModelMessage;

// ---------------------------------------------------------------------------
// Identity
// ---------------------------------------------------------------------------

/// Unique identifier for a sub-agent instance.
pub type SubagentId = Uuid;

/// Broad behavioral category for a sub-agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentKind {
    Developer,
    Planner,
    Explorer,
    Custom(String),
}

/// Reference to a profile by name (e.g. `"builtin:developer"`).
pub type SubagentProfileRef = String;

// ---------------------------------------------------------------------------
// Model candidates
// ---------------------------------------------------------------------------

/// A single model candidate in a profile's ordered fallback list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCandidate {
    pub provider: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

// ---------------------------------------------------------------------------
// Tool policy
// ---------------------------------------------------------------------------

/// Policy for which tools a sub-agent may use.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ToolPolicy {
    /// Inherit all parent tools as-is.
    #[default]
    Inherit,
    /// Replace the tool set entirely.
    Replace { tools: Vec<String> },
    /// Inherit parent tools then add/remove.
    InheritWithOverrides {
        #[serde(default)]
        add: Vec<String>,
        #[serde(default)]
        remove: Vec<String>,
    },
}

// ---------------------------------------------------------------------------
// Profile projections
// ---------------------------------------------------------------------------

/// Native tool visibility projected for model schema and dispatch paths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeToolProjection {
    pub model_visible: Vec<String>,
    pub dispatch: Vec<String>,
}

/// MCP server identity projection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpServerProjection {
    pub server_ids: Vec<String>,
}

/// Profile projection for the main/default agent runtime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MainAgentProjection {
    pub profile: SubagentProfileRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelCandidate>,
    pub native_tools: NativeToolProjection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    pub mcp_servers: McpServerProjection,
}

/// Profile projection for child subagent runtime construction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentProjection {
    pub profile: SubagentProfileRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelCandidate>,
    pub native_tools: NativeToolProjection,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    pub mcp_servers: McpServerProjection,
}

// ---------------------------------------------------------------------------
// Profile
// ---------------------------------------------------------------------------

/// Named sub-agent profile defining behavior, tools, and model preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentProfile {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SubagentKind>,
    /// Routing hint text for controllers that infer an appropriate profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default)]
    pub tools: ToolPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_agent_excluded_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tools: Vec<String>,
    #[serde(default)]
    pub models: Vec<ModelCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherits: Option<SubagentProfileRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub default: bool,
}

fn default_version() -> u32 {
    1
}

impl Default for SubagentProfile {
    fn default() -> Self {
        Self {
            name: String::new(),
            display_name: None,
            description: None,
            kind: None,
            infer: None,
            system_prompt: None,
            tools: ToolPolicy::default(),
            skills: Vec::new(),
            mcp_servers: Vec::new(),
            default_agent_excluded_tools: Vec::new(),
            excluded_tools: Vec::new(),
            models: Vec::new(),
            inherits: None,
            default_timeout_ms: None,
            metadata: HashMap::new(),
            version: 1,
            default: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Built-in profiles
// ---------------------------------------------------------------------------

impl SubagentProfile {
    /// Built-in developer profile.
    pub fn builtin_developer() -> Self {
        Self {
            name: "builtin:developer".into(),
            display_name: Some("Developer".into()),
            description: Some("General-purpose coding sub-agent".into()),
            kind: Some(SubagentKind::Developer),
            infer: Some("implementation, debugging, refactoring, tests, code review".into()),
            system_prompt: Some(
                "You are a coding sub-agent. Write clean, correct code. \
                 Use `ask_user` when user input is required. \
                 Return concise results to the parent."
                    .into(),
            ),
            ..Default::default()
        }
    }

    /// Built-in planner profile.
    pub fn builtin_planner() -> Self {
        Self {
            name: "builtin:planner".into(),
            display_name: Some("Planner".into()),
            description: Some("Planning and architecture sub-agent".into()),
            kind: Some(SubagentKind::Planner),
            infer: Some("planning, architecture, decomposition, tradeoffs".into()),
            system_prompt: Some(
                "You are a planning sub-agent. Analyze requirements, \
                 propose designs, and break work into steps. \
                 Do not write implementation code directly."
                    .into(),
            ),
            ..Default::default()
        }
    }

    /// Built-in explorer profile.
    pub fn builtin_explorer() -> Self {
        Self {
            name: "builtin:explorer".into(),
            display_name: Some("Explorer".into()),
            description: Some("Codebase exploration and research sub-agent".into()),
            kind: Some(SubagentKind::Explorer),
            infer: Some("codebase search, discovery, research, reading files".into()),
            system_prompt: Some(
                "You are an exploration sub-agent. Search the codebase, \
                 read files, and report findings. Do not modify code."
                    .into(),
            ),
            ..Default::default()
        }
    }
}

/// Compact, controller-facing profile data for registry listings.
///
/// Summaries intentionally omit `system_prompt` and `metadata`; callers that
/// need the full executable profile should use profile resolution instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentProfileSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SubagentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infer: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_agent_excluded_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub excluded_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<ModelCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_timeout_ms: Option<u64>,
    #[serde(default)]
    pub default: bool,
    pub version: u32,
}

impl From<&SubagentProfile> for SubagentProfileSummary {
    fn from(profile: &SubagentProfile) -> Self {
        Self {
            name: profile.name.clone(),
            display_name: profile.display_name.clone(),
            description: profile.description.clone(),
            kind: profile.kind.clone(),
            infer: profile.infer.clone(),
            skills: profile.skills.clone(),
            mcp_servers: profile.mcp_servers.clone(),
            default_agent_excluded_tools: profile.default_agent_excluded_tools.clone(),
            excluded_tools: profile.excluded_tools.clone(),
            models: profile.models.clone(),
            default_timeout_ms: profile.default_timeout_ms,
            default: profile.default,
            version: profile.version,
        }
    }
}

// ---------------------------------------------------------------------------
// Spawn specification
// ---------------------------------------------------------------------------

/// How to provide input to a child sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubagentInput {
    /// Prompt text only, no parent context snapshot.
    Prompt { task: String },
    /// Parent context snapshot only, no new prompt.
    Snapshot { mode: SnapshotMode },
    /// Prompt text plus parent context snapshot.
    PromptWithSnapshot { task: String, mode: SnapshotMode },
}

/// Controls how much of the parent conversation is shared with the child.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    /// Summary text only (lightweight).
    SummaryOnly,
    /// Caller-provided explicit messages (no heuristic selection).
    SelectedMessages(Vec<ModelMessage>),
    /// Full materialized conversation (read-only, excludes runtime internals).
    FullReadonlySnapshot,
}

/// Read-only context passed to a child sub-agent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubagentContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_messages: Vec<ModelMessage>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_hints: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub resources: serde_json::Value,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub metadata: serde_json::Value,
}

/// Per-spawn overrides applied on top of the resolved profile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SubagentOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<ToolPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// Full specification for spawning a sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSpec {
    /// Profile name or reference (e.g. `"builtin:developer"`).
    pub profile: SubagentProfileRef,
    /// Optional human-readable label for this instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Input mode: prompt, snapshot, or both.
    pub input: SubagentInput,
    /// Per-spawn overrides.
    #[serde(default)]
    pub overrides: SubagentOverrides,
}

// ---------------------------------------------------------------------------
// Supervisor config
// ---------------------------------------------------------------------------

/// Configuration for the [`super::supervisor::SubagentSupervisor`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSupervisorConfig {
    /// Maximum concurrent running children (semaphore-based).
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    /// Hard cap on total active children (if set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_active_children: Option<usize>,
    /// Default timeout for user input requests in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_input_timeout_ms: Option<u64>,
    /// Whether to abort all children when the supervisor is dropped.
    #[serde(default = "default_true")]
    pub abort_on_drop: bool,
}

fn default_max_concurrent() -> usize {
    4
}

fn default_true() -> bool {
    true
}

impl Default for SubagentSupervisorConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            max_active_children: None,
            default_input_timeout_ms: None,
            abort_on_drop: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Status & lifecycle
// ---------------------------------------------------------------------------

/// Lifecycle status of a sub-agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Aborted,
}

// ---------------------------------------------------------------------------
// Routing DTOs
// ---------------------------------------------------------------------------

/// Request payload for delegating a task to a sub-agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelegateSubagentRequest {
    pub profile: Option<SubagentProfileRef>,
    pub task: String,
    pub label: Option<String>,
    #[serde(default)]
    pub run_in_background: bool,
}

/// Compact artifact emitted by a completed sub-agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentArtifact {
    pub kind: String,
    pub title: String,
    pub content: String,
}

/// Compact controller-facing result for a delegated sub-agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DelegateSubagentResult {
    pub subagent_id: SubagentId,
    pub profile_id: SubagentProfileRef,
    pub status: SubagentStatus,
    pub summary: String,
    pub artifacts: Vec<SubagentArtifact>,
    pub child_thread_id: Option<ThreadId>,
    pub usage: Option<serde_json::Value>,
    pub error: Option<String>,
}

/// Caller identity for sub-agent management operations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentCaller {
    pub is_main_agent: bool,
    pub depth: u32,
    pub source_subagent_id: Option<SubagentId>,
}

impl SubagentCaller {
    /// Build a caller identity for the main/default agent.
    pub fn main_agent() -> Self {
        Self {
            is_main_agent: true,
            depth: 0,
            source_subagent_id: None,
        }
    }

    /// Build a caller identity for a child sub-agent.
    pub fn child(source: SubagentId, depth: u32) -> Self {
        Self {
            is_main_agent: false,
            depth,
            source_subagent_id: Some(source),
        }
    }
}

/// Compact summary of a child known to the routing controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentKnownChild {
    pub subagent_id: SubagentId,
    pub profile_id: SubagentProfileRef,
    pub label: Option<String>,
    pub status: SubagentStatus,
    pub model: Option<LanguageModel>,
}

/// Result for a sub-agent cancel request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentCancelResult {
    pub subagent_id: SubagentId,
    pub status: SubagentStatus,
    pub canceled: bool,
}

/// Result for a parent-to-child message send request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendSubagentMessageResult {
    pub subagent_id: SubagentId,
    pub accepted: bool,
}

/// Parent-visible routing metadata for one known child sub-agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentRoutingMetadata {
    pub subagent_id: SubagentId,
    pub profile_id: SubagentProfileRef,
    pub label: Option<String>,
    pub model: Option<LanguageModel>,
    pub parent_tool_call_id: Option<String>,
    pub child_thread_id: Option<ThreadId>,
    pub source_subagent_id: Option<SubagentId>,
    pub target_subagent_id: Option<SubagentId>,
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Parent-facing events wrapping child lifecycle and agent events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SubagentEvent {
    Spawned {
        subagent_id: SubagentId,
        label: Option<String>,
        profile: String,
        model: Option<LanguageModel>,
    },
    StatusChanged {
        subagent_id: SubagentId,
        status: SubagentStatus,
    },
    AgentEvent {
        subagent_id: SubagentId,
        label: Option<String>,
        event: Box<AgentEvent>,
    },
    Completed {
        subagent_id: SubagentId,
        result: SubagentRunResult,
    },
    Failed {
        subagent_id: SubagentId,
        error: String,
    },
    Aborted {
        subagent_id: SubagentId,
    },
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// Outcome of a completed sub-agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentRunResult {
    pub subagent_id: SubagentId,
    pub status: SubagentStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<ModelMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Completion record returned by `wait_any` / `wait_all`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentCompletion {
    pub subagent_id: SubagentId,
    pub label: Option<String>,
    pub profile: String,
    pub result: SubagentRunResult,
}

/// Summary of an active sub-agent (for `list_active`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentSummary {
    pub subagent_id: SubagentId,
    pub label: Option<String>,
    pub profile: String,
    pub model: Option<LanguageModel>,
    pub status: SubagentStatus,
}

/// Enriched snapshot for `watch_snapshot()` on a sub-agent handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentSnapshot {
    pub subagent_id: SubagentId,
    pub profile: String,
    pub label: Option<String>,
    pub model: Option<LanguageModel>,
    pub status: SubagentStatus,
    pub turn_index: usize,
    pub message_count: usize,
    pub is_streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subagent_profile_default_has_version_1() {
        let profile = SubagentProfile::default();
        assert_eq!(profile.version, 1);
        assert!(profile.name.is_empty());
        assert!(profile.display_name.is_none());
        assert!(profile.infer.is_none());
        assert_eq!(profile.tools, ToolPolicy::Inherit);
        assert!(profile.skills.is_empty());
        assert!(profile.mcp_servers.is_empty());
        assert!(profile.default_agent_excluded_tools.is_empty());
        assert!(profile.excluded_tools.is_empty());
        assert!(profile.models.is_empty());
        assert!(!profile.default);
    }

    #[test]
    fn supervisor_config_default_values() {
        let config = SubagentSupervisorConfig::default();
        assert_eq!(config.max_concurrent, 4);
        assert!(config.max_active_children.is_none());
        assert!(config.default_input_timeout_ms.is_none());
        assert!(config.abort_on_drop);
    }

    #[test]
    fn builtin_profiles_have_correct_names() {
        assert_eq!(
            SubagentProfile::builtin_developer().name,
            "builtin:developer"
        );
        assert_eq!(SubagentProfile::builtin_planner().name, "builtin:planner");
        assert_eq!(SubagentProfile::builtin_explorer().name, "builtin:explorer");
    }

    #[test]
    fn builtin_profiles_have_system_prompts() {
        assert!(SubagentProfile::builtin_developer().system_prompt.is_some());
        assert!(SubagentProfile::builtin_planner().system_prompt.is_some());
        assert!(SubagentProfile::builtin_explorer().system_prompt.is_some());
    }

    #[test]
    fn builtin_profiles_have_listing_fields() {
        let developer = SubagentProfile::builtin_developer();
        assert_eq!(developer.display_name.as_deref(), Some("Developer"));
        assert!(developer
            .infer
            .as_deref()
            .unwrap()
            .contains("implementation"));
    }

    #[test]
    fn tool_policy_serde_roundtrip() {
        let policies = vec![
            ToolPolicy::Inherit,
            ToolPolicy::Replace {
                tools: vec!["read".into(), "write".into()],
            },
            ToolPolicy::InheritWithOverrides {
                add: vec!["search".into()],
                remove: vec!["delete".into()],
            },
        ];
        for policy in &policies {
            let json = serde_json::to_string(policy).unwrap();
            let deserialized: ToolPolicy = serde_json::from_str(&json).unwrap();
            assert_eq!(&deserialized, policy);
        }
    }

    #[test]
    fn subagent_input_serde_roundtrip() {
        let inputs = vec![
            SubagentInput::Prompt {
                task: "fix bug".into(),
            },
            SubagentInput::Snapshot {
                mode: SnapshotMode::SummaryOnly,
            },
            SubagentInput::PromptWithSnapshot {
                task: "implement feature".into(),
                mode: SnapshotMode::FullReadonlySnapshot,
            },
        ];
        for input in &inputs {
            let json = serde_json::to_string(input).unwrap();
            let deserialized: SubagentInput = serde_json::from_str(&json).unwrap();
            let re_json = serde_json::to_string(&deserialized).unwrap();
            assert_eq!(json, re_json);
        }
    }

    #[test]
    fn subagent_status_serde_roundtrip() {
        let statuses = vec![
            SubagentStatus::Pending,
            SubagentStatus::Running,
            SubagentStatus::Completed,
            SubagentStatus::Failed,
            SubagentStatus::Aborted,
        ];
        for status in &statuses {
            let json = serde_json::to_string(status).unwrap();
            let deserialized: SubagentStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(&deserialized, status);
        }
    }

    #[test]
    fn subagent_event_serde_roundtrip() {
        let id = SubagentId::nil();
        let events = vec![
            SubagentEvent::Spawned {
                subagent_id: id,
                label: Some("test".into()),
                profile: "builtin:developer".into(),
                model: None,
            },
            SubagentEvent::StatusChanged {
                subagent_id: id,
                status: SubagentStatus::Running,
            },
            SubagentEvent::Aborted { subagent_id: id },
        ];
        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let deserialized: SubagentEvent = serde_json::from_str(&json).unwrap();
            let re_json = serde_json::to_string(&deserialized).unwrap();
            assert_eq!(json, re_json);
        }
    }

    #[test]
    fn model_candidate_serde_roundtrip() {
        let candidate = ModelCandidate {
            provider: "anthropic".into(),
            model: "claude-sonnet-4.5".into(),
            reasoning_effort: Some("medium".into()),
        };
        let json = serde_json::to_string(&candidate).unwrap();
        let deserialized: ModelCandidate = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, candidate);
    }

    #[test]
    fn subagent_profile_serde_roundtrip() {
        let profile = SubagentProfile {
            skills: vec!["rust-skills".into()],
            mcp_servers: vec!["github".into()],
            default_agent_excluded_tools: vec!["dangerous-delete".into()],
            excluded_tools: vec!["shell".into()],
            default: true,
            ..SubagentProfile::builtin_developer()
        };
        let json = serde_json::to_string(&profile).unwrap();
        let deserialized: SubagentProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, profile);
    }

    #[test]
    fn profile_summary_omits_runtime_prompt_fields() {
        let profile = SubagentProfile {
            skills: vec!["rust-skills".into()],
            mcp_servers: vec!["github".into()],
            default_agent_excluded_tools: vec!["dangerous-delete".into()],
            excluded_tools: vec!["shell".into()],
            default: true,
            ..SubagentProfile::builtin_developer()
        };
        let summary = SubagentProfileSummary::from(&profile);
        assert_eq!(summary.name, "builtin:developer");
        assert_eq!(summary.display_name.as_deref(), Some("Developer"));
        assert_eq!(summary.skills, vec!["rust-skills"]);
        assert_eq!(summary.mcp_servers, vec!["github"]);
        assert_eq!(
            summary.default_agent_excluded_tools,
            vec!["dangerous-delete"]
        );
        assert_eq!(summary.excluded_tools, vec!["shell"]);
        assert!(summary.default);
    }

    #[test]
    fn subagent_context_default_is_empty() {
        let ctx = SubagentContext::default();
        assert!(ctx.summary.is_none());
        assert!(ctx.selected_messages.is_empty());
        assert!(ctx.file_hints.is_empty());
    }

    #[test]
    fn subagent_overrides_default_is_none() {
        let overrides = SubagentOverrides::default();
        assert!(overrides.system_prompt.is_none());
        assert!(overrides.model.is_none());
        assert!(overrides.tools.is_none());
        assert!(overrides.timeout_ms.is_none());
    }

    #[test]
    fn subagent_snapshot_serde_roundtrip() {
        let snapshot = SubagentSnapshot {
            subagent_id: SubagentId::nil(),
            profile: "builtin:developer".into(),
            label: Some("worker-1".into()),
            model: None,
            status: SubagentStatus::Running,
            turn_index: 3,
            message_count: 7,
            is_streaming: true,
            last_error: None,
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let deserialized: SubagentSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, snapshot);
    }

    #[test]
    fn profile_version_defaults_to_1_from_json() {
        let json = r#"{"name":"test","tools":{"mode":"inherit"},"models":[]}"#;
        let profile: SubagentProfile = serde_json::from_str(json).unwrap();
        assert_eq!(profile.version, 1);
    }

    #[test]
    fn profile_new_fields_default_from_json() {
        let json = r#"{"name":"test"}"#;
        let profile: SubagentProfile = serde_json::from_str(json).unwrap();
        assert!(profile.display_name.is_none());
        assert!(profile.infer.is_none());
        assert!(profile.skills.is_empty());
        assert!(profile.mcp_servers.is_empty());
        assert!(profile.default_agent_excluded_tools.is_empty());
        assert!(profile.excluded_tools.is_empty());
        assert!(!profile.default);
    }

    #[test]
    fn delegate_subagent_request_subagent_routing_dto_defaults_foreground_from_minimal_json() {
        let request: DelegateSubagentRequest =
            serde_json::from_str(r#"{ "task": "Find runtime wiring" }"#).unwrap();

        assert_eq!(request.task, "Find runtime wiring");
        assert!(request.profile.is_none());
        assert!(request.label.is_none());
        assert!(!request.run_in_background);
    }

    #[test]
    fn delegate_subagent_result_subagent_routing_dto_serde_roundtrip() {
        let child_thread_id = ThreadId::new();
        let result = DelegateSubagentResult {
            subagent_id: SubagentId::nil(),
            profile_id: "builtin:developer".into(),
            status: SubagentStatus::Completed,
            summary: "runtime wiring found".into(),
            artifacts: vec![SubagentArtifact {
                kind: "text".into(),
                title: "notes".into(),
                content: "child result".into(),
            }],
            child_thread_id: Some(child_thread_id),
            usage: Some(serde_json::json!({ "input_tokens": 12 })),
            error: None,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: DelegateSubagentResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized, result);
    }

    #[test]
    fn subagent_caller_subagent_routing_dto_serde_roundtrip() {
        let callers = [
            SubagentCaller::main_agent(),
            SubagentCaller::child(SubagentId::nil(), 1),
        ];

        for caller in callers {
            let json = serde_json::to_string(&caller).unwrap();
            let deserialized: SubagentCaller = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, caller);
        }
    }

    #[test]
    fn subagent_cancel_result_subagent_routing_dto_serde_roundtrip() {
        let result = SubagentCancelResult {
            subagent_id: SubagentId::nil(),
            status: SubagentStatus::Aborted,
            canceled: true,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: SubagentCancelResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized, result);
    }

    #[test]
    fn send_subagent_message_result_subagent_routing_dto_serde_roundtrip() {
        let result = SendSubagentMessageResult {
            subagent_id: SubagentId::nil(),
            accepted: true,
        };

        let json = serde_json::to_string(&result).unwrap();
        let deserialized: SendSubagentMessageResult = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized, result);
    }
}
