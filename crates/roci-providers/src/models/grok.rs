//! xAI Grok model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::{ModelCapabilities, ModelInputCapabilities};

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
        let (context_length, supports_reasoning) = match self {
            Self::Grok3 => (131_072, false),
            Self::Grok3Mini => (131_072, true),
            Self::Grok4 => (256_000, true),
            Self::Grok41Fast => (2_000_000, true),
            Self::Custom(_) => (131_072, false),
        };

        ModelCapabilities {
            supports_vision: true,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: false,
            supports_reasoning,
            reasoning_effort: Default::default(),
            supports_system_messages: true,
            context_length,
            max_output_tokens: Some(16_384),
            input: ModelInputCapabilities::from_vision_support(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::GrokModel;

    #[test]
    fn capabilities_match_grok_context_windows() {
        assert_eq!(GrokModel::Grok3.capabilities().context_length, 131_072);
        assert_eq!(GrokModel::Grok3Mini.capabilities().context_length, 131_072);
        assert_eq!(GrokModel::Grok4.capabilities().context_length, 256_000);
        assert_eq!(
            GrokModel::Grok41Fast.capabilities().context_length,
            2_000_000
        );
    }

    #[test]
    fn capabilities_mark_reasoning_models() {
        assert!(!GrokModel::Grok3.capabilities().supports_reasoning);
        assert!(GrokModel::Grok3Mini.capabilities().supports_reasoning);
        assert!(GrokModel::Grok4.capabilities().supports_reasoning);
        assert!(GrokModel::Grok41Fast.capabilities().supports_reasoning);
    }
}
