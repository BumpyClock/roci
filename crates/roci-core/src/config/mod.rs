//! Configuration system (layered: code/env > protected credentials > OAuth).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

#[cfg(test)]
use crate::auth::credential::InMemoryProviderCredentialStore;
#[cfg(all(not(test), not(unix)))]
use crate::auth::credential::OsProviderCredentialStore;
use crate::auth::credential::{ProviderCredentialRecord, ProviderCredentialStore};
use crate::auth::store::TokenStore;
#[cfg(all(not(test), unix))]
use crate::auth::FileProviderCredentialStore;
use crate::models::ProviderKey;

/// Layered configuration for Roci.
///
/// Resolution order for API keys and endpoints:
/// 1. Explicit in-process/environment maps
/// 2. Protected [`ProviderCredentialStore`] records
/// 3. OAuth tokens from `TokenStore` aliases (API key only)
#[derive(Clone)]
pub struct RociConfig {
    api_keys: Arc<RwLock<HashMap<String, String>>>,
    base_urls: Arc<RwLock<HashMap<String, String>>>,
    account_ids: Arc<RwLock<HashMap<String, String>>>,
    token_store: Option<Arc<dyn TokenStore>>,
    provider_credential_store: Option<Arc<dyn ProviderCredentialStore>>,
}

impl fmt::Debug for RociConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let api_key_providers = map_keys(&self.api_keys);
        let base_url_providers = map_keys(&self.base_urls);
        let account_id_providers = map_keys(&self.account_ids);
        f.debug_struct("RociConfig")
            .field("api_key_providers", &api_key_providers)
            .field("base_url_providers", &base_url_providers)
            .field("account_id_providers", &account_id_providers)
            .field(
                "token_store",
                &self.token_store.as_ref().map(|_| "configured"),
            )
            .field(
                "provider_credential_store",
                &self
                    .provider_credential_store
                    .as_ref()
                    .map(|_| "configured"),
            )
            .finish()
    }
}

impl Default for RociConfig {
    fn default() -> Self {
        Self::new()
    }
}

fn map_keys(map: &RwLock<HashMap<String, String>>) -> Vec<String> {
    let mut keys: Vec<_> = map
        .read()
        .map(|guard| guard.keys().cloned().collect())
        .unwrap_or_default();
    keys.sort();
    keys
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

/// Production platform default credential-store backend.
///
/// Independent of the `cfg(test)` hermetic override so tests can assert the
/// shipping resolver without constructing real home-directory paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProductionProviderCredentialStoreKind {
    /// Locked `~/.roci/auth.json` map used on Unix production defaults.
    #[cfg(unix)]
    FileAuthJson,
    /// OS credential manager used on non-Unix production defaults.
    #[cfg(not(unix))]
    OsCredentialManager,
}

const fn production_provider_credential_store_kind() -> ProductionProviderCredentialStoreKind {
    #[cfg(unix)]
    {
        ProductionProviderCredentialStoreKind::FileAuthJson
    }
    #[cfg(not(unix))]
    {
        ProductionProviderCredentialStoreKind::OsCredentialManager
    }
}

#[cfg(not(test))]
fn default_provider_credential_store() -> Option<Arc<dyn ProviderCredentialStore>> {
    match production_provider_credential_store_kind() {
        #[cfg(unix)]
        ProductionProviderCredentialStoreKind::FileAuthJson => {
            FileProviderCredentialStore::new_default()
                .ok()
                .map(|store| Arc::new(store) as Arc<dyn ProviderCredentialStore>)
        }
        #[cfg(not(unix))]
        ProductionProviderCredentialStoreKind::OsCredentialManager => {
            Some(Arc::new(OsProviderCredentialStore::new()))
        }
    }
}

#[cfg(test)]
fn default_provider_credential_store() -> Option<Arc<dyn ProviderCredentialStore>> {
    Some(Arc::new(InMemoryProviderCredentialStore::default()))
}

impl RociConfig {
    /// Create empty config with default file-backed token store.
    pub fn new() -> Self {
        Self {
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            base_urls: Arc::new(RwLock::new(HashMap::new())),
            account_ids: Arc::new(RwLock::new(HashMap::new())),
            token_store: Some(Arc::new(crate::auth::store::FileTokenStore::new_default())),
            provider_credential_store: default_provider_credential_store(),
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

    /// Set the protected provider credential store, or disable stored fallback.
    ///
    /// Intended for host wiring and hermetic tests. Production defaults to the
    /// locked Unix `~/.roci/auth.json` file store on Unix and the OS credential
    /// manager elsewhere; tests default to an in-memory store.
    pub fn with_provider_credential_store(
        mut self,
        store: Option<Arc<dyn ProviderCredentialStore>>,
    ) -> Self {
        self.provider_credential_store = store;
        self
    }

    /// Access the protected provider credential store (if configured).
    pub fn provider_credential_store(&self) -> Option<&Arc<dyn ProviderCredentialStore>> {
        self.provider_credential_store.as_ref()
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
            ("AZURE_OPENAI_API_KEY", ProviderKey::Azure),
        ];

        for (env_var, provider) in env_mappings {
            if let Ok(key) = std::env::var(env_var) {
                config.set_api_key(provider.as_str(), key);
            }
        }
        for (env_var, provider) in [
            ("TOGETHER_API_KEY", "together"),
            ("OPENROUTER_API_KEY", "openrouter"),
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
            ("AZURE_OPENAI_ENDPOINT", ProviderKey::Azure),
        ];

        for (env_var, provider) in url_mappings {
            if let Ok(url) = std::env::var(env_var) {
                config.set_base_url(provider.as_str(), url);
            }
        }

        config
    }

    pub fn set_api_key(&self, provider: &str, key: String) {
        self.api_keys
            .write()
            .unwrap()
            .insert(provider.to_string(), key);
    }

    /// Resolve an API key for a provider.
    ///
    /// Checks explicit/environment keys, a protected stored API key, then the
    /// existing OAuth token-store alias. Protected-store access failures fail
    /// closed and never trigger plaintext fallback.
    pub fn get_api_key(&self, provider: &str) -> Option<String> {
        let provider_key = ProviderKey::parse(provider);
        if let Some(key) = get_from_map(&self.api_keys, provider, provider_key) {
            return Some(key);
        }

        if let Some(record) = self.stored_credential(provider) {
            return Some(record.api_key.expose_secret().to_string());
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
        let provider_key = ProviderKey::parse(provider);
        let explicit_base_url = get_from_map(&self.base_urls, provider, provider_key);
        if explicit_base_url.is_some() || self.has_explicit_api_key(provider) {
            return explicit_base_url;
        }
        self.stored_credential(provider)
            .and_then(|record| record.endpoint)
            .map(|endpoint| endpoint.as_str().to_string())
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

    /// True when an explicit/env API key is set (ignores all stored fallback).
    pub fn has_explicit_api_key(&self, provider: &str) -> bool {
        let provider_key = ProviderKey::parse(provider);
        get_from_map(&self.api_keys, provider, provider_key).is_some()
    }

    /// True when a protected Roci-owned provider credential record is present.
    pub fn has_stored_api_key(&self, provider: &str) -> bool {
        self.stored_credential(provider).is_some()
    }

    fn stored_credential(&self, provider: &str) -> Option<ProviderCredentialRecord> {
        let canonical = ProviderKey::parse(provider)
            .map(ProviderKey::as_str)
            .unwrap_or(provider);
        self.provider_credential_store
            .as_ref()?
            .load(canonical)
            .ok()
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::credential::{
        InMemoryProviderCredentialStore, ProviderApiKey, ProviderEndpoint,
    };
    use crate::auth::store::{FileTokenStore, TokenStoreConfig};
    use crate::auth::token::Token;
    use chrono::{Duration, Utc};
    use tempfile::TempDir;

    fn config_with_temp_store(dir: &std::path::Path) -> RociConfig {
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.to_path_buf()));
        RociConfig::new()
            .with_token_store(Some(Arc::new(store)))
            .with_provider_credential_store(Some(Arc::new(
                InMemoryProviderCredentialStore::default(),
            )))
    }

    fn config_with_credential_store(store: Arc<dyn ProviderCredentialStore>) -> RociConfig {
        RociConfig::new()
            .with_token_store(None)
            .with_provider_credential_store(Some(store))
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
        let config = RociConfig::new()
            .with_token_store(None)
            .with_provider_credential_store(None);

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

    #[test]
    fn explicit_maps_take_precedence_over_full_stored_record() {
        let store = Arc::new(InMemoryProviderCredentialStore::default());
        store
            .save(
                "anthropic",
                &ProviderCredentialRecord::new(
                    ProviderApiKey::new("stored-key"),
                    Some(ProviderEndpoint::new("https://stored.example")),
                ),
            )
            .unwrap();
        let config = config_with_credential_store(store);
        config.set_api_key("anthropic", "explicit-key".into());
        config.set_base_url("anthropic", "https://explicit.example".into());

        assert_eq!(config.get_api_key("anthropic"), Some("explicit-key".into()));
        assert_eq!(
            config.get_base_url("anthropic"),
            Some("https://explicit.example".into())
        );
        assert!(config.has_explicit_api_key("anthropic"));
        assert!(config.has_stored_api_key("anthropic"));
    }

    #[test]
    fn explicit_key_does_not_mix_with_stored_endpoint() {
        let store = Arc::new(InMemoryProviderCredentialStore::default());
        store
            .save(
                "anthropic",
                &ProviderCredentialRecord::new(
                    ProviderApiKey::new("stored-key"),
                    Some(ProviderEndpoint::new("https://stored.example")),
                ),
            )
            .unwrap();
        let config = config_with_credential_store(store);
        config.set_api_key("anthropic", "explicit-key".into());

        assert_eq!(config.get_api_key("anthropic"), Some("explicit-key".into()));
        assert_eq!(config.get_base_url("anthropic"), None);
    }

    #[test]
    fn stored_record_precedes_anthropic_oauth_token_as_full_object() {
        let dir = TempDir::new().unwrap();
        let token_store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().to_path_buf(),
        )));
        token_store
            .save("claude-code", "default", &make_token("oauth-token", None))
            .unwrap();
        let credential_store = Arc::new(InMemoryProviderCredentialStore::default());
        let expected = ProviderCredentialRecord::new(
            ProviderApiKey::new("stored-key"),
            Some(ProviderEndpoint::new("https://stored.example")),
        );
        credential_store.save("anthropic", &expected).unwrap();
        assert_eq!(credential_store.load("anthropic").unwrap(), Some(expected));
        let config = RociConfig::new()
            .with_token_store(Some(token_store))
            .with_provider_credential_store(Some(credential_store));

        assert_eq!(config.get_api_key("anthropic"), Some("stored-key".into()));
        assert_eq!(
            config.get_base_url("anthropic"),
            Some("https://stored.example".into())
        );
    }

    #[test]
    fn config_debug_redacts_keys_tokens_endpoints_and_account_material() {
        let credential_store = Arc::new(InMemoryProviderCredentialStore::default());
        credential_store
            .save(
                "anthropic",
                &ProviderCredentialRecord::new(
                    ProviderApiKey::new("stored-secret"),
                    Some(ProviderEndpoint::new("https://user:pass@stored.example")),
                ),
            )
            .unwrap();
        let config = config_with_credential_store(credential_store);
        config.set_api_key("anthropic", "explicit-secret".into());
        config.set_base_url("anthropic", "https://token@explicit.example".into());
        config.set_account_id("anthropic", "account-secret".into());

        let debug = format!("{config:?}");
        for secret in [
            "stored-secret",
            "user:pass",
            "explicit-secret",
            "token@explicit",
            "account-secret",
        ] {
            assert!(!debug.contains(secret), "debug leaked {secret}");
        }
    }

    #[test]
    fn production_default_credential_store_kind_matches_platform() {
        let kind = production_provider_credential_store_kind();
        #[cfg(unix)]
        assert_eq!(kind, ProductionProviderCredentialStoreKind::FileAuthJson);
        #[cfg(not(unix))]
        assert_eq!(
            kind,
            ProductionProviderCredentialStoreKind::OsCredentialManager
        );
    }

    #[test]
    fn config_new_defaults_to_in_memory_credential_store_under_tests() {
        let config = RociConfig::new().with_token_store(None);
        let debug = format!("{config:?}");
        assert!(debug.contains("provider_credential_store: Some(\"configured\")"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_file_default_store_debug_identifies_type_without_paths() {
        use crate::auth::FileProviderCredentialStore;

        assert_eq!(
            production_provider_credential_store_kind(),
            ProductionProviderCredentialStoreKind::FileAuthJson
        );

        let temp = TempDir::new().unwrap();
        let root = temp.path().join(".roci");
        let store = FileProviderCredentialStore::new(&root);
        let debug = format!("{store:?}");
        assert_eq!(debug, "FileProviderCredentialStore([REDACTED])");
        assert!(!debug.contains(root.to_string_lossy().as_ref()));

        let record = ProviderCredentialRecord::new(
            ProviderApiKey::new("file-default-secret"),
            Some(ProviderEndpoint::new("https://file.example")),
        );
        store.save("openai", &record).unwrap();

        let config = RociConfig::new()
            .with_token_store(None)
            .with_provider_credential_store(Some(Arc::new(store)));
        assert_eq!(
            config.get_api_key("openai"),
            Some("file-default-secret".into())
        );
        let config_debug = format!("{config:?}");
        assert!(config_debug.contains("provider_credential_store: Some(\"configured\")"));
        assert!(!config_debug.contains("file-default-secret"));
        assert!(!config_debug.contains(root.to_string_lossy().as_ref()));
    }

    #[cfg(not(unix))]
    #[test]
    fn non_unix_os_default_store_debug_identifies_type() {
        use crate::auth::credential::OsProviderCredentialStore;

        assert_eq!(
            production_provider_credential_store_kind(),
            ProductionProviderCredentialStoreKind::OsCredentialManager
        );
        let store = OsProviderCredentialStore::new();
        assert_eq!(format!("{store:?}"), "OsProviderCredentialStore(..)");
    }
}
