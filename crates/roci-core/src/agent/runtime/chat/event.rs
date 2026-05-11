use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::domain::{
    ApprovalSnapshot, DiffSnapshot, HumanInteractionSnapshot, MessageSnapshot, MessageStatus,
    PlanSnapshot, ReasoningSnapshot, SessionResourceSnapshot, ThreadId, ToolExecutionSnapshot,
    ToolStatus, TurnId, TurnSnapshot,
};
use crate::agent::subagents::types::{
    DelegateSubagentResult, SubagentId, SubagentProfileRef, SubagentStatus,
};
use crate::agent_loop::RetryEvent;
use crate::models::LanguageModel;
use crate::types::{AgentToolResult, Role};

pub const AGENT_RUNTIME_EVENT_SCHEMA_VERSION: u16 = 1;

/// Cursor for replaying semantic runtime events for one thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RuntimeCursor {
    pub thread_id: ThreadId,
    pub seq: u64,
}

impl RuntimeCursor {
    #[must_use]
    pub const fn new(thread_id: ThreadId, seq: u64) -> Self {
        Self { thread_id, seq }
    }
}

/// Stable semantic event envelope emitted by `AgentRuntime`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRuntimeEvent {
    pub schema_version: u16,
    pub seq: u64,
    pub thread_id: ThreadId,
    pub turn_id: Option<TurnId>,
    pub timestamp: DateTime<Utc>,
    pub payload: AgentRuntimeEventPayload,
}

impl AgentRuntimeEvent {
    #[must_use]
    pub fn new(
        seq: u64,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
        payload: AgentRuntimeEventPayload,
    ) -> Self {
        Self {
            schema_version: AGENT_RUNTIME_EVENT_SCHEMA_VERSION,
            seq,
            thread_id,
            turn_id,
            timestamp: Utc::now(),
            payload,
        }
    }

    #[must_use]
    pub const fn cursor(&self) -> RuntimeCursor {
        RuntimeCursor::new(self.thread_id, self.seq)
    }
}

/// Parent-visible snapshot of one sub-agent at event time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentRuntimeSnapshot {
    pub subagent_id: SubagentId,
    pub profile_id: SubagentProfileRef,
    pub label: Option<String>,
    pub status: SubagentStatus,
    pub model: Option<LanguageModel>,
    pub parent_turn_id: Option<TurnId>,
    pub parent_tool_call_id: Option<String>,
    pub child_thread_id: Option<ThreadId>,
    pub source_subagent_id: Option<SubagentId>,
    pub target_subagent_id: Option<SubagentId>,
    pub sequence: u64,
}

/// Child message summary without parent-thread message ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentMessageSnapshot {
    pub role: Role,
    pub text: String,
    pub status: MessageStatus,
}

/// Child tool-call summary without parent-thread tool execution ids.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SubagentToolCallSnapshot {
    pub tool_call_id: String,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub result: Option<AgentToolResult>,
    pub status: ToolStatus,
}

/// Semantic runtime events. This set intentionally excludes raw provider loop
/// events and catch-all snapshot updates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentRuntimeEventPayload {
    TurnQueued {
        turn: TurnSnapshot,
    },
    TurnStarted {
        turn: TurnSnapshot,
    },
    MessageStarted {
        message: MessageSnapshot,
    },
    MessageUpdated {
        message: MessageSnapshot,
    },
    MessageCompleted {
        message: MessageSnapshot,
    },
    ToolStarted {
        tool: ToolExecutionSnapshot,
    },
    ToolUpdated {
        tool: ToolExecutionSnapshot,
    },
    ToolCompleted {
        tool: ToolExecutionSnapshot,
    },
    ApprovalRequired {
        approval: ApprovalSnapshot,
    },
    ApprovalResolved {
        approval: ApprovalSnapshot,
    },
    ApprovalCanceled {
        approval: ApprovalSnapshot,
    },
    HumanInteractionRequested {
        interaction: HumanInteractionSnapshot,
    },
    HumanInteractionResolved {
        interaction: HumanInteractionSnapshot,
    },
    HumanInteractionCanceled {
        interaction: HumanInteractionSnapshot,
    },
    ReasoningUpdated {
        reasoning: ReasoningSnapshot,
        delta: String,
    },
    PlanUpdated {
        plan: PlanSnapshot,
    },
    DiffUpdated {
        diff: DiffSnapshot,
    },
    Retry {
        event: RetryEvent,
    },
    PlanWritten {
        resource: SessionResourceSnapshot,
    },
    WorkspaceUpdated {
        resource: SessionResourceSnapshot,
    },
    ArtifactCreated {
        resource: SessionResourceSnapshot,
    },
    TempFileWritten {
        resource: SessionResourceSnapshot,
    },
    CheckpointCreated {
        resource: SessionResourceSnapshot,
    },
    SessionFileWritten {
        resource: SessionResourceSnapshot,
    },
    SessionFileDeleted {
        resource: SessionResourceSnapshot,
    },
    SubagentStarted {
        subagent: SubagentRuntimeSnapshot,
    },
    SubagentProgress {
        subagent: SubagentRuntimeSnapshot,
        message: Option<String>,
    },
    SubagentToolCallStarted {
        subagent: SubagentRuntimeSnapshot,
        tool: SubagentToolCallSnapshot,
    },
    SubagentToolCallCompleted {
        subagent: SubagentRuntimeSnapshot,
        tool: SubagentToolCallSnapshot,
    },
    SubagentMessage {
        subagent: SubagentRuntimeSnapshot,
        message: SubagentMessageSnapshot,
    },
    SubagentNeedsInput {
        subagent: SubagentRuntimeSnapshot,
        question: String,
        context: Option<String>,
    },
    SubagentCompleted {
        subagent: SubagentRuntimeSnapshot,
        result: DelegateSubagentResult,
    },
    SubagentFailed {
        subagent: SubagentRuntimeSnapshot,
        error: String,
    },
    SubagentCancelled {
        subagent: SubagentRuntimeSnapshot,
    },
    TurnCompleted {
        turn: TurnSnapshot,
    },
    TurnFailed {
        turn: TurnSnapshot,
        error: String,
    },
    TurnCanceled {
        turn: TurnSnapshot,
    },
}

impl AgentRuntimeEventPayload {
    #[must_use]
    pub const fn turn_queued_name() -> &'static str {
        "turn_queued"
    }

    #[must_use]
    pub const fn turn_started_name() -> &'static str {
        "turn_started"
    }

    #[must_use]
    pub const fn message_started_name() -> &'static str {
        "message_started"
    }

    #[must_use]
    pub const fn message_updated_name() -> &'static str {
        "message_updated"
    }

    #[must_use]
    pub const fn message_completed_name() -> &'static str {
        "message_completed"
    }

    #[must_use]
    pub const fn tool_started_name() -> &'static str {
        "tool_started"
    }

    #[must_use]
    pub const fn tool_updated_name() -> &'static str {
        "tool_updated"
    }

    #[must_use]
    pub const fn tool_completed_name() -> &'static str {
        "tool_completed"
    }

    #[must_use]
    pub const fn approval_required_name() -> &'static str {
        "approval_required"
    }

    #[must_use]
    pub const fn approval_resolved_name() -> &'static str {
        "approval_resolved"
    }

    #[must_use]
    pub const fn approval_canceled_name() -> &'static str {
        "approval_canceled"
    }

    #[must_use]
    pub const fn human_interaction_requested_name() -> &'static str {
        "human_interaction_requested"
    }

    #[must_use]
    pub const fn human_interaction_resolved_name() -> &'static str {
        "human_interaction_resolved"
    }

    #[must_use]
    pub const fn human_interaction_canceled_name() -> &'static str {
        "human_interaction_canceled"
    }

    #[must_use]
    pub const fn reasoning_updated_name() -> &'static str {
        "reasoning_updated"
    }

    #[must_use]
    pub const fn plan_updated_name() -> &'static str {
        "plan_updated"
    }

    #[must_use]
    pub const fn diff_updated_name() -> &'static str {
        "diff_updated"
    }

    #[must_use]
    pub const fn retry_name() -> &'static str {
        "retry"
    }

    #[must_use]
    pub const fn plan_written_name() -> &'static str {
        "plan_written"
    }

    #[must_use]
    pub const fn workspace_updated_name() -> &'static str {
        "workspace_updated"
    }

    #[must_use]
    pub const fn artifact_created_name() -> &'static str {
        "artifact_created"
    }

    #[must_use]
    pub const fn temp_file_written_name() -> &'static str {
        "temp_file_written"
    }

    #[must_use]
    pub const fn checkpoint_created_name() -> &'static str {
        "checkpoint_created"
    }

    #[must_use]
    pub const fn session_file_written_name() -> &'static str {
        "session_file_written"
    }

    #[must_use]
    pub const fn session_file_deleted_name() -> &'static str {
        "session_file_deleted"
    }

    #[must_use]
    pub const fn subagent_started_name() -> &'static str {
        "subagent_started"
    }

    #[must_use]
    pub const fn subagent_progress_name() -> &'static str {
        "subagent_progress"
    }

    #[must_use]
    pub const fn subagent_tool_call_started_name() -> &'static str {
        "subagent_tool_call_started"
    }

    #[must_use]
    pub const fn subagent_tool_call_completed_name() -> &'static str {
        "subagent_tool_call_completed"
    }

    #[must_use]
    pub const fn subagent_message_name() -> &'static str {
        "subagent_message"
    }

    #[must_use]
    pub const fn subagent_needs_input_name() -> &'static str {
        "subagent_needs_input"
    }

    #[must_use]
    pub const fn subagent_completed_name() -> &'static str {
        "subagent_completed"
    }

    #[must_use]
    pub const fn subagent_failed_name() -> &'static str {
        "subagent_failed"
    }

    #[must_use]
    pub const fn subagent_cancelled_name() -> &'static str {
        "subagent_cancelled"
    }

    #[must_use]
    pub const fn is_subagent_event(&self) -> bool {
        matches!(
            self,
            Self::SubagentStarted { .. }
                | Self::SubagentProgress { .. }
                | Self::SubagentToolCallStarted { .. }
                | Self::SubagentToolCallCompleted { .. }
                | Self::SubagentMessage { .. }
                | Self::SubagentNeedsInput { .. }
                | Self::SubagentCompleted { .. }
                | Self::SubagentFailed { .. }
                | Self::SubagentCancelled { .. }
        )
    }

    #[must_use]
    pub const fn turn_completed_name() -> &'static str {
        "turn_completed"
    }

    #[must_use]
    pub const fn turn_failed_name() -> &'static str {
        "turn_failed"
    }

    #[must_use]
    pub const fn turn_canceled_name() -> &'static str {
        "turn_canceled"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagents::types::{
        DelegateSubagentResult, SubagentArtifact, SubagentId, SubagentStatus,
    };
    use crate::models::LanguageModel;
    use crate::types::{AgentToolResult, Role};

    fn test_model() -> LanguageModel {
        LanguageModel::Known {
            provider_key: "test".to_string(),
            model_id: "semantic-events".to_string(),
        }
    }

    fn test_subagent() -> SubagentRuntimeSnapshot {
        let thread_id = ThreadId::new();
        SubagentRuntimeSnapshot {
            subagent_id: SubagentId::nil(),
            profile_id: "designer".to_string(),
            label: Some("ux pass".to_string()),
            status: SubagentStatus::Running,
            model: Some(test_model()),
            parent_turn_id: Some(TurnId::new(thread_id, 0, 1)),
            parent_tool_call_id: Some("parent-tool-1".to_string()),
            child_thread_id: Some(ThreadId::new()),
            source_subagent_id: None,
            target_subagent_id: Some(SubagentId::nil()),
            sequence: 7,
        }
    }

    fn test_tool(status: ToolStatus) -> SubagentToolCallSnapshot {
        SubagentToolCallSnapshot {
            tool_call_id: "child-tool-1".to_string(),
            tool_name: "inspect".to_string(),
            args: serde_json::json!({ "path": "src/lib.rs" }),
            result: (status == ToolStatus::Completed).then(|| AgentToolResult {
                tool_call_id: "child-tool-1".to_string(),
                result: serde_json::json!({ "ok": true }),
                is_error: false,
            }),
            status,
        }
    }

    fn test_result() -> DelegateSubagentResult {
        DelegateSubagentResult {
            subagent_id: SubagentId::nil(),
            profile_id: "designer".to_string(),
            status: SubagentStatus::Completed,
            summary: "done".to_string(),
            artifacts: vec![SubagentArtifact {
                kind: "note".to_string(),
                title: "summary".to_string(),
                content: "handoff".to_string(),
            }],
            child_thread_id: None,
            usage: Some(serde_json::json!({ "tokens": 12 })),
            error: None,
        }
    }

    #[test]
    fn subagent_runtime_wiring_payloads_serde_roundtrip() {
        let subagent = test_subagent();
        let payloads = vec![
            AgentRuntimeEventPayload::SubagentStarted {
                subagent: subagent.clone(),
            },
            AgentRuntimeEventPayload::SubagentProgress {
                subagent: subagent.clone(),
                message: Some("working".to_string()),
            },
            AgentRuntimeEventPayload::SubagentToolCallStarted {
                subagent: subagent.clone(),
                tool: test_tool(ToolStatus::Running),
            },
            AgentRuntimeEventPayload::SubagentToolCallCompleted {
                subagent: subagent.clone(),
                tool: test_tool(ToolStatus::Completed),
            },
            AgentRuntimeEventPayload::SubagentMessage {
                subagent: subagent.clone(),
                message: SubagentMessageSnapshot {
                    role: Role::Assistant,
                    text: "child answer".to_string(),
                    status: MessageStatus::Completed,
                },
            },
            AgentRuntimeEventPayload::SubagentNeedsInput {
                subagent: subagent.clone(),
                question: "Pick one?".to_string(),
                context: Some("Need design choice".to_string()),
            },
            AgentRuntimeEventPayload::SubagentCompleted {
                subagent: subagent.clone(),
                result: test_result(),
            },
            AgentRuntimeEventPayload::SubagentFailed {
                subagent: subagent.clone(),
                error: "boom".to_string(),
            },
            AgentRuntimeEventPayload::SubagentCancelled { subagent },
        ];

        for payload in payloads {
            let encoded = serde_json::to_string(&payload).expect("payload serializes");
            let decoded: AgentRuntimeEventPayload =
                serde_json::from_str(&encoded).expect("payload deserializes");

            assert_eq!(decoded, payload);
        }
    }
}
