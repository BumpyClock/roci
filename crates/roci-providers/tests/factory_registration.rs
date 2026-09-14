//! Integration tests for roci-providers factory and auth backend registration.
//!
//! Verifies that `register_default_providers()` and
//! `register_default_auth_backends()` populate the registry/service with
//! expected keys. No real API calls are made.

use std::sync::Arc;

use tempfile::TempDir;

use roci_core::auth::{AuthService, FileTokenStore, ProviderAuthManager, TokenStoreConfig};
use roci_core::config::RociConfig;
#[cfg(any(feature = "github-copilot", feature = "ollama"))]
use roci_core::models::ModelListOptions;
use roci_core::provider::ProviderRegistry;

// ---------------------------------------------------------------------------
// register_default_providers
// ---------------------------------------------------------------------------

#[test]
fn register_default_providers_matches_enabled_features() {
    let expected: &[&str] = &[
        #[cfg(feature = "openai")]
        "openai",
        #[cfg(feature = "openai")]
        "codex",
        #[cfg(feature = "anthropic")]
        "anthropic",
        #[cfg(feature = "google")]
        "google",
        #[cfg(feature = "grok")]
        "grok",
        #[cfg(feature = "groq")]
        "groq",
        #[cfg(feature = "mistral")]
        "mistral",
        #[cfg(feature = "ollama")]
        "ollama",
        #[cfg(feature = "lmstudio")]
        "lmstudio",
        #[cfg(feature = "openai-compatible")]
        "openai-compatible",
        #[cfg(feature = "github-copilot")]
        "github-copilot",
        #[cfg(feature = "anthropic-compatible")]
        "anthropic-compatible",
        #[cfg(feature = "azure")]
        "azure",
        #[cfg(feature = "openrouter")]
        "openrouter",
        #[cfg(feature = "together")]
        "together",
    ];
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);

    let mut actual = registry.provider_keys();
    actual.sort_unstable();
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    assert_eq!(actual, expected);
}

#[cfg(feature = "github-copilot")]
#[tokio::test]
async fn explicit_github_copilot_catalog_falls_back_without_credentials() {
    let config = RociConfig::new()
        .with_token_store(None)
        .with_provider_credential_store(None);
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);
    let options = ModelListOptions {
        provider_key: Some("github-copilot".to_string()),
        ..ModelListOptions::default()
    };

    let catalog = registry.list_models(&config, &options).await.unwrap();

    assert!(!catalog.models().is_empty());
    assert!(catalog
        .models()
        .iter()
        .all(|model| model.provider_key == "github-copilot"));
}

#[cfg(feature = "ollama")]
#[tokio::test]
async fn register_default_providers_all_catalog_keeps_local_ollama_without_credentials() {
    let config = RociConfig::new()
        .with_token_store(None)
        .with_provider_credential_store(None);
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);
    assert_eq!(registry.requires_credentials("ollama"), Some(false));

    let catalog = registry
        .list_models(&config, &ModelListOptions::default())
        .await
        .unwrap();

    assert!(catalog
        .models()
        .iter()
        .any(|model| model.provider_key == "ollama" && model.model_id == "llama3.3"));
}

// ---------------------------------------------------------------------------
// register_default_auth_backends
// ---------------------------------------------------------------------------

fn temp_auth_service() -> (TempDir, AuthService) {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
        dir.path().to_path_buf(),
    )));
    (dir, AuthService::new(store))
}

#[test]
fn default_auth_backends_match_enabled_launch_factories() {
    let (_dir, mut auth) = temp_auth_service();
    roci_providers::register_default_auth_backends(&mut auth);
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);

    let config = RociConfig::new()
        .with_token_store(None)
        .with_provider_credential_store(None);
    let result = ProviderAuthManager::new(auth, registry, config);

    assert!(result.is_ok());
}

#[cfg(feature = "github-copilot")]
#[test]
fn register_default_auth_backends_includes_github_copilot() {
    let (_dir, mut svc) = temp_auth_service();
    roci_providers::register_default_auth_backends(&mut svc);

    let statuses = svc.all_statuses();
    let names: Vec<&str> = statuses.iter().map(|(name, _, _)| *name).collect();
    assert!(
        names.contains(&"GitHub Copilot"),
        "expected GitHub Copilot in {names:?}"
    );
}

#[cfg(feature = "openai")]
#[test]
fn register_default_auth_backends_includes_codex() {
    let (_dir, mut svc) = temp_auth_service();
    roci_providers::register_default_auth_backends(&mut svc);

    let statuses = svc.all_statuses();
    let names: Vec<&str> = statuses.iter().map(|(name, _, _)| *name).collect();
    assert!(names.contains(&"Codex"), "expected Codex in {names:?}");
}

#[cfg(feature = "anthropic")]
#[test]
fn register_default_auth_backends_includes_claude() {
    let (_dir, mut svc) = temp_auth_service();
    roci_providers::register_default_auth_backends(&mut svc);

    let statuses = svc.all_statuses();
    let names: Vec<&str> = statuses.iter().map(|(name, _, _)| *name).collect();
    assert!(names.contains(&"Claude"), "expected Claude in {names:?}");
}

#[cfg(feature = "github-copilot")]
#[test]
fn copilot_alias_resolves_after_registration() {
    let (_dir, mut svc) = temp_auth_service();
    roci_providers::register_default_auth_backends(&mut svc);

    let result = svc.get_status("copilot");
    assert!(result.is_ok(), "copilot alias should resolve to backend");
}

#[cfg(feature = "anthropic")]
#[test]
fn claude_alias_resolves_after_registration() {
    let (_dir, mut svc) = temp_auth_service();
    roci_providers::register_default_auth_backends(&mut svc);

    let result = svc.get_status("claude");
    assert!(result.is_ok(), "claude alias should resolve to backend");
}
