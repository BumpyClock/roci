//! Model definitions and selection.

pub mod capabilities;
pub mod selector;

#[cfg(feature = "anthropic")]
pub mod anthropic;
#[cfg(feature = "google")]
pub mod google;
#[cfg(feature = "grok")]
pub mod grok;
#[cfg(feature = "groq")]
pub mod groq;
#[cfg(feature = "lmstudio")]
pub mod lmstudio;
#[cfg(feature = "mistral")]
pub mod mistral;
#[cfg(feature = "ollama")]
pub mod ollama;
#[cfg(feature = "openai")]
pub mod openai;
#[cfg(feature = "openai-compatible")]
pub mod openai_compatible;

pub use capabilities::ModelCapabilities;
pub use selector::ModelSelector;

use serde::{Deserialize, Serialize};
use std::fmt;

/// Top-level language model enum, dispatching to provider-specific variants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "provider", content = "model")]
pub enum LanguageModel {
    #[cfg(feature = "openai")]
    OpenAi(openai::OpenAiModel),
    #[cfg(feature = "openai")]
    OpenAiCodex(openai::OpenAiModel),
    #[cfg(feature = "anthropic")]
    Anthropic(anthropic::AnthropicModel),
    #[cfg(feature = "google")]
    Google(google::GoogleModel),
    #[cfg(feature = "grok")]
    Grok(grok::GrokModel),
    #[cfg(feature = "groq")]
    Groq(groq::GroqModel),
    #[cfg(feature = "mistral")]
    Mistral(mistral::MistralModel),
    #[cfg(feature = "ollama")]
    Ollama(ollama::OllamaModel),
    #[cfg(feature = "lmstudio")]
    LmStudio(lmstudio::LmStudioModel),
    #[cfg(feature = "openai-compatible")]
    OpenAiCompatible(openai_compatible::OpenAiCompatibleModel),
    #[cfg(feature = "openai-compatible")]
    GitHubCopilot(openai_compatible::OpenAiCompatibleModel),
    /// Custom model with explicit provider and model ID.
    Custom { provider: String, model_id: String },
}

impl LanguageModel {
    /// Get the model's API identifier string.
    pub fn model_id(&self) -> &str {
        match self {
            #[cfg(feature = "openai")]
            Self::OpenAi(m) => m.as_str(),
            #[cfg(feature = "openai")]
            Self::OpenAiCodex(m) => m.as_str(),
            #[cfg(feature = "anthropic")]
            Self::Anthropic(m) => m.as_str(),
            #[cfg(feature = "google")]
            Self::Google(m) => m.as_str(),
            #[cfg(feature = "grok")]
            Self::Grok(m) => m.as_str(),
            #[cfg(feature = "groq")]
            Self::Groq(m) => m.as_str(),
            #[cfg(feature = "mistral")]
            Self::Mistral(m) => m.as_str(),
            #[cfg(feature = "ollama")]
            Self::Ollama(m) => m.as_str(),
            #[cfg(feature = "lmstudio")]
            Self::LmStudio(m) => m.as_str(),
            #[cfg(feature = "openai-compatible")]
            Self::OpenAiCompatible(m) => m.model_id.as_str(),
            #[cfg(feature = "openai-compatible")]
            Self::GitHubCopilot(m) => m.model_id.as_str(),
            Self::Custom { model_id, .. } => model_id,
        }
    }

    /// Get the provider name.
    pub fn provider_name(&self) -> &str {
        match self {
            #[cfg(feature = "openai")]
            Self::OpenAi(_) => "openai",
            #[cfg(feature = "openai")]
            Self::OpenAiCodex(_) => "codex",
            #[cfg(feature = "anthropic")]
            Self::Anthropic(_) => "anthropic",
            #[cfg(feature = "google")]
            Self::Google(_) => "google",
            #[cfg(feature = "grok")]
            Self::Grok(_) => "grok",
            #[cfg(feature = "groq")]
            Self::Groq(_) => "groq",
            #[cfg(feature = "mistral")]
            Self::Mistral(_) => "mistral",
            #[cfg(feature = "ollama")]
            Self::Ollama(_) => "ollama",
            #[cfg(feature = "lmstudio")]
            Self::LmStudio(_) => "lmstudio",
            #[cfg(feature = "openai-compatible")]
            Self::OpenAiCompatible(_) => "openai-compatible",
            #[cfg(feature = "openai-compatible")]
            Self::GitHubCopilot(_) => "github-copilot",
            Self::Custom { provider, .. } => provider,
        }
    }

    /// Get capabilities for this model.
    pub fn capabilities(&self) -> ModelCapabilities {
        match self {
            #[cfg(feature = "openai")]
            Self::OpenAi(m) => m.capabilities(),
            #[cfg(feature = "openai")]
            Self::OpenAiCodex(m) => m.capabilities(),
            #[cfg(feature = "anthropic")]
            Self::Anthropic(m) => m.capabilities(),
            #[cfg(feature = "google")]
            Self::Google(m) => m.capabilities(),
            #[cfg(feature = "grok")]
            Self::Grok(m) => m.capabilities(),
            #[cfg(feature = "groq")]
            Self::Groq(m) => m.capabilities(),
            #[cfg(feature = "mistral")]
            Self::Mistral(m) => m.capabilities(),
            #[cfg(feature = "ollama")]
            Self::Ollama(m) => m.capabilities(),
            #[cfg(feature = "lmstudio")]
            Self::LmStudio(m) => m.capabilities(),
            #[cfg(feature = "openai-compatible")]
            Self::OpenAiCompatible(m) => m.capabilities(),
            #[cfg(feature = "openai-compatible")]
            Self::GitHubCopilot(m) => m.capabilities(),
            Self::Custom { .. } => ModelCapabilities::default(),
        }
    }
}

impl fmt::Display for LanguageModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider_name(), self.model_id())
    }
}
