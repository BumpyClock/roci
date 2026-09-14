//! Configuration system (layered: code/env > protected credentials > OAuth).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, RwLock};

#[cfg(test)]
use crate::auth::credential::InMemoryProviderCredentialStore;
#[cfg(all(not(test), not(unix)))]
use crate::auth::credential::OsProviderCredentialStore;
use crate::auth::credential::{
    ProviderCredentialRecord, ProviderCredentialStore, ProviderCredentialStoreError,
};
use crate::auth::store::TokenStore;
#[cfg(all(not(test), unix))]
use crate::auth::FileProviderCredentialStore;
use crate::models::ProviderKey;

/// Layered configuration for Roci.
///
/// Resolution order for API keys and endpoints:
/// 1. Explicit in-process/environment maps
/// 2. Protected [`ProviderCredentialStore`] records
/// 3. OAuth tokens from `TokenStore` aliases (identity and refresh state preserved)
#[derive(Clone)]
pub struct RociConfig {
    api_keys: Arc<RwLock<HashMap<String, String>>>,
    base_urls: Arc<RwLock<HashMap<String, String>>>,
    account_ids: Arc<RwLock<HashMap<String, String>>>,
    account: String,
    raw_token_store: Option<Arc<dyn TokenStore>>,
    raw_credential_store: Option<Arc<dyn ProviderCredentialStore>>,
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

#[cfg(all(not(test), unix))]
fn default_provider_credential_store() -> Option<Arc<dyn ProviderCredentialStore>> {
    match FileProviderCredentialStore::new_default() {
        Ok(store) => Some(Arc::new(store)),
        Err(error) => {
            tracing::warn!(%error, "default provider credential store unavailable");
            None
        }
    }
}

#[cfg(all(not(test), not(unix)))]
fn default_provider_credential_store() -> Option<Arc<dyn ProviderCredentialStore>> {
    Some(Arc::new(OsProviderCredentialStore::new()))
}

#[cfg(test)]
fn default_provider_credential_store() -> Option<Arc<dyn ProviderCredentialStore>> {
    Some(Arc::new(InMemoryProviderCredentialStore::default()))
}

impl RociConfig {
    /// Select credentials without discarding OAuth identity or expired refreshable tokens.
    /// Storage failures stop resolution rather than falling through to another source.
    pub fn resolve_provider_credential(
        &self,
        provider: &str,
    ) -> Result<Option<crate::auth::ResolvedProviderCredential>, crate::auth::AuthError> {
        use crate::auth::{
            AuthError, CredentialMaterial, ProviderApiKey, ProviderEndpoint,
            ResolvedProviderCredential,
        };
        let key = ProviderKey::parse(provider);
        let explicit_endpoint =
            get_from_map(&self.base_urls, provider, key).map(ProviderEndpoint::new);
        if let Some(api_key) = get_from_map(&self.api_keys, provider, key) {
            return Ok(Some(ResolvedProviderCredential {
                material: CredentialMaterial::ApiKey(ProviderApiKey::new(api_key)),
                endpoint: explicit_endpoint,
            }));
        }
        if let Some(record) = self
            .stored_credential(provider)
            .map_err(|_| AuthError::Io("provider credential store could not be read".into()))?
        {
            return Ok(Some(ResolvedProviderCredential {
                material: CredentialMaterial::ApiKey(record.api_key),
                endpoint: explicit_endpoint.or(record.endpoint),
            }));
        }
        let Some(store_key) = key.and_then(ProviderKey::token_store_key) else {
            return Ok(None);
        };
        let Some(store) = self.token_store.as_ref() else {
            return Ok(None);
        };
        Ok(store
            .load(store_key, "default")?
            .map(|token| ResolvedProviderCredential {
                material: CredentialMaterial::OAuth(token),
                endpoint: explicit_endpoint,
            }))
    }

    /// Create empty config with default file-backed token store.
    pub fn new() -> Self {
        let token_store: Option<Arc<dyn TokenStore>> =
            Some(Arc::new(crate::auth::store::FileTokenStore::new_default()));
        let credential_store = default_provider_credential_store();
        Self {
            account: "default".into(),
            raw_token_store: token_store.clone(),
            raw_credential_store: credential_store.clone(),
            api_keys: Arc::new(RwLock::new(HashMap::new())),
            base_urls: Arc::new(RwLock::new(HashMap::new())),
            account_ids: Arc::new(RwLock::new(HashMap::new())),
            token_store,
            provider_credential_store: credential_store,
        }
    }

    /// Create config with a specific token store (or `None` to disable fallback).
    pub fn with_token_store(mut self, store: Option<Arc<dyn TokenStore>>) -> Self {
        self.raw_token_store = store.map(|store| store.unscoped_store().unwrap_or(store));
        self.scope_stores();
        self
    }

    /// Select a named account for login and execution. No default-account fallback.
    ///
    /// Nondefault selection clears environment/in-process keys so another account
    /// cannot silently shadow this selection. Set explicit overrides afterwards.
    pub fn with_account(
        mut self,
        account: impl Into<String>,
    ) -> Result<Self, crate::auth::AuthError> {
        let account = account.into();
        crate::auth::account::validate_account(&account)?;
        if account != self.account {
            self.api_keys = Arc::new(RwLock::new(HashMap::new()));
            self.account_ids = Arc::new(RwLock::new(HashMap::new()));
        }
        self.account = account;
        self.scope_stores();
        Ok(self)
    }

    /// Account namespace selected for this configuration and its agent sessions.
    pub fn account(&self) -> &str {
        &self.account
    }

    fn scope_stores(&mut self) {
        self.token_store = self.raw_token_store.as_ref().map(|inner| {
            Arc::new(crate::auth::account::AccountTokenStore {
                inner: inner.clone(),
                account: self.account.clone(),
            }) as Arc<dyn TokenStore>
        });
        self.provider_credential_store = self.raw_credential_store.as_ref().map(|inner| {
            Arc::new(crate::auth::account::AccountCredentialStore {
                inner: inner.clone(),
                account: self.account.clone(),
            }) as Arc<dyn ProviderCredentialStore>
        });
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
        self.raw_credential_store = store.map(|store| store.unscoped_store().unwrap_or(store));
        self.scope_stores();
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

    pub fn set_base_url(&self, provider: &str, url: String) {
        self.base_urls
            .write()
            .unwrap()
            .insert(provider.to_string(), url);
    }

    /// Resolve an endpoint for a provider that does not require credentials.
    ///
    /// Authenticated callers must use the endpoint returned with
    /// [`Self::resolve_provider_credential`] to keep key and endpoint paired.
    pub fn resolve_provider_endpoint(
        &self,
        provider: &str,
    ) -> Result<Option<crate::auth::ProviderEndpoint>, crate::auth::AuthError> {
        let key = ProviderKey::parse(provider);
        if let Some(endpoint) = get_from_map(&self.base_urls, provider, key) {
            return Ok(Some(crate::auth::ProviderEndpoint::new(endpoint)));
        }
        if get_from_map(&self.api_keys, provider, key).is_some() {
            return Ok(None);
        }
        self.stored_credential(provider)
            .map(|record| record.and_then(|record| record.endpoint))
            .map_err(|_| {
                crate::auth::AuthError::Io("provider credential store could not be read".into())
            })
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
        self.resolve_provider_credential(provider)
            .ok()
            .flatten()
            .is_some_and(|credential| match credential.material {
                crate::auth::CredentialMaterial::ApiKey(_) => true,
                crate::auth::CredentialMaterial::OAuth(token) => {
                    token.is_valid() || token.refresh_token.is_some()
                }
            })
    }

    /// True when an explicit/env API key is set (ignores all stored fallback).
    pub fn has_explicit_api_key(&self, provider: &str) -> bool {
        let provider_key = ProviderKey::parse(provider);
        get_from_map(&self.api_keys, provider, provider_key).is_some()
    }

    /// True when a protected Roci-owned provider credential record is present.
    pub fn has_stored_api_key(&self, provider: &str) -> bool {
        match self.stored_credential(provider) {
            Ok(record) => record.is_some(),
            Err(error) => {
                tracing::warn!(%error, "failed to load protected provider credentials");
                false
            }
        }
    }

    fn stored_credential(
        &self,
        provider: &str,
    ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
        let canonical = ProviderKey::parse(provider)
            .map(ProviderKey::as_str)
            .unwrap_or(provider);
        let Some(store) = self.provider_credential_store.as_ref() else {
            return Ok(None);
        };
        store.load(canonical)
    }
}

#[cfg(test)]
fn resolved_secret(config: &RociConfig, provider: &str) -> Option<String> {
    config
        .resolve_provider_credential(provider)
        .unwrap()
        .map(|credential| match credential.material {
            crate::auth::CredentialMaterial::ApiKey(key) => key.expose_secret().to_owned(),
            crate::auth::CredentialMaterial::OAuth(token) => token.access_token,
        })
}

#[cfg(test)]
fn resolved_endpoint(config: &RociConfig, provider: &str) -> Option<String> {
    config
        .resolve_provider_endpoint(provider)
        .unwrap()
        .map(|url| url.as_str().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::credential::{
        InMemoryProviderCredentialStore, ProviderApiKey, ProviderCredentialStoreError,
        ProviderEndpoint,
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

    struct RotatingCredentialStore {
        first: ProviderCredentialRecord,
        second: ProviderCredentialRecord,
        loads: std::sync::atomic::AtomicUsize,
    }

    impl ProviderCredentialStore for RotatingCredentialStore {
        fn load(
            &self,
            _provider: &str,
        ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
            let index = self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(if index == 0 {
                self.first.clone()
            } else {
                self.second.clone()
            }))
        }

        fn save(
            &self,
            _provider: &str,
            _record: &ProviderCredentialRecord,
        ) -> Result<(), ProviderCredentialStoreError> {
            Ok(())
        }

        fn clear(&self, _provider: &str) -> Result<(), ProviderCredentialStoreError> {
            Ok(())
        }
    }

    struct CountingTokenStore {
        loads: std::sync::atomic::AtomicUsize,
    }

    impl TokenStore for CountingTokenStore {
        fn load(
            &self,
            _provider: &str,
            _profile: &str,
        ) -> Result<Option<Token>, crate::auth::AuthError> {
            self.loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        }

        fn save(
            &self,
            _provider: &str,
            _profile: &str,
            _token: &Token,
        ) -> Result<(), crate::auth::AuthError> {
            Ok(())
        }

        fn clear(&self, _provider: &str, _profile: &str) -> Result<(), crate::auth::AuthError> {
            Ok(())
        }

        fn save_if_current(
            &self,
            _: &str,
            _: &str,
            _: Option<&Token>,
            _: &Token,
        ) -> Result<bool, crate::auth::AuthError> {
            panic!("credential lookup test must not refresh tokens")
        }

        fn try_acquire_refresh_lease(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<Box<dyn crate::auth::store::TokenRefreshLease>>, crate::auth::AuthError>
        {
            panic!("credential lookup test must not acquire leases")
        }
    }

    struct FailingLoadCredentialStore;

    impl ProviderCredentialStore for FailingLoadCredentialStore {
        fn load(
            &self,
            _provider: &str,
        ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
            Err(ProviderCredentialStoreError::InvalidRecord)
        }

        fn save(
            &self,
            _provider: &str,
            _record: &ProviderCredentialRecord,
        ) -> Result<(), ProviderCredentialStoreError> {
            Ok(())
        }

        fn clear(&self, _provider: &str) -> Result<(), ProviderCredentialStoreError> {
            Ok(())
        }
    }

    fn make_token(access_token: &str, expires_at: Option<chrono::DateTime<Utc>>) -> Token {
        Token {
            provider_metadata: None,
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

        assert_eq!(resolved_secret(&config, "openai"), None);
    }

    #[test]
    fn codex_falls_back_to_token_store() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let token = make_token("oauth-access-token", None);
        store.save("openai-codex", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert_eq!(
            resolved_secret(&config, "codex"),
            Some("oauth-access-token".to_string()),
        );
    }

    #[test]
    fn provider_store_failure_does_not_fall_back_to_oauth_token() {
        let dir = TempDir::new().unwrap();
        let token_store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().to_path_buf(),
        )));
        token_store
            .save(
                "claude-code",
                "default",
                &make_token("oauth-access-token", None),
            )
            .unwrap();
        let config = RociConfig::new()
            .with_token_store(Some(token_store))
            .with_provider_credential_store(Some(Arc::new(FailingLoadCredentialStore)));

        assert!(config.resolve_provider_credential("anthropic").is_err());
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
            resolved_secret(&config, "openai"),
            Some("env-api-key".to_string()),
        );
    }

    #[test]
    fn expired_codex_token_retains_oauth_identity() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let expired = Utc::now() - Duration::hours(1);
        let token = make_token("stale-token", Some(expired));
        store.save("openai-codex", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        let credential = config
            .resolve_provider_credential("codex")
            .unwrap()
            .unwrap();
        assert!(
            matches!(credential.material, crate::auth::CredentialMaterial::OAuth(token) if token.access_token == "stale-token" && token.expires_at == Some(expired))
        );
    }

    #[test]
    fn non_expired_codex_token_in_store_is_returned() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let future = Utc::now() + Duration::hours(1);
        let token = make_token("fresh-token", Some(future));
        store.save("openai-codex", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert_eq!(
            resolved_secret(&config, "codex"),
            Some("fresh-token".to_string()),
        );
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

        assert_eq!(resolved_secret(&config, "some-unknown-provider"), None);
    }

    #[test]
    fn config_without_token_store_returns_none_for_missing_key() {
        let config = RociConfig::new()
            .with_token_store(None)
            .with_provider_credential_store(None);

        assert_eq!(resolved_secret(&config, "openai"), None);
    }

    #[test]
    fn anthropic_falls_back_to_claude_code_token() {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        let token = make_token("claude-oauth-token", None);
        store.save("claude-code", "default", &token).unwrap();

        let config = config_with_temp_store(dir.path());

        assert_eq!(
            resolved_secret(&config, "anthropic"),
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
            resolved_secret(&config, "github-copilot"),
            Some("copilot-token".to_string()),
        );
    }

    #[test]
    fn api_key_and_base_url_share_one_stored_record_snapshot() {
        let store = Arc::new(RotatingCredentialStore {
            first: ProviderCredentialRecord::new(
                ProviderApiKey::new("first-key"),
                Some(ProviderEndpoint::new("https://first.example")),
            ),
            second: ProviderCredentialRecord::new(
                ProviderApiKey::new("second-key"),
                Some(ProviderEndpoint::new("https://second.example")),
            ),
            loads: std::sync::atomic::AtomicUsize::new(0),
        });
        let config = config_with_credential_store(store.clone());

        let credential = config
            .resolve_provider_credential("anthropic")
            .unwrap()
            .unwrap();
        let crate::auth::CredentialMaterial::ApiKey(key) = credential.material else {
            panic!("expected API key")
        };
        let credentials = (
            Some(key.expose_secret().to_owned()),
            credential.endpoint.map(|url| url.as_str().to_owned()),
        );

        assert_eq!(
            credentials,
            (
                Some("first-key".to_string()),
                Some("https://first.example".to_string())
            )
        );
        assert_eq!(store.loads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn stored_record_skips_oauth_token_store_load() {
        let token_store = Arc::new(CountingTokenStore {
            loads: std::sync::atomic::AtomicUsize::new(0),
        });
        let credential_store = Arc::new(InMemoryProviderCredentialStore::default());
        credential_store
            .save(
                "anthropic",
                &ProviderCredentialRecord::new(ProviderApiKey::new("stored-key"), None),
            )
            .unwrap();
        let config = RociConfig::new()
            .with_token_store(Some(token_store.clone()))
            .with_provider_credential_store(Some(credential_store));

        assert_eq!(
            resolved_secret(&config, "anthropic"),
            Some("stored-key".into())
        );
        assert_eq!(
            token_store.loads.load(std::sync::atomic::Ordering::SeqCst),
            0
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

        assert_eq!(
            resolved_secret(&config, "anthropic"),
            Some("explicit-key".into())
        );
        assert_eq!(
            resolved_endpoint(&config, "anthropic"),
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

        assert_eq!(
            resolved_secret(&config, "anthropic"),
            Some("explicit-key".into())
        );
        assert_eq!(resolved_endpoint(&config, "anthropic"), None);
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

        assert_eq!(
            resolved_secret(&config, "anthropic"),
            Some("stored-key".into())
        );
        assert_eq!(
            resolved_endpoint(&config, "anthropic"),
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
    fn config_new_defaults_to_isolated_credential_stores_under_tests() {
        let first = RociConfig::new().with_token_store(None);
        let second = RociConfig::new().with_token_store(None);
        let first_store = first.provider_credential_store().expect("default store");
        let second_store = second.provider_credential_store().expect("default store");
        let provider = format!("test-isolation-{}", uuid::Uuid::new_v4());
        let record = ProviderCredentialRecord::new(ProviderApiKey::new("test-key"), None);

        first_store
            .save(&provider, &record)
            .expect("save credential");
        let first_record = first_store.load(&provider);
        let second_record = second_store.load(&provider);
        first_store.clear(&provider).expect("clear test credential");

        assert_eq!(first_record.expect("load saved credential"), Some(record));
        assert_eq!(second_record.expect("load independent store"), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_file_store_config_resolves_credentials_and_redacts_debug() {
        use crate::auth::FileProviderCredentialStore;

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
            resolved_secret(&config, "openai"),
            Some("file-default-secret".into())
        );
        let config_debug = format!("{config:?}");
        assert!(config_debug.contains("provider_credential_store: Some(\"configured\")"));
        assert!(!config_debug.contains("file-default-secret"));
        assert!(!config_debug.contains(root.to_string_lossy().as_ref()));
    }

    #[cfg(not(unix))]
    #[test]
    fn non_unix_os_store_debug_identifies_type() {
        use crate::auth::credential::OsProviderCredentialStore;

        let store = OsProviderCredentialStore::new();
        assert_eq!(format!("{store:?}"), "OsProviderCredentialStore(..)");
    }
}

#[cfg(test)]
mod account_tests {
    use super::*;
    use crate::auth::{FileTokenStore, ProviderApiKey, Token, TokenStoreConfig};

    #[test]
    fn named_accounts_isolate_tokens_keys_logout_and_refresh_publication() {
        let dir = tempfile::tempdir().unwrap();
        let tokens = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().to_owned(),
        )));
        let base = RociConfig::new().with_token_store(Some(tokens));
        let work = base.clone().with_account("work").unwrap();
        let personal = base.clone().with_account("personal").unwrap();
        let token = Token {
            access_token: "work-token".into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: None,
            account_id: None,
            provider_metadata: None,
        };
        work.token_store()
            .unwrap()
            .save("openai-codex", "default", &token)
            .unwrap();
        assert!(work.resolve_provider_credential("codex").unwrap().is_some());
        assert!(personal
            .resolve_provider_credential("codex")
            .unwrap()
            .is_none());
        assert!(base.resolve_provider_credential("codex").unwrap().is_none());
        let key = ProviderCredentialRecord::new(ProviderApiKey::new("work-key"), None);
        work.provider_credential_store()
            .unwrap()
            .save("openai", &key)
            .unwrap();
        assert!(work.has_credentials("openai"));
        assert!(!personal.has_credentials("openai"));
        assert!(!base.has_credentials("openai"));
        personal
            .token_store()
            .unwrap()
            .clear("openai-codex", "default")
            .unwrap();
        assert!(work.has_credentials("codex"));
        work.token_store()
            .unwrap()
            .clear("openai-codex", "default")
            .unwrap();
        assert!(!work
            .token_store()
            .unwrap()
            .save_if_current("openai-codex", "default", Some(&token), &token)
            .unwrap());
    }

    #[test]
    fn selecting_missing_account_never_uses_environment_or_another_account() {
        let config = RociConfig::new()
            .with_token_store(None)
            .with_provider_credential_store(None);
        config.set_api_key("openai", "default-key".into());
        let work = config.clone().with_account("work").unwrap();
        assert!(!work.has_credentials("openai"));
        assert!(config.has_credentials("openai"));
        for invalid in ["", "../work", "WORK", "work_home", "---", " work"] {
            assert!(config.clone().with_account(invalid).is_err());
        }
    }

    #[test]
    fn reinjected_account_facades_can_be_rebound_without_nested_namespaces() {
        let dir = tempfile::tempdir().unwrap();
        let make_token = |access: &str, _expiry: Option<chrono::DateTime<chrono::Utc>>| {
            serde_json::from_value::<Token>(serde_json::json!({"access_token":access})).unwrap()
        };
        let raw_tokens = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        raw_tokens
            .save("claude-code", "work", &make_token("work-oauth", None))
            .unwrap();
        raw_tokens
            .save(
                "claude-code",
                "personal",
                &make_token("personal-oauth", None),
            )
            .unwrap();
        let raw_credentials = Arc::new(InMemoryProviderCredentialStore::default());
        raw_credentials
            .save(
                "google@work",
                &ProviderCredentialRecord::new(ProviderApiKey::new("work-key"), None),
            )
            .unwrap();
        raw_credentials
            .save(
                "google@personal",
                &ProviderCredentialRecord::new(ProviderApiKey::new("personal-key"), None),
            )
            .unwrap();
        let work = RociConfig::new()
            .with_token_store(Some(raw_tokens))
            .with_provider_credential_store(Some(raw_credentials))
            .with_account("work")
            .unwrap();
        let personal = RociConfig::new()
            .with_token_store(work.token_store().cloned())
            .with_provider_credential_store(work.provider_credential_store().cloned())
            .with_account("personal")
            .unwrap();
        assert_eq!(
            resolved_secret(&work, "anthropic").as_deref(),
            Some("work-oauth")
        );
        assert_eq!(
            resolved_secret(&personal, "anthropic").as_deref(),
            Some("personal-oauth")
        );
        assert_eq!(
            resolved_secret(&work, "google").as_deref(),
            Some("work-key")
        );
        assert_eq!(
            resolved_secret(&personal, "google").as_deref(),
            Some("personal-key")
        );
        assert_ne!(
            work.token_store()
                .unwrap()
                .refresh_coordination_identity("default"),
            personal
                .token_store()
                .unwrap()
                .refresh_coordination_identity("default")
        );
    }
}
