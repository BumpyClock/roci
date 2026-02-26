//! Model selection and parsing.

use std::str::FromStr;

use super::LanguageModel;
use crate::error::RociError;

/// Parse a "provider:model" string into a LanguageModel.
pub struct ModelSelector;

impl ModelSelector {
    /// Parse "provider:model_id" into a LanguageModel.
    ///
    /// All parsed models produce `LanguageModel::Known`. Validation of whether
    /// the provider is registered happens at `ProviderRegistry::create_provider()`
    /// time, not at parse time.
    ///
    /// Examples: "openai:gpt-4o", "anthropic:claude-opus-4-5-20251101", "ollama:llama3.3"
    pub fn parse(s: &str) -> Result<LanguageModel, RociError> {
        let (provider, model_id) = s.split_once(':').ok_or_else(|| {
            RociError::InvalidArgument(format!(
                "Invalid model selector '{s}': expected 'provider:model_id'"
            ))
        })?;

        Ok(LanguageModel::Known {
            provider_key: provider.to_string(),
            model_id: model_id.to_string(),
        })
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
    fn parse_unknown_provider_becomes_known() {
        let model = ModelSelector::parse("somecloud:my-model").unwrap();
        assert_eq!(model.provider_name(), "somecloud");
        assert_eq!(model.model_id(), "my-model");
        assert!(matches!(model, LanguageModel::Known { .. }));
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
