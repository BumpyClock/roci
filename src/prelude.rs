//! Convenience re-exports for common use.

pub use crate::error::{RociError, Result};
pub use crate::types::{
    ModelMessage, Role, ContentPart, GenerationSettings, Usage, FinishReason,
    GenerateTextResult, TextStreamDelta, StreamEventType, StreamTextResult,
};
pub use crate::models::LanguageModel;
pub use crate::provider::ModelProvider;
pub use crate::tools::{Tool, AgentTool, ToolArguments, AgentToolParameters};
pub use crate::config::RociConfig;
