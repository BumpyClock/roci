//! Run event stream types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::{AgentToolCall, AgentToolResult};

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
    Lifecycle { state: RunLifecycle },
    AssistantDelta { text: String },
    ReasoningDelta { text: String },
    ToolCallStarted { call: AgentToolCall },
    ToolCallDelta { call_id: String, delta: serde_json::Value },
    ToolCallCompleted { call: AgentToolCall },
    ToolResult { result: AgentToolResult },
    PlanUpdated { plan: String },
    DiffUpdated { diff: String },
    ApprovalRequired { request: ApprovalRequest },
    Error { message: String },
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
