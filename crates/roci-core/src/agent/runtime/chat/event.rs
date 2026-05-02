use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::domain::{
    ApprovalSnapshot, DiffSnapshot, HumanInteractionSnapshot, MessageSnapshot, PlanSnapshot,
    ReasoningSnapshot, ThreadId, ToolExecutionSnapshot, TurnId, TurnSnapshot,
};

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
