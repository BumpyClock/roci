//! Anthropic model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use super::capabilities::ModelCapabilities;

/// Anthropic models.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum AnthropicModel {
    #[strum(serialize = "claude-opus-4-5-20251101")]
    ClaudeOpus45,
    #[strum(serialize = "claude-sonnet-4-5-20250514")]
    ClaudeSonnet45,
    #[strum(serialize = "claude-sonnet-4-20250514")]
    ClaudeSonnet4,
    #[strum(serialize = "claude-haiku-3-5-20241022")]
    ClaudeHaiku35,
    #[strum(serialize = "claude-3-opus-20240229")]
    Claude3Opus,
    #[strum(serialize = "claude-3-sonnet-20240229")]
    Claude3Sonnet,
    #[strum(serialize = "claude-3-haiku-20240307")]
    Claude3Haiku,
    /// Custom/unknown Anthropic model by ID.
    #[strum(default)]
    Custom(String),
}

impl AnthropicModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::ClaudeOpus45 => "claude-opus-4-5-20251101",
            Self::ClaudeSonnet45 => "claude-sonnet-4-5-20250514",
            Self::ClaudeSonnet4 => "claude-sonnet-4-20250514",
            Self::ClaudeHaiku35 => "claude-haiku-3-5-20241022",
            Self::Claude3Opus => "claude-3-opus-20240229",
            Self::Claude3Sonnet => "claude-3-sonnet-20240229",
            Self::Claude3Haiku => "claude-3-haiku-20240307",
            Self::Custom(s) => s,
        }
    }

    /// Whether this model supports extended thinking.
    pub fn supports_extended_thinking(&self) -> bool {
        matches!(
            self,
            Self::ClaudeOpus45 | Self::ClaudeSonnet45 | Self::ClaudeSonnet4
        )
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        let ctx = match self {
            Self::ClaudeOpus45 | Self::ClaudeSonnet45 | Self::ClaudeSonnet4 => 200_000,
            Self::ClaudeHaiku35 => 200_000,
            Self::Claude3Opus | Self::Claude3Sonnet | Self::Claude3Haiku => 200_000,
            Self::Custom(_) => 200_000,
        };
        ModelCapabilities {
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: false, // Anthropic uses tool_use for structured output
            supports_json_schema: false,
            supports_reasoning: self.supports_extended_thinking(),
            supports_system_messages: true,
            context_length: ctx,
            max_output_tokens: Some(8_192),
        }
    }
}
