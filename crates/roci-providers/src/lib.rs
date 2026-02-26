//! Roci Providers -- Built-in provider transports and OAuth flows.
//!
//! This crate contains all concrete provider implementations (OpenAI,
//! Anthropic, Google, etc.) and OAuth flow implementations (GitHub Copilot,
//! OpenAI Codex, Claude Code).
//!
//! Provider-agnostic abstractions live in `roci-core`.

pub mod auth;
pub mod factories;
pub mod models;
pub mod provider;

use std::sync::Arc;

/// Register all enabled built-in provider factories with the given registry.
#[allow(unused_variables)]
pub fn register_default_providers(registry: &mut roci_core::provider::ProviderRegistry) {
    #[cfg(feature = "openai")]
    {
        registry.register(Arc::new(factories::OpenAiFactory));
        registry.register(Arc::new(factories::CodexFactory));
    }

    #[cfg(feature = "anthropic")]
    registry.register(Arc::new(factories::AnthropicFactory));

    #[cfg(feature = "google")]
    registry.register(Arc::new(factories::GoogleFactory));

    #[cfg(feature = "grok")]
    registry.register(Arc::new(factories::GrokFactory));

    #[cfg(feature = "groq")]
    registry.register(Arc::new(factories::GroqFactory));

    #[cfg(feature = "mistral")]
    registry.register(Arc::new(factories::MistralFactory));

    #[cfg(feature = "ollama")]
    registry.register(Arc::new(factories::OllamaFactory));

    #[cfg(feature = "lmstudio")]
    registry.register(Arc::new(factories::LmStudioFactory));

    #[cfg(feature = "openai-compatible")]
    {
        registry.register(Arc::new(factories::OpenAiCompatibleFactory));
        registry.register(Arc::new(factories::GitHubCopilotFactory));
    }

    #[cfg(feature = "anthropic-compatible")]
    registry.register(Arc::new(factories::AnthropicCompatibleFactory));

    #[cfg(feature = "azure")]
    registry.register(Arc::new(factories::AzureFactory));

    #[cfg(feature = "openrouter")]
    registry.register(Arc::new(factories::OpenRouterFactory));

    #[cfg(feature = "together")]
    registry.register(Arc::new(factories::TogetherFactory));

    #[cfg(feature = "replicate")]
    registry.register(Arc::new(factories::ReplicateFactory));
}

/// Register all built-in auth backends with the given auth service.
pub fn register_default_auth_backends(service: &mut roci_core::auth::AuthService) {
    service.register_backend(Arc::new(auth::GitHubCopilotBackend));
    service.register_backend(Arc::new(auth::OpenAiCodexBackend));
    service.register_backend(Arc::new(auth::ClaudeCodeBackend));
}
