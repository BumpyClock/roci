//! OpenAI model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::ModelCapabilities;
use roci_core::types::ReasoningEffort;

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
    #[strum(serialize = "gpt-5.1")]
    Gpt51,
    #[strum(serialize = "gpt-5.2")]
    Gpt52,
    #[strum(serialize = "gpt-5-pro")]
    Gpt5Pro,
    #[strum(serialize = "gpt-5-mini")]
    Gpt5Mini,
    #[strum(serialize = "gpt-5-nano")]
    Gpt5Nano,
    #[strum(serialize = "gpt-5-thinking")]
    Gpt5Thinking,
    #[strum(serialize = "gpt-5-thinking-mini")]
    Gpt5ThinkingMini,
    #[strum(serialize = "gpt-5-thinking-nano")]
    Gpt5ThinkingNano,
    #[strum(serialize = "gpt-5-chat-latest")]
    Gpt5ChatLatest,
    #[strum(serialize = "gpt-4o-realtime-preview")]
    Gpt4oRealtimePreview,
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
            Self::Gpt51 => "gpt-5.1",
            Self::Gpt52 => "gpt-5.2",
            Self::Gpt5Pro => "gpt-5-pro",
            Self::Gpt5Mini => "gpt-5-mini",
            Self::Gpt5Nano => "gpt-5-nano",
            Self::Gpt5Thinking => "gpt-5-thinking",
            Self::Gpt5ThinkingMini => "gpt-5-thinking-mini",
            Self::Gpt5ThinkingNano => "gpt-5-thinking-nano",
            Self::Gpt5ChatLatest => "gpt-5-chat-latest",
            Self::Gpt4oRealtimePreview => "gpt-4o-realtime-preview",
            Self::Custom(s) => s,
        }
    }

    /// Whether this model uses the Responses API (vs Chat Completions).
    pub fn uses_responses_api(&self) -> bool {
        match self {
            Self::O3
            | Self::O3Mini
            | Self::O4Mini
            | Self::Gpt5
            | Self::Gpt51
            | Self::Gpt52
            | Self::Gpt5Pro
            | Self::Gpt5Mini
            | Self::Gpt5Nano
            | Self::Gpt5Thinking
            | Self::Gpt5ThinkingMini
            | Self::Gpt5ThinkingNano
            | Self::Gpt5ChatLatest => true,
            Self::Custom(id) => {
                let lower = id.to_ascii_lowercase();
                lower.starts_with("gpt-5")
                    || lower.starts_with("o3")
                    || lower.starts_with("o4")
                    || lower.contains("codex")
            }
            _ => false,
        }
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
            Self::Gpt5
            | Self::Gpt51
            | Self::Gpt5Pro
            | Self::Gpt5Mini
            | Self::Gpt5Nano
            | Self::Gpt5Thinking
            | Self::Gpt5ThinkingMini
            | Self::Gpt5ThinkingNano
            | Self::Gpt5ChatLatest => false,
            _ => true,
        }
    }

    /// Whether the model supports GPT-5 text verbosity controls.
    pub fn supports_text_verbosity(&self) -> bool {
        matches!(
            self,
            Self::Gpt5
                | Self::Gpt51
                | Self::Gpt52
                | Self::Gpt5Pro
                | Self::Gpt5Mini
                | Self::Gpt5Nano
                | Self::Gpt5Thinking
                | Self::Gpt5ThinkingMini
                | Self::Gpt5ThinkingNano
                | Self::Gpt5ChatLatest
        )
    }

    /// Whether this model is in the GPT-5 family.
    pub fn is_gpt5_family(&self) -> bool {
        matches!(
            self,
            Self::Gpt5
                | Self::Gpt51
                | Self::Gpt52
                | Self::Gpt5Pro
                | Self::Gpt5Mini
                | Self::Gpt5Nano
                | Self::Gpt5Thinking
                | Self::Gpt5ThinkingMini
                | Self::Gpt5ThinkingNano
                | Self::Gpt5ChatLatest
        )
    }

    /// Whether this model should be treated as GPT-5 for request shaping.
    pub fn is_gpt5_family_id(&self) -> bool {
        match self {
            Self::Custom(id) => id.starts_with("gpt-5"),
            _ => self.is_gpt5_family(),
        }
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
            Self::Gpt5
            | Self::Gpt51
            | Self::Gpt52
            | Self::Gpt5Pro
            | Self::Gpt5Mini
            | Self::Gpt5Nano
            | Self::Gpt5Thinking
            | Self::Gpt5ThinkingMini
            | Self::Gpt5ThinkingNano
            | Self::Gpt5ChatLatest => (400_000, true, true, true, true),
            Self::Gpt4oRealtimePreview => (128_000, true, true, false, true),
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
            max_output_tokens: Some(match self {
                Self::O1 | Self::O1Mini | Self::O1Pro | Self::O3 | Self::O3Mini | Self::O4Mini => {
                    100_000
                }
                Self::Gpt5
                | Self::Gpt51
                | Self::Gpt52
                | Self::Gpt5Pro
                | Self::Gpt5Mini
                | Self::Gpt5Nano
                | Self::Gpt5Thinking
                | Self::Gpt5ThinkingMini
                | Self::Gpt5ThinkingNano
                | Self::Gpt5ChatLatest => 128_000,
                _ => 16_384,
            }),
        }
    }
}
