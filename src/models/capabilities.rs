//! Model capabilities descriptor.

use serde::{Deserialize, Serialize};

/// Describes what a model can do.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ModelCapabilities {
    pub supports_vision: bool,
    pub supports_tools: bool,
    pub supports_streaming: bool,
    pub supports_json_mode: bool,
    pub supports_json_schema: bool,
    pub supports_reasoning: bool,
    pub supports_system_messages: bool,
    pub context_length: usize,
    pub max_output_tokens: Option<usize>,
}

impl Default for ModelCapabilities {
    fn default() -> Self {
        Self {
            supports_vision: false,
            supports_tools: false,
            supports_streaming: true,
            supports_json_mode: false,
            supports_json_schema: false,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length: 4096,
            max_output_tokens: None,
        }
    }
}

impl ModelCapabilities {
    /// Full-featured model capabilities.
    pub fn full(context_length: usize) -> Self {
        Self {
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: true,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length,
            max_output_tokens: None,
        }
    }
}
