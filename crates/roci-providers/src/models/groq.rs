//! Groq model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::{ModelCapabilities, ModelInputCapabilities};

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
            Self::Llama3370bVersatile => 131_072,
            Self::Llama318bInstant => 128_000,
            Self::Mixtral8x7b => 32_768,
            Self::Custom(_) => 32_768,
        };
        let max_output = match self {
            Self::Llama3370bVersatile => 32_768,
            _ => 8_192,
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
            max_output_tokens: Some(max_output),
            input: ModelInputCapabilities::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groq_models_have_no_image_input_capabilities() {
        let caps = GroqModel::Llama3370bVersatile.capabilities();

        assert!(!caps.supports_vision);
        assert!(caps.input.image.is_none());
        assert_eq!(caps.supports_vision, caps.input.image.is_some());
    }

    #[test]
    fn llama_3_3_70b_has_current_groq_token_limits() {
        let caps = GroqModel::Llama3370bVersatile.capabilities();

        assert_eq!(caps.context_length, 131_072);
        assert_eq!(caps.max_output_tokens, Some(32_768));
    }
}
