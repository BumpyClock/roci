//! Configuration system (layered: code > env > credential file).

pub mod auth;

pub use auth::{AuthManager, AuthValue};

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, OnceLock, RwLock};

use crate::auth::store::TokenStore;
use crate::models::ProviderKey;

/// Global default config (lazy-initialized from env).
static DEFAULT_CONFIG: OnceLock<RociConfig> = OnceLock::new();

/// Layered configuration for Roci.
///
/// Resolution order for API keys:
/// 1. Explicit keys (from env vars or `set_api_key`)
/// 2. OAuth tokens from `TokenStore` (from `roci auth login`)
#[derive(Clone)]
pub struct RociConfig {
    api_keys: Arc<RwLock<HashMap<String, String>>>,
    base_urls: Arc<RwLock<HashMap<String, String>>>,
    account_ids: Arc<RwLock<HashMap<String, String>>>,
    token_store: Option<Arc<dyn TokenStore>>,
}

impl fmt::Debug for RociConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RociConfig")
            .field("api_keys", &self.api_keys)
            .field("base_urls", &self.base_urls)
            .field("account_ids", &self.account_ids)
            .field("token_store", &self.token_store.as_ref().map(|_| ".."))
            .finish()
    }
}

impl Default for RociConfig {
    fn default() -> Self {
        Self::new()
    }
}

fn get_from_map(
    map: &RwLock<HashMap<String, String>>,
    provider: &str,
    provider_key: Option<ProviderKey>,
) -> Option<String> {
    let guard = map.read().ok()?;
    if let Some(value) = guard.get(provider) {
        return Some(value.clone());
    }
    if let Some(key) = provider_key {
        for lookup in key.lookup_keys() {
            if let Some(value) = guard.get(*lookup) {
                return Some(value.clone());
            }
        }
    }
    None
}

impl RociConfig {
    /// Create empty config with default file-backed token store.
    pub fn new() -> Self {
        Self {
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            base_urls: Arc::new(RwLock::new(HashMap::new())),
            account_ids: Arc::new(RwLock::new(HashMap::new())),
            token_store: Some(Arc::new(crate::auth::store::FileTokenStore::new_default())),
        }
    }

    /// Create config with a specific token store (or `None` to disable fallback).
    pub fn with_token_store(mut self, store: Option<Arc<dyn TokenStore>>) -> Self {
        self.token_store = store;
        self
    }

    /// Access the underlying token store (if configured).
    pub fn token_store(&self) -> Option<&Arc<dyn TokenStore>> {
        self.token_store.as_ref()
    }

    /// Load from environment variables (OPENAI_API_KEY, ANTHROPIC_API_KEY, etc.).
    pub fn from_env() -> Self {
        let _ = dotenvy::dotenv(); // load .env if present, ignore error
        let config = Self::new();

        let env_mappings = [
            ("OPENAI_API_KEY", ProviderKey::OpenAi),
            ("OPENAI_CODEX_TOKEN", ProviderKey::Codex),
            ("CHATGPT_TOKEN", ProviderKey::Codex),
            ("OPENAI_COMPAT_API_KEY", ProviderKey::OpenAiCompatible),
            ("ANTHROPIC_API_KEY", ProviderKey::Anthropic),
            ("GOOGLE_API_KEY", ProviderKey::Google),
            ("GEMINI_API_KEY", ProviderKey::Google),
            ("XAI_API_KEY", ProviderKey::Grok),
            ("GROK_API_KEY", ProviderKey::Grok),
            ("GROQ_API_KEY", ProviderKey::Groq),
            ("MISTRAL_API_KEY", ProviderKey::Mistral),
        ];

        for (env_var, provider) in env_mappings {
            if let Ok(key) = std::env::var(env_var) {
                config.set_api_key(provider.as_str(), key);
            }
        }
        for (env_var, provider) in [
            ("TOGETHER_API_KEY", "together"),
            ("OPENROUTER_API_KEY", "openrouter"),
            ("REPLICATE_API_TOKEN", "replicate"),
        ] {
            if let Ok(key) = std::env::var(env_var) {
                config.set_api_key(provider, key);
            }
        }

        // Base URL overrides
        let url_mappings = [
            ("OPENAI_BASE_URL", ProviderKey::OpenAi),
            ("OPENAI_CODEX_BASE_URL", ProviderKey::Codex),
            ("CHATGPT_BASE_URL", ProviderKey::Codex),
            ("OPENAI_COMPAT_BASE_URL", ProviderKey::OpenAiCompatible),
            ("ANTHROPIC_BASE_URL", ProviderKey::Anthropic),
            ("OLLAMA_BASE_URL", ProviderKey::Ollama),
            ("LMSTUDIO_BASE_URL", ProviderKey::LmStudio),
        ];

        for (env_var, provider) in url_mappings {
            if let Ok(url) = std::env::var(env_var) {
                config.set_base_url(provider.as_str(), url);
            }
        }

        config
    }

    /// Get (or create) the global default config.
    pub fn global() -> &'static RociConfig {
        DEFAULT_CONFIG.get_or_init(Self::from_env)
    }

    pub fn set_api_key(&self, provider: &str, key: String) {
        self.api_keys
            .write()
            .unwrap()
            .insert(provider.to_string(), key);
    }

    /// Resolve an API key for a provider.
    ///
    /// Checks explicit keys first, then falls back to the token store
    /// for OAuth tokens saved via `roci auth login`.
    pub fn get_api_key(&self, provider: &str) -> Option<String> {
        let provider_key = ProviderKey::parse(provider);
        if let Some(key) = get_from_map(&self.api_keys, provider, provider_key) {
            return Some(key);
        }

        if let Some(ref store) = self.token_store {
            if let Some(store_key) = provider_key.and_then(ProviderKey::token_store_key) {
                if let Ok(Some(token)) = store.load(store_key, "default") {
                    let is_valid = token
                        .expires_at
                        .map(|exp| exp > chrono::Utc::now())
                        .unwrap_or(true);
                    if is_valid {
                        return Some(token.access_token);
                    }
                }
            }
        }

        None
    }

    pub fn get_api_key_for(&self, provider: ProviderKey) -> Option<String> {
        self.get_api_key(provider.as_str())
    }

    pub fn set_base_url(&self, provider: &str, url: String) {
        self.base_urls
            .write()
            .unwrap()
            .insert(provider.to_string(), url);
    }

    pub fn get_base_url(&self, provider: &str) -> Option<String> {
        get_from_map(&self.base_urls, provider, ProviderKey::parse(provider))
    }

    pub fn get_base_url_for(&self, provider: ProviderKey) -> Option<String> {
        self.get_base_url(provider.as_str())
    }

    pub fn set_account_id(&self, provider: &str, account_id: String) {
        self.account_ids
            .write()
            .unwrap()
            .insert(provider.to_string(), account_id);
    }

    pub fn get_account_id(&self, provider: &str) -> Option<String> {
        get_from_map(&self.account_ids, provider, ProviderKey::parse(provider))
    }

    pub fn get_account_id_for(&self, provider: ProviderKey) -> Option<String> {
        self.get_account_id(provider.as_str())
    }

    /// Check if a provider has credentials configured (explicit key or token store).
    pub fn has_credentials(&self, provider: &str) -> bool {
        self.get_api_key(provider).is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::store::{FileTokenStore, TokenStoreConfig};
    use crate::auth::token::Token;
    use chrono::{Duration, Utc};
    use tempfile::TempDir;

    fn config_with_temp_store(dir: &std::path::Path) -> RociConfig {
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.to_path_buf()));
        RociConfig::new().with_token_store(Some(Arc::new(store)))
    }

    fn make_token(access_token: &str, expires_at: Option<chrono::DateTime<Utc>>) -> Token {
        Token {
            access_token: access_token.to_string(),
            refresh_token: None,
            id_token: None,
            expires_at,
            last_refresh: None,
            scopes: None,
            account_id: None,
        }
    }

    #[test]
    fn openai_does_not_fall_back_to_codex_token_store() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let token = make_token("oauth-access-token", None);
        store.save("openai-codex", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert_eq!(config.get_api_key("openai"), None);
    }

    #[test]
    fn codex_falls_back_to_token_store() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let token = make_token("oauth-access-token", None);
        store.save("openai-codex", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert_eq!(
            config.get_api_key("codex"),
            Some("oauth-access-token".to_string()),
        );
    }

    #[test]
    fn explicit_key_takes_precedence_over_token_store() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let token = make_token("oauth-token", None);
        store.save("openai-codex", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());
        config.set_api_key("openai", "env-api-key".to_string());

        assert_eq!(
            config.get_api_key("openai"),
            Some("env-api-key".to_string()),
        );
    }

    #[test]
    fn expired_codex_token_in_store_returns_none() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let expired = Utc::now() - Duration::hours(1);
        let token = make_token("stale-token", Some(expired));
        store.save("openai-codex", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert_eq!(config.get_api_key("codex"), None);
    }

    #[test]
    fn non_expired_codex_token_in_store_is_returned() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let future = Utc::now() + Duration::hours(1);
        let token = make_token("fresh-token", Some(future));
        store.save("openai-codex", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert_eq!(config.get_api_key("codex"), Some("fresh-token".to_string()),);
    }

    #[test]
    fn has_credentials_checks_codex_token_store() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let token = make_token("token-for-creds-check", None);
        store.save("openai-codex", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert!(config.has_credentials("codex"));
    }

    #[test]
    fn unmapped_provider_returns_none_from_token_store() {
        let dir = TempDir::new().unwrap();
        let config = config_with_temp_store(dir.path());

        assert_eq!(config.get_api_key("some-unknown-provider"), None);
    }

    #[test]
    fn config_without_token_store_returns_none_for_missing_key() {
        let config = RociConfig::new().with_token_store(None);

        assert_eq!(config.get_api_key("openai"), None);
    }

    #[test]
    fn anthropic_falls_back_to_claude_code_token() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let token = make_token("claude-oauth-token", None);
        store.save("claude-code", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert_eq!(
            config.get_api_key("anthropic"),
            Some("claude-oauth-token".to_string()),
        );
    }

    #[test]
    fn github_copilot_falls_back_to_token_store() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let token = make_token("copilot-token", None);
        store.save("github-copilot", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert_eq!(
            config.get_api_key("github-copilot"),
            Some("copilot-token".to_string()),
        );
    }
}
