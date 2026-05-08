//! Static model catalogs for built-in providers.

use std::collections::BTreeMap;

use roci_core::models::{
    ModelCapabilities, ModelCatalog, ModelCatalogSource, ModelInfo, ModelPolicy,
};

fn model_info(
    provider_key: &str,
    model_id: &str,
    capabilities: ModelCapabilities,
    requires_credentials: bool,
    local: bool,
    default_for_provider: bool,
) -> ModelInfo {
    ModelInfo {
        provider_key: provider_key.to_string(),
        model_id: model_id.to_string(),
        display_name: Some(model_id.to_string()),
        capabilities,
        policy: ModelPolicy {
            requires_credentials,
            local,
            deprecated: false,
            default_for_provider,
        },
        source: ModelCatalogSource::Static,
        metadata: BTreeMap::new(),
    }
}

#[cfg(feature = "openai")]
pub fn openai_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::openai::OpenAiModel;

    let models = [
        OpenAiModel::Gpt4o,
        OpenAiModel::Gpt4oMini,
        OpenAiModel::Gpt4Turbo,
        OpenAiModel::Gpt4,
        OpenAiModel::Gpt35Turbo,
        OpenAiModel::Gpt4oRealtimePreview,
        OpenAiModel::Gpt41,
        OpenAiModel::Gpt41Mini,
        OpenAiModel::Gpt41Nano,
        OpenAiModel::O1,
        OpenAiModel::O1Mini,
        OpenAiModel::O1Pro,
        OpenAiModel::O3,
        OpenAiModel::O3Mini,
        OpenAiModel::O4Mini,
        OpenAiModel::Gpt5,
        OpenAiModel::Gpt51,
        OpenAiModel::Gpt52,
        OpenAiModel::Gpt5Pro,
        OpenAiModel::Gpt5Mini,
        OpenAiModel::Gpt5Nano,
        OpenAiModel::Gpt5Thinking,
        OpenAiModel::Gpt5ThinkingMini,
        OpenAiModel::Gpt5ThinkingNano,
        OpenAiModel::Gpt5ChatLatest,
    ];

    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(
            provider_key,
            &id,
            model.capabilities(),
            true,
            false,
            id == "gpt-4o",
        )
    }))
}

#[cfg(feature = "openai")]
pub fn codex_catalog(provider_key: &str) -> ModelCatalog {
    let mut catalog = openai_catalog(provider_key);
    catalog.update_models(|model| {
        model.policy.default_for_provider = model.model_id == "gpt-5";
    });
    catalog
}

#[cfg(feature = "anthropic")]
pub fn anthropic_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::anthropic::AnthropicModel;

    let models = [
        AnthropicModel::ClaudeOpus45,
        AnthropicModel::ClaudeSonnet45,
        AnthropicModel::ClaudeSonnet4,
        AnthropicModel::ClaudeHaiku35,
        AnthropicModel::Claude3Opus,
        AnthropicModel::Claude3Sonnet,
        AnthropicModel::Claude3Haiku,
    ];

    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}

#[cfg(feature = "google")]
pub fn google_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::google::GoogleModel;

    let models = [
        GoogleModel::Gemini25Pro,
        GoogleModel::Gemini25Flash,
        GoogleModel::Gemini25FlashLite,
        GoogleModel::Gemini20Flash,
        GoogleModel::Gemini3Flash,
        GoogleModel::Gemini3FlashPreview,
        GoogleModel::Gemini3ProPreview,
        GoogleModel::Gemini15Pro,
        GoogleModel::Gemini15Flash,
    ];

    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(
            provider_key,
            &id,
            model.capabilities(),
            true,
            false,
            id == "gemini-2.5-pro",
        )
    }))
}

#[cfg(feature = "grok")]
pub fn grok_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::grok::GrokModel;

    let models = [
        GrokModel::Grok3,
        GrokModel::Grok3Mini,
        GrokModel::Grok4,
        GrokModel::Grok41Fast,
    ];

    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}

#[cfg(feature = "groq")]
pub fn groq_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::groq::GroqModel;

    let models = [
        GroqModel::Llama3370bVersatile,
        GroqModel::Llama318bInstant,
        GroqModel::Mixtral8x7b,
    ];

    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}

#[cfg(feature = "mistral")]
pub fn mistral_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::mistral::MistralModel;

    let models = [
        MistralModel::MistralLarge,
        MistralModel::MistralMedium,
        MistralModel::MistralSmall,
        MistralModel::Codestral,
    ];

    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}

#[cfg(feature = "ollama")]
pub fn ollama_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::ollama::OllamaModel;

    let models = [
        OllamaModel::Llama33,
        OllamaModel::Llama31,
        OllamaModel::Mistral,
        OllamaModel::CodeLlama,
        OllamaModel::DeepseekR1,
        OllamaModel::Qwen25,
    ];

    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(
            provider_key,
            &id,
            model.capabilities(),
            false,
            true,
            id == "llama3.3",
        )
    }))
}

#[cfg(feature = "lmstudio")]
pub fn lmstudio_catalog(_provider_key: &str) -> ModelCatalog {
    ModelCatalog::default()
}

#[cfg(feature = "github-copilot")]
pub fn github_copilot_static_catalog(provider_key: &str) -> ModelCatalog {
    use crate::models::openai::OpenAiModel;

    let models = [
        OpenAiModel::Gpt41,
        OpenAiModel::Gpt41Mini,
        OpenAiModel::Gpt5,
        OpenAiModel::Gpt5Mini,
        OpenAiModel::O4Mini,
    ];

    ModelCatalog::from_models(models.into_iter().map(|model| {
        let id = model.as_str().to_string();
        model_info(provider_key, &id, model.capabilities(), true, false, false)
    }))
}

pub fn empty_catalog(_provider_key: &str) -> ModelCatalog {
    ModelCatalog::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "openai")]
    #[test]
    fn openai_static_catalog_contains_gpt4o_with_capabilities() {
        let catalog = openai_catalog("openai");
        let gpt4o = catalog
            .models()
            .iter()
            .find(|model| model.model_id == "gpt-4o")
            .expect("gpt-4o present");

        assert_eq!(gpt4o.provider_key, "openai");
        assert!(gpt4o.capabilities.supports_vision);
        assert!(gpt4o.capabilities.supports_tools);
        assert!(gpt4o.policy.requires_credentials);
        assert!(!gpt4o.policy.local);
    }

    #[cfg(feature = "google")]
    #[test]
    fn google_static_catalog_contains_gemini_with_vision() {
        let catalog = google_catalog("google");
        let gemini = catalog
            .models()
            .iter()
            .find(|model| model.model_id == "gemini-2.5-pro")
            .expect("gemini present");

        assert!(gemini.capabilities.supports_vision);
        assert_eq!(gemini.provider_key, "google");
    }

    #[cfg(feature = "ollama")]
    #[test]
    fn ollama_static_catalog_is_local() {
        let catalog = ollama_catalog("ollama");
        let llama = catalog
            .models()
            .iter()
            .find(|model| model.model_id == "llama3.3")
            .expect("llama3.3 present");

        assert!(llama.policy.local);
        assert!(!llama.policy.requires_credentials);
    }

    #[cfg(feature = "all-providers")]
    #[test]
    fn all_provider_catalog_builders_do_not_panic() {
        assert!(!openai_catalog("openai").models().is_empty());
        assert!(!codex_catalog("codex").models().is_empty());
        assert!(!anthropic_catalog("anthropic").models().is_empty());
        assert!(!google_catalog("google").models().is_empty());
        assert!(!grok_catalog("grok").models().is_empty());
        assert!(!groq_catalog("groq").models().is_empty());
        assert!(!mistral_catalog("mistral").models().is_empty());
        assert!(!ollama_catalog("ollama").models().is_empty());
        assert!(lmstudio_catalog("lmstudio").models().is_empty());
        assert!(!github_copilot_static_catalog("github-copilot")
            .models()
            .is_empty());
        assert!(empty_catalog("openrouter").models().is_empty());
    }
}
