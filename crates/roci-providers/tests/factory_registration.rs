//! Integration tests for roci-providers factory and auth backend registration.
//!
//! Verifies that `register_default_providers()` and
//! `register_default_auth_backends()` populate the registry/service with
//! expected keys. No real API calls are made.

use std::sync::Arc;

use tempfile::TempDir;

use roci_core::auth::{AuthService, FileTokenStore, ProviderAuthManager, TokenStoreConfig};
use roci_core::config::RociConfig;
#[cfg(feature = "ollama")]
use roci_core::models::ModelListOptions;
use roci_core::provider::ProviderRegistry;

// ---------------------------------------------------------------------------
// register_default_providers
// ---------------------------------------------------------------------------

#[test]
fn register_default_providers_registers_openai_key() {
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);

    assert!(
        registry.has_provider("openai"),
        "expected openai to be registered"
    );
}

#[test]
fn register_default_providers_registers_anthropic_key() {
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);

    assert!(
        registry.has_provider("anthropic"),
        "expected anthropic to be registered"
    );
}

#[test]
fn register_default_providers_registers_google_key() {
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);

    assert!(
        registry.has_provider("google"),
        "expected google to be registered"
    );
}

#[test]
fn register_default_providers_registers_codex_key() {
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);

    assert!(
        registry.has_provider("codex"),
        "expected codex to be registered"
    );
}

#[cfg(feature = "github-copilot")]
#[test]
fn register_default_providers_registers_github_copilot_when_feature_enabled() {
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);

    assert!(
        registry.has_provider("github-copilot"),
        "expected github-copilot to be registered"
    );
}

#[test]
fn register_default_providers_populates_multiple_keys() {
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);

    let keys = registry.provider_keys();
    assert!(
        keys.len() >= 4,
        "expected at least 4 keys, got {}",
        keys.len()
    );
}

#[cfg(feature = "grok")]
#[test]
fn register_default_providers_registers_grok_when_feature_enabled() {
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);
    assert!(registry.has_provider("grok"));
}

#[cfg(feature = "groq")]
#[test]
fn register_default_providers_registers_groq_when_feature_enabled() {
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);
    assert!(registry.has_provider("groq"));
}

#[cfg(feature = "ollama")]
#[test]
fn register_default_providers_registers_ollama_when_feature_enabled() {
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);
    assert!(registry.has_provider("ollama"));
}

#[cfg(feature = "ollama")]
#[tokio::test]
async fn register_default_providers_all_catalog_keeps_local_ollama_without_credentials() {
    let config = RociConfig::new().with_token_store(None);
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

    let result = ProviderAuthManager::new(auth, registry, RociConfig::new());

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
#[tokio::test]
async fn copilot_alias_resolves_after_registration() {
    let (_dir, mut svc) = temp_auth_service();
    roci_providers::register_default_auth_backends(&mut svc);

    let result = svc.get_status("copilot");
    assert!(result.is_ok(), "copilot alias should resolve to backend");
}

#[cfg(feature = "anthropic")]
#[tokio::test]
async fn claude_alias_resolves_after_registration() {
    let (_dir, mut svc) = temp_auth_service();
    roci_providers::register_default_auth_backends(&mut svc);

    let result = svc.get_status("claude");
    assert!(result.is_ok(), "claude alias should resolve to backend");
}
