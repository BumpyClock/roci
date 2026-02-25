//! Run event stream types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::message::ContentPart;
use crate::types::{AgentToolCall, AgentToolResult, ModelMessage, TextStreamDelta};

use super::approvals::ApprovalRequest;
use super::types::RunId;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    },

    // -- Turn boundaries --
    TurnStart {
        run_id: RunId,
        turn_index: usize,
    },
    TurnEnd {
        run_id: RunId,
        turn_index: usize,
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

    // -- Roci-specific events (kept for backward compat) --
    Approval {
        request: ApprovalRequest,
    },
    Reasoning {
        text: String,
    },
    Error {
        error: String,
    },
    System {
        message: String,
    },
}
