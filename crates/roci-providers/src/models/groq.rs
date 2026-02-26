//! Groq model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::ModelCapabilities;

/// Groq models (OpenAI-compatible API).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum GroqModel {
    #[strum(serialize = "llama-3.3-70b-versatile")]
    Llama3370bVersatile,
    #[strum(serialize = "llama-3.1-8b-instant")]
    Llama318bInstant,
    #[strum(serialize = "mixtral-8x7b-32768")]
    Mixtral8x7b,
    #[strum(default)]
    Custom(String),
}

impl GroqModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Llama3370bVersatile => "llama-3.3-70b-versatile",
            Self::Llama318bInstant => "llama-3.1-8b-instant",
            Self::Mixtral8x7b => "mixtral-8x7b-32768",
            Self::Custom(s) => s,
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        let ctx = match self {
            Self::Llama3370bVersatile => 128_000,
            Self::Llama318bInstant => 128_000,
            Self::Mixtral8x7b => 32_768,
            Self::Custom(_) => 32_768,
        };
        ModelCapabilities {
            supports_vision: false,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: false,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length: ctx,
            max_output_tokens: Some(8_192),
        }
    }
}
