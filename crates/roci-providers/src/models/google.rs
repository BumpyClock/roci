//! Google Gemini model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::ModelCapabilities;

/// Google Gemini models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum GoogleModel {
    #[strum(serialize = "gemini-2.5-pro")]
    Gemini25Pro,
    #[strum(serialize = "gemini-2.5-flash")]
    Gemini25Flash,
    #[strum(serialize = "gemini-2.5-flash-lite")]
    Gemini25FlashLite,
    #[strum(serialize = "gemini-2.0-flash")]
    Gemini20Flash,
    #[strum(serialize = "gemini-3-flash")]
    Gemini3Flash,
    #[strum(serialize = "gemini-3-flash-preview")]
    Gemini3FlashPreview,
    #[strum(serialize = "gemini-3-pro-preview")]
    Gemini3ProPreview,
    #[strum(serialize = "gemini-1.5-pro")]
    Gemini15Pro,
    #[strum(serialize = "gemini-1.5-flash")]
    Gemini15Flash,
    /// Custom/unknown Google model.
    #[strum(default)]
    Custom(String),
}

impl GoogleModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Gemini25Pro => "gemini-2.5-pro",
            Self::Gemini25Flash => "gemini-2.5-flash",
            Self::Gemini25FlashLite => "gemini-2.5-flash-lite",
            Self::Gemini20Flash => "gemini-2.0-flash",
            Self::Gemini3Flash => "gemini-3-flash",
            Self::Gemini3FlashPreview => "gemini-3-flash-preview",
            Self::Gemini3ProPreview => "gemini-3-pro-preview",
            Self::Gemini15Pro => "gemini-1.5-pro",
            Self::Gemini15Flash => "gemini-1.5-flash",
            Self::Custom(s) => s,
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        let ctx = match self {
            Self::Gemini25Pro | Self::Gemini25Flash | Self::Gemini25FlashLite => 1_000_000,
            Self::Gemini20Flash
            | Self::Gemini3Flash
            | Self::Gemini3FlashPreview
            | Self::Gemini3ProPreview => 1_000_000,
            Self::Gemini15Pro | Self::Gemini15Flash => 2_000_000,
            Self::Custom(_) => 1_000_000,
        };
        ModelCapabilities {
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: true,
            supports_reasoning: matches!(self, Self::Gemini25Pro | Self::Gemini25Flash),
            supports_system_messages: true,
            context_length: ctx,
            max_output_tokens: Some(8_192),
        }
    }
}
