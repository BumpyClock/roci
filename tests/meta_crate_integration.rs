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
fn default_auth_service_includes_enabled_backends() {
    let (_dir, store) = temp_store();
    let svc = roci::default_auth_service(store);
    let registry = roci::default_registry();
    // Dependency features may be unified by another workspace member (the CLI).
    let mut expected: Vec<_> = [
        ("github-copilot", "GitHub Copilot"),
        ("codex", "Codex"),
        ("anthropic", "Claude"),
    ]
    .into_iter()
    .filter_map(|(provider, backend)| registry.has_provider(provider).then_some(backend))
    .collect();
    let statuses = svc.all_statuses();
    let mut names: Vec<_> = statuses.iter().map(|(name, _, _)| *name).collect();
    expected.sort_unstable();
    names.sort_unstable();
    assert_eq!(names, expected);
}

// ---------------------------------------------------------------------------
// Feature flag behavior
// ---------------------------------------------------------------------------

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
