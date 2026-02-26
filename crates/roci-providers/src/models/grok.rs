//! xAI Grok model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::ModelCapabilities;

/// Grok models (OpenAI-compatible API).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum GrokModel {
    #[strum(serialize = "grok-3")]
    Grok3,
    #[strum(serialize = "grok-3-mini")]
    Grok3Mini,
    #[strum(serialize = "grok-4")]
    Grok4,
    #[strum(serialize = "grok-4.1-fast")]
    Grok41Fast,
    #[strum(default)]
    Custom(String),
}

impl GrokModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Grok3 => "grok-3",
            Self::Grok3Mini => "grok-3-mini",
            Self::Grok4 => "grok-4",
            Self::Grok41Fast => "grok-4.1-fast",
            Self::Custom(s) => s,
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: false,
            supports_reasoning: matches!(self, Self::Grok3Mini),
            supports_system_messages: true,
            context_length: 131_072,
            max_output_tokens: Some(16_384),
        }
    }
}
