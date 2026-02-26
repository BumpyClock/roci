//! Generation result types.

use serde::{Deserialize, Serialize};

use super::generation::FinishReason;
use super::message::ModelMessage;
use super::usage::Usage;

/// Result of a text generation call.
#[derive(Debug, Clone)]
pub struct GenerateTextResult {
    /// Final generated text.
    pub text: String,
    /// All generation steps (multi-step if tools were used).
    pub steps: Vec<GenerationStep>,
    /// Full message history including tool interactions.
    pub messages: Vec<ModelMessage>,
    /// Aggregated usage across all steps.
    pub usage: Usage,
    /// Why the final step finished.
    pub finish_reason: Option<FinishReason>,
}

/// A single generation step (one model call).
#[derive(Debug, Clone)]
pub struct GenerationStep {
    /// Text generated in this step.
    pub text: String,
    /// Tool calls made in this step, if any.
    pub tool_calls: Vec<super::message::AgentToolCall>,
    /// Tool results returned in this step, if any.
    pub tool_results: Vec<super::message::AgentToolResult>,
    /// Token usage for this step.
    pub usage: Usage,
    /// Finish reason for this step.
    pub finish_reason: Option<FinishReason>,
}

/// Result of a structured object generation call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerateObjectResult<T> {
    /// Deserialized object.
    pub object: T,
    /// Raw JSON text.
    pub raw_text: String,
    /// Token usage.
    pub usage: Usage,
    /// Finish reason.
    pub finish_reason: Option<FinishReason>,
}
