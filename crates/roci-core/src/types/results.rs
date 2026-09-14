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
    /// Tool calls returned by the provider; these are not executed.
    pub tool_calls: Vec<super::message::AgentToolCall>,
    /// Input messages sent to the provider.
    pub messages: Vec<ModelMessage>,
    /// Token usage for this call.
    pub usage: Usage,
    /// Why generation finished.
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
