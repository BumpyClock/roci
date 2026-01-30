//! OpenAI model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use super::capabilities::ModelCapabilities;
use crate::types::ReasoningEffort;

/// OpenAI models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum OpenAiModel {
    #[strum(serialize = "gpt-4o")]
    Gpt4o,
    #[strum(serialize = "gpt-4o-mini")]
    Gpt4oMini,
    #[strum(serialize = "gpt-4-turbo")]
    Gpt4Turbo,
    #[strum(serialize = "gpt-4")]
    Gpt4,
    #[strum(serialize = "gpt-3.5-turbo")]
    Gpt35Turbo,
    #[strum(serialize = "o1")]
    O1,
    #[strum(serialize = "o1-mini")]
    O1Mini,
    #[strum(serialize = "o1-pro")]
    O1Pro,
    #[strum(serialize = "o3")]
    O3,
    #[strum(serialize = "o3-mini")]
    O3Mini,
    #[strum(serialize = "o4-mini")]
    O4Mini,
    #[strum(serialize = "gpt-4.1")]
    Gpt41,
    #[strum(serialize = "gpt-4.1-mini")]
    Gpt41Mini,
    #[strum(serialize = "gpt-4.1-nano")]
    Gpt41Nano,
    #[strum(serialize = "gpt-5")]
    Gpt5,
    #[strum(serialize = "gpt-5.2")]
    Gpt52,
    #[strum(serialize = "gpt-5-mini")]
    Gpt5Mini,
    #[strum(serialize = "gpt-5-nano")]
    Gpt5Nano,
    /// Custom/unknown OpenAI model by ID.
    #[strum(default)]
    Custom(String),
}

impl OpenAiModel {
    /// Get the API model identifier.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Gpt4o => "gpt-4o",
            Self::Gpt4oMini => "gpt-4o-mini",
            Self::Gpt4Turbo => "gpt-4-turbo",
            Self::Gpt4 => "gpt-4",
            Self::Gpt35Turbo => "gpt-3.5-turbo",
            Self::O1 => "o1",
            Self::O1Mini => "o1-mini",
            Self::O1Pro => "o1-pro",
            Self::O3 => "o3",
            Self::O3Mini => "o3-mini",
            Self::O4Mini => "o4-mini",
            Self::Gpt41 => "gpt-4.1",
            Self::Gpt41Mini => "gpt-4.1-mini",
            Self::Gpt41Nano => "gpt-4.1-nano",
            Self::Gpt5 => "gpt-5",
            Self::Gpt52 => "gpt-5.2",
            Self::Gpt5Mini => "gpt-5-mini",
            Self::Gpt5Nano => "gpt-5-nano",
            Self::Custom(s) => s,
        }
    }

    /// Whether this model uses the Responses API (vs Chat Completions).
    pub fn uses_responses_api(&self) -> bool {
        matches!(
            self,
            Self::O3
                | Self::O3Mini
                | Self::O4Mini
                | Self::Gpt41
                | Self::Gpt41Mini
                | Self::Gpt41Nano
                | Self::Gpt5
                | Self::Gpt52
                | Self::Gpt5Mini
                | Self::Gpt5Nano
        )
    }

    /// Whether this is a reasoning model.
    pub fn is_reasoning(&self) -> bool {
        matches!(
            self,
            Self::O1 | Self::O1Mini | Self::O1Pro | Self::O3 | Self::O3Mini | Self::O4Mini
        )
    }

    /// Whether the model accepts sampling parameters in the Responses API.
    pub fn supports_sampling_params(&self, reasoning_effort: Option<ReasoningEffort>) -> bool {
        match self {
            Self::Gpt52 => matches!(reasoning_effort, Some(ReasoningEffort::None)),
            Self::Gpt5 | Self::Gpt5Mini | Self::Gpt5Nano => false,
            _ => true,
        }
    }

    /// Whether the model supports GPT-5 text verbosity controls.
    pub fn supports_text_verbosity(&self) -> bool {
        matches!(
            self,
            Self::Gpt5 | Self::Gpt52 | Self::Gpt5Mini | Self::Gpt5Nano
        )
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        let (ctx, vision, tools, reasoning, json_schema) = match self {
            Self::Gpt4o | Self::Gpt4oMini => (128_000, true, true, false, true),
            Self::Gpt4Turbo => (128_000, true, true, false, true),
            Self::Gpt4 => (8_192, false, true, false, false),
            Self::Gpt35Turbo => (16_385, false, true, false, false),
            Self::O1 | Self::O1Pro => (200_000, true, false, true, false),
            Self::O1Mini => (128_000, false, false, true, false),
            Self::O3 | Self::O3Mini | Self::O4Mini => (200_000, true, true, true, true),
            Self::Gpt41 | Self::Gpt41Mini | Self::Gpt41Nano => (1_000_000, true, true, false, true),
            Self::Gpt5 | Self::Gpt52 | Self::Gpt5Mini | Self::Gpt5Nano => {
                (1_000_000, true, true, true, true)
            }
            Self::Custom(_) => (128_000, true, true, false, true),
        };
        ModelCapabilities {
            supports_vision: vision,
            supports_tools: tools,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: json_schema,
            supports_reasoning: reasoning,
            supports_system_messages: !self.is_reasoning()
                || matches!(self, Self::O3 | Self::O3Mini | Self::O4Mini),
            context_length: ctx,
            max_output_tokens: Some(if self.is_reasoning() { 100_000 } else { 16_384 }),
        }
    }
}
