//! Run event stream types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::message::ContentPart;
use crate::types::{AgentToolCall, AgentToolResult, ModelMessage, TextStreamDelta};

use super::approvals::{ApprovalDecision, ApprovalRequest};
use super::types::RunId;

/// Retry behavior for provider failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryMode {
    Bounded { max_attempts: u32 },
    Persistent,
}

impl Default for RetryMode {
    fn default() -> Self {
        Self::Bounded { max_attempts: 3 }
    }
}

/// Retry lifecycle event kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryEventKind {
    RetryScheduled,
    RetryResuming,
    RetryCanceled,
    CandidateAdvancing,
    RetryExhausted,
}

/// Provider failure category used for retry and health decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureCategory {
    RateLimit,
    Network,
    Server,
    Timeout,
    Overflow,
    Auth,
    Configuration,
    InvalidRequest,
    Tool,
    Canceled,
    Unknown,
}

/// Action selected after a retry decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryNextAction {
    Sleep,
    ResumeSameCandidate,
    AdvanceCandidate,
    ReturnFailure,
    Cancel,
}

/// Retry and candidate-advancement event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryEvent {
    pub kind: RetryEventKind,
    pub run_id: RunId,
    pub provider: String,
    pub model_id: String,
    pub candidate_index: usize,
    pub attempt: u32,
    pub retry_mode: RetryMode,
    pub failure_category: FailureCategory,
    pub sleep_ms: Option<u64>,
    pub elapsed_retry_ms: u64,
    pub candidates_remaining: usize,
    pub partial_output_seen: bool,
    pub next_action: RetryNextAction,
}

/// Stream category for events.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunEventStream {
    Lifecycle,
    Assistant,
    Reasoning,
    Tool,
    Plan,
    Diff,
    Approval,
    System,
}

/// Run lifecycle event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunLifecycle {
    Started,
    Completed,
    Failed { error: String },
    Canceled,
}

/// Concrete event payloads emitted by the agent loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEventPayload {
    Lifecycle {
        state: RunLifecycle,
    },
    AssistantDelta {
        text: String,
    },
    ReasoningDelta {
        text: String,
    },
    ToolCallStarted {
        call: AgentToolCall,
    },
    ToolCallDelta {
        call_id: String,
        delta: serde_json::Value,
    },
    ToolCallCompleted {
        call: AgentToolCall,
    },
    ToolResult {
        result: AgentToolResult,
    },
    PlanUpdated {
        plan: String,
    },
    DiffUpdated {
        diff: String,
    },
    ApprovalRequired {
        request: ApprovalRequest,
    },
    Error {
        message: String,
    },
    Retry {
        event: RetryEvent,
    },
}

/// Envelope for streaming run events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunEvent {
    pub run_id: RunId,
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub stream: RunEventStream,
    pub payload: RunEventPayload,
}

// ---------------------------------------------------------------------------
// AgentEvent — pi-mono aligned event system
// ---------------------------------------------------------------------------

/// Partial result emitted during tool execution via the `on_update` callback.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolUpdatePayload {
    /// Content parts (text, images) produced so far.
    pub content: Vec<ContentPart>,
    /// Opaque details for UI or logging.
    #[serde(default)]
    pub details: serde_json::Value,
}

/// High-level agent events aligned with pi-mono's event system.
///
/// These events provide turn-level boundaries and streaming tool updates
/// in addition to the lower-level `RunEvent`/`RunEventPayload` events.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    // -- Lifecycle --
    AgentStart {
        run_id: RunId,
    },
    AgentEnd {
        run_id: RunId,
        messages: Vec<ModelMessage>,
    },

    // -- Turn boundaries --
    TurnStart {
        run_id: RunId,
        turn_index: usize,
    },
    TurnEnd {
        run_id: RunId,
        turn_index: usize,
        assistant_message: Option<ModelMessage>,
        tool_results: Vec<AgentToolResult>,
    },

    // -- Message streaming --
    MessageStart {
        message: ModelMessage,
    },
    MessageUpdate {
        message: ModelMessage,
        assistant_message_event: TextStreamDelta,
    },
    MessageEnd {
        message: ModelMessage,
    },

    // -- Tool execution (enhanced with streaming) --
    ToolExecutionStart {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
    },
    ToolExecutionUpdate {
        tool_call_id: String,
        tool_name: String,
        args: serde_json::Value,
        partial_result: ToolUpdatePayload,
    },
    ToolExecutionEnd {
        tool_call_id: String,
        tool_name: String,
        result: AgentToolResult,
        is_error: bool,
    },

    HumanInteractionRequested {
        request: crate::human_interaction::HumanInteractionRequest,
    },
    HumanInteractionResolved {
        response: crate::human_interaction::HumanInteractionResponse,
    },
    HumanInteractionCanceled {
        request_id: crate::human_interaction::HumanInteractionRequestId,
        reason: Option<String>,
    },

    // -- Roci-specific events --
    Approval {
        request: ApprovalRequest,
    },
    ApprovalResolved {
        request_id: String,
        decision: ApprovalDecision,
    },
    Reasoning {
        text: String,
    },
    PlanUpdated {
        plan: String,
    },
    DiffUpdated {
        diff: String,
    },
    Error {
        error: String,
    },
    System {
        message: String,
    },
}
