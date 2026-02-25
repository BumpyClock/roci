//! Tests for configuration system.

use std::sync::{Mutex, OnceLock};

use roci::config::{AuthManager, AuthValue, RociConfig};
use roci::error::RociError;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

const CONFIG_ENV_VARS: [&str; 17] = [
    "OPENAI_API_KEY",
    "OPENAI_COMPAT_API_KEY",
    "ANTHROPIC_API_KEY",
    "GOOGLE_API_KEY",
    "GEMINI_API_KEY",
    "XAI_API_KEY",
    "GROK_API_KEY",
    "GROQ_API_KEY",
    "MISTRAL_API_KEY",
    "TOGETHER_API_KEY",
    "OPENROUTER_API_KEY",
    "REPLICATE_API_TOKEN",
    "OPENAI_BASE_URL",
    "OPENAI_COMPAT_BASE_URL",
    "ANTHROPIC_BASE_URL",
    "OLLAMA_BASE_URL",
    "LMSTUDIO_BASE_URL",
];

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn capture(keys: &[&str]) -> Self {
        let saved = keys
            .iter()
            .map(|key| ((*key).to_string(), std::env::var(key).ok()))
            .collect();
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in &self.saved {
            match value {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }
}

fn env_lock_guard() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn config_set_get_api_key() {
    let config = RociConfig::new();
    config.set_api_key("openai", "sk-test-123".to_string());
    assert_eq!(
        config.get_api_key("openai"),
        Some("sk-test-123".to_string())
    );
    assert_eq!(config.get_api_key("anthropic"), None);
}

#[test]
fn config_set_get_base_url() {
    let config = RociConfig::new();
    config.set_base_url("openai", "http://localhost:8080".to_string());
    assert_eq!(
        config.get_base_url("openai"),
        Some("http://localhost:8080".to_string())
    );
}

#[test]
fn config_has_credentials() {
    let config = RociConfig::new();
    assert!(!config.has_credentials("openai"));
    config.set_api_key("openai", "sk-test".to_string());
    assert!(config.has_credentials("openai"));
}

#[test]
fn config_from_env_maps_api_keys_and_base_urls() {
    let _env_lock = env_lock_guard();
    let _env_guard = EnvGuard::capture(&CONFIG_ENV_VARS);
    for key in CONFIG_ENV_VARS {
        std::env::remove_var(key);
    }

    std::env::set_var("OPENAI_API_KEY", "test-openai-key");
    std::env::set_var("ANTHROPIC_API_KEY", "test-anthropic-key");
    std::env::set_var("OPENAI_BASE_URL", "http://localhost:9999/v1");

    let config = RociConfig::from_env();

    assert_eq!(
        config.get_api_key("openai"),
        Some("test-openai-key".to_string())
    );
    assert_eq!(
        config.get_api_key("anthropic"),
        Some("test-anthropic-key".to_string())
    );
    assert_eq!(
        config.get_base_url("openai"),
        Some("http://localhost:9999/v1".to_string())
    );
}

#[test]
fn config_from_env_applies_alias_precedence_for_shared_providers() {
    let _env_lock = env_lock_guard();
    let _env_guard = EnvGuard::capture(&CONFIG_ENV_VARS);
    for key in CONFIG_ENV_VARS {
        std::env::remove_var(key);
    }

    std::env::set_var("GOOGLE_API_KEY", "google-key");
    std::env::set_var("GEMINI_API_KEY", "gemini-key");
    std::env::set_var("XAI_API_KEY", "xai-key");
    std::env::set_var("GROK_API_KEY", "grok-key");

    let config = RociConfig::from_env();

    assert_eq!(config.get_api_key("google"), Some("gemini-key".to_string()));
    assert_eq!(config.get_api_key("grok"), Some("grok-key".to_string()));
}

#[test]
fn config_from_env_reads_openai_compatible_mappings() {
    let _env_lock = env_lock_guard();
    let _env_guard = EnvGuard::capture(&CONFIG_ENV_VARS);
    for key in CONFIG_ENV_VARS {
        std::env::remove_var(key);
    }

    std::env::set_var("OPENAI_COMPAT_API_KEY", "compat-key");
    std::env::set_var("OPENAI_COMPAT_BASE_URL", "http://compat.local/v1");

    let config = RociConfig::from_env();

    assert_eq!(
        config.get_api_key("openai-compatible"),
        Some("compat-key".to_string())
    );
    assert_eq!(
        config.get_base_url("openai-compatible"),
        Some("http://compat.local/v1".to_string())
    );
}

#[test]
fn auth_value_resolve_returns_plain_values_for_api_key_and_bearer_token() {
    let api_key = AuthValue::ApiKey("sk-test".to_string());
    let bearer = AuthValue::BearerToken("token-123".to_string());

    assert_eq!(api_key.resolve().unwrap(), "sk-test");
    assert_eq!(bearer.resolve().unwrap(), "token-123");
}

#[test]
fn auth_value_resolve_reads_environment_variable() {
    let _env_lock = env_lock_guard();
    let key = "ROCI_TEST_AUTH_ENV";
    let _env_guard = EnvGuard::capture(&[key]);

    std::env::set_var(key, "env-secret");
    let value = AuthValue::EnvVar(key.to_string());

    assert_eq!(value.resolve().unwrap(), "env-secret");
}

#[test]
fn auth_value_resolve_returns_authentication_error_for_missing_env_var() {
    let _env_lock = env_lock_guard();
    let key = "ROCI_TEST_AUTH_ENV_MISSING";
    let _env_guard = EnvGuard::capture(&[key]);

    std::env::remove_var(key);
    let err = AuthValue::EnvVar(key.to_string()).resolve().unwrap_err();

    match err {
        RociError::Authentication(message) => assert!(message.contains(key)),
        other => panic!("expected authentication error, got {other:?}"),
    }
}

#[test]
fn auth_manager_resolve_returns_authentication_error_for_missing_provider() {
    let manager = AuthManager::new();
    let err = manager.resolve("missing-provider").unwrap_err();

    match err {
        RociError::Authentication(message) => assert!(message.contains("missing-provider")),
        other => panic!("expected authentication error, got {other:?}"),
    }
}
