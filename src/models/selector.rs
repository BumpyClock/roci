//! Model selection and parsing.

use std::str::FromStr;

use super::LanguageModel;
use crate::error::RociError;

/// Parse a "provider:model" string into a LanguageModel.
pub struct ModelSelector;

impl ModelSelector {
    /// Parse "provider:model_id" into a LanguageModel.
    ///
    /// Examples: "openai:gpt-4o", "anthropic:claude-opus-4-5-20251101", "ollama:llama3.3"
    pub fn parse(s: &str) -> Result<LanguageModel, RociError> {
        let (provider, model_id) = s.split_once(':').ok_or_else(|| {
            RociError::InvalidArgument(format!(
                "Invalid model selector '{s}': expected 'provider:model_id'"
            ))
        })?;

        match provider {
            #[cfg(feature = "openai")]
            "openai" => {
                use super::openai::OpenAiModel;
                // Keep openai:* on the OpenAI provider path. Do not infer Codex from model-id substrings.
                let m = OpenAiModel::from_str(model_id)
                    .unwrap_or(OpenAiModel::Custom(model_id.to_string()));
                Ok(LanguageModel::OpenAi(m))
            }
            #[cfg(feature = "openai")]
            "openai-codex" | "openai_codex" | "codex" => {
                use super::openai::OpenAiModel;
                // Codex routing is explicit and provider-alias based.
                let m = OpenAiModel::from_str(model_id)
                    .unwrap_or(OpenAiModel::Custom(model_id.to_string()));
                Ok(LanguageModel::OpenAiCodex(m))
            }
            #[cfg(feature = "anthropic")]
            "anthropic" => {
                use super::anthropic::AnthropicModel;
                let m = AnthropicModel::from_str(model_id)
                    .unwrap_or(AnthropicModel::Custom(model_id.to_string()));
                Ok(LanguageModel::Anthropic(m))
            }
            #[cfg(feature = "google")]
            "google" | "gemini" => {
                use super::google::GoogleModel;
                let m = GoogleModel::from_str(model_id)
                    .unwrap_or(GoogleModel::Custom(model_id.to_string()));
                Ok(LanguageModel::Google(m))
            }
            #[cfg(feature = "grok")]
            "grok" | "xai" => {
                use super::grok::GrokModel;
                let m = GrokModel::from_str(model_id)
                    .unwrap_or(GrokModel::Custom(model_id.to_string()));
                Ok(LanguageModel::Grok(m))
            }
            #[cfg(feature = "groq")]
            "groq" => {
                use super::groq::GroqModel;
                let m = GroqModel::from_str(model_id)
                    .unwrap_or(GroqModel::Custom(model_id.to_string()));
                Ok(LanguageModel::Groq(m))
            }
            #[cfg(feature = "mistral")]
            "mistral" => {
                use super::mistral::MistralModel;
                let m = MistralModel::from_str(model_id)
                    .unwrap_or(MistralModel::Custom(model_id.to_string()));
                Ok(LanguageModel::Mistral(m))
            }
            #[cfg(feature = "ollama")]
            "ollama" => {
                use super::ollama::OllamaModel;
                let m = OllamaModel::from_str(model_id)
                    .unwrap_or(OllamaModel::Custom(model_id.to_string()));
                Ok(LanguageModel::Ollama(m))
            }
            #[cfg(feature = "lmstudio")]
            "lmstudio" => {
                use super::lmstudio::LmStudioModel;
                let m = LmStudioModel::from_str(model_id)
                    .unwrap_or(LmStudioModel::Custom(model_id.to_string()));
                Ok(LanguageModel::LmStudio(m))
            }
            #[cfg(feature = "openai-compatible")]
            "openai-compatible" | "openai_compatible" => {
                use super::openai_compatible::OpenAiCompatibleModel;
                Ok(LanguageModel::OpenAiCompatible(OpenAiCompatibleModel::new(
                    model_id, None,
                )))
            }
            #[cfg(feature = "openai-compatible")]
            "github-copilot" | "github_copilot" | "copilot" => {
                use super::openai_compatible::OpenAiCompatibleModel;
                Ok(LanguageModel::GitHubCopilot(OpenAiCompatibleModel::new(
                    model_id, None,
                )))
            }
            _ => Ok(LanguageModel::Custom {
                provider: provider.to_string(),
                model_id: model_id.to_string(),
            }),
        }
    }
}

impl FromStr for LanguageModel {
    type Err = RociError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ModelSelector::parse(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_openai_known_model() {
        let model = ModelSelector::parse("openai:gpt-4o").unwrap();
        assert_eq!(model.provider_name(), "openai");
        assert_eq!(model.model_id(), "gpt-4o");
    }

    #[test]
    fn parse_openai_custom_model() {
        let model = ModelSelector::parse("openai:ft:gpt-4o:my-org").unwrap();
        assert_eq!(model.model_id(), "ft:gpt-4o:my-org");
        assert_eq!(model.provider_name(), "openai");
    }

    #[test]
    fn parse_openai_codex_like_model_stays_openai() {
        let model = ModelSelector::parse("openai:gpt-5.3-codex-spark").unwrap();
        assert!(matches!(model, LanguageModel::OpenAi(_)));
        assert_eq!(model.model_id(), "gpt-5.3-codex-spark");
    }

    #[test]
    fn parse_openai_ft_codex_like_model_stays_openai() {
        let model = ModelSelector::parse("openai:ft:gpt-5.3-codex-spark:my-org").unwrap();
        assert!(matches!(model, LanguageModel::OpenAi(_)));
        assert_eq!(model.model_id(), "ft:gpt-5.3-codex-spark:my-org");
    }

    #[test]
    fn parse_codex_alias_models_route_to_openai_codex() {
        for provider_alias in ["openai-codex", "openai_codex", "codex"] {
            let model =
                ModelSelector::parse(&format!("{provider_alias}:gpt-5.3-codex-spark")).unwrap();
            assert!(
                matches!(model, LanguageModel::OpenAiCodex(_)),
                "expected OpenAiCodex for alias {provider_alias}, got {model:?}"
            );
            assert_eq!(model.model_id(), "gpt-5.3-codex-spark");
        }
    }

    #[test]
    fn parse_anthropic_model() {
        let model = ModelSelector::parse("anthropic:claude-opus-4-5-20251101").unwrap();
        assert_eq!(model.provider_name(), "anthropic");
        assert_eq!(model.model_id(), "claude-opus-4-5-20251101");
    }

    #[test]
    fn parse_google_model() {
        let model = ModelSelector::parse("google:gemini-2.5-pro").unwrap();
        assert_eq!(model.provider_name(), "google");
        assert_eq!(model.model_id(), "gemini-2.5-pro");
    }

    #[test]
    fn parse_unknown_provider_becomes_custom() {
        let model = ModelSelector::parse("somecloud:my-model").unwrap();
        assert_eq!(model.provider_name(), "somecloud");
        assert_eq!(model.model_id(), "my-model");
    }

    #[cfg(feature = "openai-compatible")]
    #[test]
    fn parse_github_copilot_model() {
        let model = ModelSelector::parse("github-copilot:gpt-5.2-codex").unwrap();
        assert_eq!(model.provider_name(), "github-copilot");
        assert_eq!(model.model_id(), "gpt-5.2-codex");
    }

    #[test]
    fn parse_missing_colon_is_error() {
        assert!(ModelSelector::parse("gpt-4o").is_err());
    }

    #[test]
    fn roundtrip_display_parse() {
        let model = ModelSelector::parse("openai:gpt-4o").unwrap();
        let s = model.to_string();
        let parsed: LanguageModel = s.parse().unwrap();
        assert_eq!(parsed.model_id(), "gpt-4o");
    }
}
