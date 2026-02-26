//! Mistral model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::ModelCapabilities;

/// Mistral models (OpenAI-compatible API).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum MistralModel {
    #[strum(serialize = "mistral-large-latest")]
    MistralLarge,
    #[strum(serialize = "mistral-medium-latest")]
    MistralMedium,
    #[strum(serialize = "mistral-small-latest")]
    MistralSmall,
    #[strum(serialize = "codestral-latest")]
    Codestral,
    #[strum(default)]
    Custom(String),
}

impl MistralModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::MistralLarge => "mistral-large-latest",
            Self::MistralMedium => "mistral-medium-latest",
            Self::MistralSmall => "mistral-small-latest",
            Self::Codestral => "codestral-latest",
            Self::Custom(s) => s,
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            supports_vision: matches!(self, Self::MistralLarge),
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: false,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length: 128_000,
            max_output_tokens: Some(8_192),
        }
    }
}
