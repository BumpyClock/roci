//! Integration tests for the `roci` meta-crate.
//!
//! Verifies that `default_registry()` and `default_auth_service()` produce
//! correctly-wired instances, and that re-exports are accessible through the
//! `roci` namespace.

use std::sync::Arc;

use tempfile::TempDir;

use roci::auth::{FileTokenStore, TokenStoreConfig};

// ---------------------------------------------------------------------------
// Re-export accessibility
// ---------------------------------------------------------------------------

#[test]
fn model_provider_trait_is_accessible_via_roci() {
    fn _assert_trait_accessible<T: roci::provider::ModelProvider>() {}
    // Compile-time check: the trait is importable and usable as a bound.
}

#[test]
fn auth_service_is_accessible_via_roci() {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
        dir.path().to_path_buf(),
    )));
    let _svc: roci::auth::AuthService = roci::auth::AuthService::new(store);
}

#[test]
fn provider_registry_is_accessible_via_roci() {
    let _reg: roci::provider::ProviderRegistry = roci::provider::ProviderRegistry::new();
}

#[test]
fn config_is_accessible_via_roci() {
    let _cfg: roci::config::RociConfig = roci::config::RociConfig::new().with_token_store(None);
}

#[test]
fn error_type_is_accessible_via_roci() {
    let _err: roci::error::RociError = roci::error::RociError::ModelNotFound("test".to_string());
}

#[test]
fn prelude_types_are_accessible_via_roci() {
    let _: fn() -> roci::prelude::RociConfig = roci::prelude::RociConfig::new;
}

// ---------------------------------------------------------------------------
// default_registry()
// ---------------------------------------------------------------------------

#[test]
fn default_registry_contains_openai() {
    let registry = roci::default_registry();
    assert!(
        registry.has_provider("openai"),
        "default registry should include openai"
    );
}

#[test]
fn default_registry_contains_anthropic() {
    let registry = roci::default_registry();
    assert!(
        registry.has_provider("anthropic"),
        "default registry should include anthropic"
    );
}

#[test]
fn default_registry_contains_google() {
    let registry = roci::default_registry();
    assert!(
        registry.has_provider("google"),
        "default registry should include google"
    );
}

#[test]
fn default_registry_contains_codex() {
    let registry = roci::default_registry();
    assert!(
        registry.has_provider("codex"),
        "default registry should include codex"
    );
}

#[test]
fn default_registry_has_multiple_providers() {
    let registry = roci::default_registry();
    let keys = registry.provider_keys();
    assert!(
        keys.len() >= 4,
        "expected at least 4 default providers, got {}",
        keys.len()
    );
}

// ---------------------------------------------------------------------------
// default_auth_service()
// ---------------------------------------------------------------------------

fn temp_store() -> (TempDir, Arc<dyn roci::auth::TokenStore>) {
    let dir = TempDir::new().unwrap();
    let store: Arc<dyn roci::auth::TokenStore> = Arc::new(FileTokenStore::new(
        TokenStoreConfig::new(dir.path().to_path_buf()),
    ));
    (dir, store)
}

#[test]
fn default_auth_service_has_three_backends() {
    let (_dir, store) = temp_store();
    let svc = roci::default_auth_service(store);

    let statuses = svc.all_statuses();
    assert_eq!(
        statuses.len(),
        3,
        "expected 3 auth backends, got {}",
        statuses.len()
    );
}

#[test]
fn default_auth_service_includes_copilot_backend() {
    let (_dir, store) = temp_store();
    let svc = roci::default_auth_service(store);

    let names: Vec<&str> = svc
        .all_statuses()
        .iter()
        .map(|(name, _, _)| *name)
        .collect();
    assert!(
        names.contains(&"GitHub Copilot"),
        "expected GitHub Copilot backend, got {names:?}"
    );
}

#[test]
fn default_auth_service_includes_codex_backend() {
    let (_dir, store) = temp_store();
    let svc = roci::default_auth_service(store);

    let names: Vec<&str> = svc
        .all_statuses()
        .iter()
        .map(|(name, _, _)| *name)
        .collect();
    assert!(
        names.contains(&"Codex"),
        "expected Codex backend, got {names:?}"
    );
}

#[test]
fn default_auth_service_includes_claude_backend() {
    let (_dir, store) = temp_store();
    let svc = roci::default_auth_service(store);

    let names: Vec<&str> = svc
        .all_statuses()
        .iter()
        .map(|(name, _, _)| *name)
        .collect();
    assert!(
        names.contains(&"Claude"),
        "expected Claude backend, got {names:?}"
    );
}

// ---------------------------------------------------------------------------
// Feature flag behavior
// ---------------------------------------------------------------------------

#[cfg(feature = "openai")]
#[test]
fn openai_feature_enables_openai_in_registry() {
    let registry = roci::default_registry();
    assert!(
        registry.has_provider("openai"),
        "openai feature enabled but not in registry"
    );
}

#[cfg(feature = "anthropic")]
#[test]
fn anthropic_feature_enables_anthropic_in_registry() {
    let registry = roci::default_registry();
    assert!(
        registry.has_provider("anthropic"),
        "anthropic feature enabled but not in registry"
    );
}

#[cfg(feature = "google")]
#[test]
fn google_feature_enables_google_in_registry() {
    let registry = roci::default_registry();
    assert!(
        registry.has_provider("google"),
        "google feature enabled but not in registry"
    );
}

#[cfg(feature = "grok")]
#[test]
fn grok_feature_enables_grok_in_registry() {
    let registry = roci::default_registry();
    assert!(registry.has_provider("grok"));
}

#[cfg(feature = "groq")]
#[test]
fn groq_feature_enables_groq_in_registry() {
    let registry = roci::default_registry();
    assert!(registry.has_provider("groq"));
}
