//! LMStudio local model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use super::capabilities::ModelCapabilities;

/// LMStudio models (local, OpenAI-compatible API).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum LmStudioModel {
    /// LMStudio always serves whatever model is loaded, referenced by ID.
    #[strum(default)]
    Custom(String),
}

impl LmStudioModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Custom(s) => s,
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            supports_vision: false,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: false,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length: 32_768,
            max_output_tokens: None,
        }
    }
}
