//! Mistral model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::{ModelCapabilities, ModelInputCapabilities};

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
        let vision = matches!(
            self,
            Self::MistralLarge | Self::MistralMedium | Self::MistralSmall
        );
        let context_length = match self {
            Self::MistralLarge => 131_072,
            Self::MistralMedium | Self::MistralSmall | Self::Codestral | Self::Custom(_) => 32_768,
        };

        ModelCapabilities {
            supports_vision: vision,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: false,
            supports_reasoning: false,
            supports_system_messages: true,
            context_length,
            max_output_tokens: Some(8_192),
            input: ModelInputCapabilities::from_vision_support(vision),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MistralModel;

    #[test]
    fn capabilities_match_mistral_model_context_windows() {
        assert_eq!(
            MistralModel::MistralLarge.capabilities().context_length,
            131_072
        );
        assert_eq!(
            MistralModel::MistralMedium.capabilities().context_length,
            32_768
        );
        assert_eq!(
            MistralModel::MistralSmall.capabilities().context_length,
            32_768
        );
        assert_eq!(
            MistralModel::Codestral.capabilities().context_length,
            32_768
        );
    }

    #[test]
    fn capabilities_enable_vision_for_chat_models_only() {
        assert!(MistralModel::MistralLarge.capabilities().supports_vision);
        assert!(MistralModel::MistralMedium.capabilities().supports_vision);
        assert!(MistralModel::MistralSmall.capabilities().supports_vision);
        assert!(!MistralModel::Codestral.capabilities().supports_vision);
    }
}
