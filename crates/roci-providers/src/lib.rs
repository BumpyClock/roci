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
pub mod overflow;
pub mod provider;

#[cfg(any(feature = "openai", feature = "anthropic", feature = "google"))]
use std::sync::Arc;

#[cfg(any(feature = "openai", feature = "anthropic", feature = "google"))]
use overflow::OverflowClassifyingFactory;

/// Register all enabled built-in provider factories with the given registry.
///
/// Each factory is wrapped with [`overflow::OverflowClassifyingFactory`] so every
/// provider instance gains text-based overflow classification without
/// editing individual provider implementations.
pub fn register_default_providers(_registry: &mut roci_core::provider::ProviderRegistry) {
    #[cfg(feature = "openai")]
    {
        _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
            factories::OpenAiFactory,
        )));
        _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
            factories::CodexFactory,
        )));
    }

    #[cfg(feature = "anthropic")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::AnthropicFactory,
    )));

    #[cfg(feature = "google")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::GoogleFactory,
    )));

    #[cfg(feature = "grok")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::GrokFactory,
    )));

    #[cfg(feature = "cursor")]
    _registry.register(std::sync::Arc::new(auth::factory::CursorFactory));

    #[cfg(feature = "groq")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::GroqFactory,
    )));

    #[cfg(feature = "mistral")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::MistralFactory,
    )));

    #[cfg(feature = "ollama")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::OllamaFactory,
    )));

    #[cfg(feature = "lmstudio")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::LmStudioFactory,
    )));

    #[cfg(feature = "openai-compatible")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::OpenAiCompatibleFactory,
    )));

    #[cfg(feature = "github-copilot")]
    {
        _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
            factories::GitHubCopilotFactory,
        )));
    }

    #[cfg(feature = "anthropic-compatible")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::AnthropicCompatibleFactory,
    )));

    #[cfg(feature = "azure")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::AzureFactory,
    )));

    #[cfg(feature = "openrouter")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::OpenRouterFactory,
    )));

    #[cfg(feature = "together")]
    _registry.register(OverflowClassifyingFactory::wrap(Arc::new(
        factories::TogetherFactory,
    )));
}

/// Register all built-in auth backends with the given auth service.
pub fn register_default_auth_backends(_service: &mut roci_core::auth::AuthService) {
    #[cfg(feature = "github-copilot")]
    _service.register_backend(Arc::new(auth::GitHubCopilotBackend));
    #[cfg(feature = "openai")]
    _service.register_backend(Arc::new(auth::OpenAiCodexBackend));
    #[cfg(feature = "anthropic")]
    _service.register_backend(Arc::new(auth::ClaudeCodeBackend));
    #[cfg(feature = "grok")]
    _service.register_backend(Arc::new(auth::xai::XaiBackend));
    #[cfg(feature = "google")]
    _service.register_backend(Arc::new(auth::gemini::GeminiBackend));
    #[cfg(feature = "cursor")]
    _service.register_backend(std::sync::Arc::new(auth::cursor::CursorBackend));
}
