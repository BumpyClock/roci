//! Streaming types.

use serde::{Deserialize, Serialize};

use super::usage::Usage;
use super::generation::FinishReason;

/// A delta emitted during streaming.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextStreamDelta {
    /// The incremental text chunk.
    pub text: String,
    /// Event type.
    pub event_type: StreamEventType,
    /// Finish reason (only on the final delta).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    /// Usage (typically only on the final delta).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

/// Type of stream event.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StreamEventType {
    /// Incremental text content.
    TextDelta,
    /// Tool call being built.
    ToolCallDelta,
    /// Stream started.
    Start,
    /// Stream finished.
    Done,
    /// Error during stream.
    Error,
}

/// Final result after consuming a text stream.
#[derive(Debug, Clone)]
pub struct StreamTextResult {
    /// Full accumulated text.
    pub text: String,
    /// Token usage.
    pub usage: Usage,
    /// Finish reason.
    pub finish_reason: Option<FinishReason>,
}
