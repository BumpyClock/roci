//! AuthBackend implementations for built-in OAuth providers.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use roci_core::auth::{
    AuthBackend, AuthError, AuthPollResult, AuthStep, CredentialFlow, DeviceCodeSession, Token,
    TokenStore,
};

use super::claude_code::{ClaudeCodeAuth, PkceSession};
use super::github_copilot::GitHubCopilotAuth;
use super::openai_codex::{CodexPkceSession, OpenAiCodexAuth};

// ---------------------------------------------------------------------------
// GitHub Copilot
// ---------------------------------------------------------------------------

pub struct GitHubCopilotBackend;

#[async_trait]
impl AuthBackend for GitHubCopilotBackend {
    fn aliases(&self) -> &[&str] {
        &["copilot", "github-copilot", "github"]
    }

    fn display_name(&self) -> &str {
        "GitHub Copilot"
    }

    fn store_key(&self) -> &str {
        "github-copilot"
    }

    fn runtime_store_key(&self) -> &str {
        "github-copilot-api"
    }

    fn canonical_provider_key(&self) -> &str {
        "github-copilot"
    }

    fn oauth_flow(&self) -> CredentialFlow {
        CredentialFlow::DeviceCode
    }

    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
        let auth = GitHubCopilotAuth::new(store.clone());
        let session = auth.start_device_code().await?;
        Ok(AuthStep::DeviceCode {
            verification_url: session.verification_url.clone(),
            user_code: session.user_code.clone(),
            interval: Duration::from_secs(session.interval_secs),
            expires_at: session.expires_at,
            session,
        })
    }

    async fn poll_device_code(
        &self,
        store: &Arc<dyn TokenStore>,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        let auth = GitHubCopilotAuth::new(store.clone());
        let result = auth.poll_device_code(session).await?;

        // Exchange Copilot JWT on success; both exchange and save must succeed
        // for the login to be considered successful.
        if let AuthPollResult::Authorized { .. } = &result {
            let copilot_token = auth.exchange_copilot_token().await?;
            let api_token = Token {
                provider_metadata: None,
                access_token: copilot_token.token,
                refresh_token: None,
                id_token: None,
                expires_at: Some(copilot_token.expires_at),
                last_refresh: Some(Utc::now()),
                scopes: None,
                account_id: Some(copilot_token.base_url),
            };
            store.save("github-copilot-api", "default", &api_token)?;
        }

        Ok(result)
    }

    async fn complete_pkce(
        &self,
        _store: &Arc<dyn TokenStore>,
        _code: &str,
        _state: &str,
        _session_data: &serde_json::Value,
    ) -> Result<Token, AuthError> {
        Err(AuthError::Unsupported(
            "GitHub Copilot uses device-code flow, not PKCE".into(),
        ))
    }

    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        match store.load(self.store_key(), "default")? {
            Some(token) => Ok(Some(token)),
            None => store.load("github-copilot-api", "default"),
        }
    }

    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError> {
        let primary_token = store.load(self.store_key(), "default")?;

        store.clear(self.store_key(), "default")?;
        if let Err(clear_error) = store.clear("github-copilot-api", "default") {
            if let Some(token) = primary_token {
                if let Err(rollback_error) = store.save(self.store_key(), "default", &token) {
                    return Err(AuthError::Io(format!(
                        "failed to clear github-copilot-api after clearing github-copilot: {clear_error}; failed to restore github-copilot: {rollback_error}"
                    )));
                }
            }
            return Err(clear_error);
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// OpenAI Codex
// ---------------------------------------------------------------------------

pub struct OpenAiCodexBackend;

#[async_trait]
impl AuthBackend for OpenAiCodexBackend {
    fn oauth_flows(&self) -> Vec<CredentialFlow> {
        vec![CredentialFlow::DeviceCode, CredentialFlow::Pkce]
    }

    async fn start_login_with_flow(
        &self,
        store: &Arc<dyn TokenStore>,
        flow: CredentialFlow,
    ) -> Result<AuthStep, AuthError> {
        match flow {
            CredentialFlow::DeviceCode => self.start_login(store).await,
            CredentialFlow::Pkce => {
                let session = OpenAiCodexAuth::new(store.clone()).start_browser_login()?;
                Ok(AuthStep::Pkce {
                    authorize_url: session.authorize_url.clone(),
                    state: session.state.clone(),
                    session_data: serde_json::to_value(session)?,
                })
            }
            _ => Err(AuthError::Unsupported(
                "Codex supports device-code and PKCE login".into(),
            )),
        }
    }

    async fn complete_pkce(
        &self,
        store: &Arc<dyn TokenStore>,
        code: &str,
        state: &str,
        session_data: &serde_json::Value,
    ) -> Result<Token, AuthError> {
        let session: CodexPkceSession = serde_json::from_value(session_data.clone())?;
        if session.state != state {
            return Err(AuthError::InvalidResponse(
                "Codex login session state mismatch".into(),
            ));
        }
        OpenAiCodexAuth::new(store.clone())
            .complete_browser_login(&session, code)
            .await
    }
    fn aliases(&self) -> &[&str] {
        &["chatgpt", "codex", "openai-codex"]
    }

    fn display_name(&self) -> &str {
        "Codex"
    }

    fn store_key(&self) -> &str {
        "openai-codex"
    }

    fn canonical_provider_key(&self) -> &str {
        "codex"
    }

    fn import_credentials(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        OpenAiCodexAuth::new(store.clone()).import_codex_auth_json(None)
    }

    fn oauth_flow(&self) -> CredentialFlow {
        CredentialFlow::DeviceCode
    }

    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
        let auth = OpenAiCodexAuth::new(store.clone());
        let session = auth.start_device_code().await?;
        Ok(AuthStep::DeviceCode {
            verification_url: session.verification_url.clone(),
            user_code: session.user_code.clone(),
            interval: Duration::from_secs(session.interval_secs),
            expires_at: session.expires_at,
            session,
        })
    }

    async fn poll_device_code(
        &self,
        store: &Arc<dyn TokenStore>,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        let auth = OpenAiCodexAuth::new(store.clone());
        let result = auth.poll_device_code(session).await?;
        Ok(result)
    }

    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        store.load(self.store_key(), "default")
    }

    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError> {
        store.clear(self.store_key(), "default")
    }
}

// ---------------------------------------------------------------------------
// Claude Code
// ---------------------------------------------------------------------------

pub struct ClaudeCodeBackend;

#[async_trait]
impl AuthBackend for ClaudeCodeBackend {
    fn aliases(&self) -> &[&str] {
        &["claude", "anthropic", "claude-code"]
    }

    fn display_name(&self) -> &str {
        "Claude"
    }

    fn store_key(&self) -> &str {
        "claude-code"
    }

    fn canonical_provider_key(&self) -> &str {
        "anthropic"
    }

    fn import_credentials(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        ClaudeCodeAuth::new(store.clone()).import_cli_credentials(None)
    }

    fn oauth_flow(&self) -> CredentialFlow {
        CredentialFlow::Pkce
    }

    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
        let auth = ClaudeCodeAuth::new(store.clone());
        let session = auth.start_auth()?;
        Ok(AuthStep::Pkce {
            authorize_url: session.authorize_url.clone(),
            state: session.state.clone(),
            session_data: pkce_session_to_json(&session),
        })
    }

    async fn poll_device_code(
        &self,
        _store: &Arc<dyn TokenStore>,
        _session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        Err(AuthError::Unsupported(
            "Claude Code uses PKCE flow, not device-code".into(),
        ))
    }

    async fn complete_pkce(
        &self,
        store: &Arc<dyn TokenStore>,
        code: &str,
        _state: &str,
        session_data: &serde_json::Value,
    ) -> Result<Token, AuthError> {
        let session = pkce_session_from_data(session_data)?;
        let auth = ClaudeCodeAuth::new(store.clone());
        auth.exchange_code(&session, code).await
    }

    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        store.load(self.store_key(), "default")
    }

    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError> {
        store.clear(self.store_key(), "default")
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pkce_session_to_json(session: &PkceSession) -> serde_json::Value {
    serde_json::json!({
        "authorize_url": session.authorize_url,
        "state": session.state,
        "code_verifier": session.code_verifier,
    })
}

/// Reconstruct a `PkceSession` from preserved session data.
///
/// All fields are required; returns an error if any are missing or empty.
fn pkce_session_from_data(data: &serde_json::Value) -> Result<PkceSession, AuthError> {
    let authorize_url = data
        .get("authorize_url")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AuthError::InvalidResponse("missing authorize_url in PKCE session data".into())
        })?
        .to_string();
    let state = data
        .get("state")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AuthError::InvalidResponse("missing state in PKCE session data".into()))?
        .to_string();
    let code_verifier = data
        .get("code_verifier")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            AuthError::InvalidResponse("missing code_verifier in PKCE session data".into())
        })?
        .to_string();
    Ok(PkceSession {
        authorize_url,
        state,
        code_verifier,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "openai")]
    #[tokio::test]
    async fn codex_exposes_both_flows_and_explicit_browser_login() {
        use roci_core::auth::{
            AuthService, FileTokenStore, HostAuthStep, ProviderAuthManager, TokenStoreConfig,
        };
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        let mut service = AuthService::new(store);
        service.register_backend(Arc::new(OpenAiCodexBackend));
        let mut registry = roci_core::provider::ProviderRegistry::new();
        crate::register_default_providers(&mut registry);
        let manager =
            ProviderAuthManager::new(service, registry, roci_core::config::RociConfig::new())
                .unwrap();
        let status = manager.status("codex").unwrap();
        assert!(status
            .descriptor
            .credential_flows
            .contains(&CredentialFlow::DeviceCode));
        assert!(status
            .descriptor
            .credential_flows
            .contains(&CredentialFlow::Pkce));
        let step = manager
            .start_login_with_flow("openai-codex", CredentialFlow::Pkce)
            .await
            .unwrap();
        assert!(matches!(step, HostAuthStep::Pkce { .. }));
        assert!(!format!("{step:?}").contains("code_verifier"));
        assert!(matches!(
            manager
                .start_login_with_flow("codex", CredentialFlow::BrowserPoll)
                .await,
            Err(AuthError::Unsupported(_))
        ));
    }

    #[test]
    fn copilot_logout_uses_derived_refresh_lease() {
        use roci_core::auth::{AuthService, FileTokenStore, TokenStoreConfig};
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        let mut service = AuthService::new(store.clone());
        service.register_backend(Arc::new(GitHubCopilotBackend));
        let lease = store
            .try_acquire_refresh_lease("github-copilot-api", "default")
            .unwrap()
            .unwrap();
        assert!(matches!(
            service.logout("copilot"),
            Err(AuthError::RateLimited {
                retry_after_ms: Some(100)
            })
        ));
        drop(lease);
        service.logout("copilot").unwrap();
    }
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn temp_store() -> (tempfile::TempDir, Arc<dyn TokenStore>) {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let store = roci_core::auth::store::FileTokenStore::new(
            roci_core::auth::store::TokenStoreConfig::new(dir.path().to_path_buf()),
        );
        (dir, Arc::new(store))
    }

    fn token(access_token: &str) -> Token {
        Token {
            provider_metadata: None,
            access_token: access_token.to_string(),
            refresh_token: None,
            id_token: None,
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            last_refresh: Some(Utc::now()),
            scopes: None,
            account_id: None,
        }
    }

    struct FailingClearTokenStore {
        tokens: Mutex<HashMap<(String, String), Token>>,
        fail_clear_provider: &'static str,
    }

    impl FailingClearTokenStore {
        fn new(fail_clear_provider: &'static str) -> Self {
            Self {
                tokens: Mutex::new(HashMap::new()),
                fail_clear_provider,
            }
        }
    }

    impl TokenStore for FailingClearTokenStore {
        fn save_if_current(
            &self,
            _provider: &str,
            _profile: &str,
            _expected: Option<&Token>,
            _replacement: &Token,
        ) -> Result<bool, AuthError> {
            panic!("backend logout test must not refresh tokens")
        }

        fn try_acquire_refresh_lease(
            &self,
            _provider: &str,
            _profile: &str,
        ) -> Result<Option<Box<dyn roci_core::auth::TokenRefreshLease>>, AuthError> {
            panic!("direct backend logout test must not acquire refresh leases")
        }

        fn load(&self, provider: &str, profile: &str) -> Result<Option<Token>, AuthError> {
            Ok(self
                .tokens
                .lock()
                .expect("tokens lock")
                .get(&(provider.to_string(), profile.to_string()))
                .cloned())
        }

        fn save(&self, provider: &str, profile: &str, token: &Token) -> Result<(), AuthError> {
            self.tokens
                .lock()
                .expect("tokens lock")
                .insert((provider.to_string(), profile.to_string()), token.clone());
            Ok(())
        }

        fn clear(&self, provider: &str, profile: &str) -> Result<(), AuthError> {
            if provider == self.fail_clear_provider {
                return Err(AuthError::Io(format!("clear failed for {provider}")));
            }
            self.tokens
                .lock()
                .expect("tokens lock")
                .remove(&(provider.to_string(), profile.to_string()));
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // PKCE session data round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn pkce_session_round_trips_through_json() {
        let session = PkceSession {
            authorize_url: "https://example.com/authorize".to_string(),
            state: "random-state".to_string(),
            code_verifier: "secret-verifier".to_string(),
        };
        let json = pkce_session_to_json(&session);
        let restored = pkce_session_from_data(&json).expect("should parse");
        assert_eq!(restored.authorize_url, session.authorize_url);
        assert_eq!(restored.state, session.state);
        assert_eq!(restored.code_verifier, session.code_verifier);
    }

    #[test]
    fn pkce_session_from_data_rejects_missing_code_verifier() {
        let data = serde_json::json!({
            "authorize_url": "https://example.com/authorize",
            "state": "random-state",
        });
        let err = pkce_session_from_data(&data).unwrap_err();
        match err {
            AuthError::InvalidResponse(msg) => {
                assert!(
                    msg.contains("code_verifier"),
                    "error should mention code_verifier: {msg}"
                );
            }
            other => panic!("expected InvalidResponse, got {other:?}"),
        }
    }

    #[test]
    fn pkce_session_from_data_rejects_empty_fields() {
        let data = serde_json::json!({
            "authorize_url": "",
            "state": "s",
            "code_verifier": "v",
        });
        let err = pkce_session_from_data(&data).unwrap_err();
        match err {
            AuthError::InvalidResponse(msg) => {
                assert!(
                    msg.contains("authorize_url"),
                    "error should mention authorize_url: {msg}"
                );
            }
            other => panic!("expected InvalidResponse, got {other:?}"),
        }
    }

    #[test]
    fn pkce_session_from_data_rejects_non_object() {
        let data = serde_json::json!("just-a-string");
        let err = pkce_session_from_data(&data).unwrap_err();
        assert!(matches!(err, AuthError::InvalidResponse(_)));
    }

    // -----------------------------------------------------------------------
    // ClaudeCodeBackend::complete_pkce requires session_data
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn claude_complete_pkce_rejects_invalid_session_data() {
        let (_dir, store) = temp_store();
        let backend = ClaudeCodeBackend;
        let result = backend
            .complete_pkce(&store, "code", "state", &serde_json::Value::Null)
            .await;
        assert!(result.is_err());
        match result.unwrap_err() {
            AuthError::InvalidResponse(msg) => {
                assert!(
                    msg.contains("PKCE session data"),
                    "error should identify invalid session data: {msg}"
                );
            }
            other => panic!("expected InvalidResponse, got {other:?}"),
        }
    }

    #[test]
    fn copilot_status_falls_back_to_api_token_store() {
        let (_dir, store) = temp_store();
        store
            .save("github-copilot-api", "default", &token("api-token"))
            .expect("save token");
        let backend = GitHubCopilotBackend;

        let status = backend.get_status(&store).expect("status");

        assert_eq!(
            status.map(|token| token.access_token).as_deref(),
            Some("api-token")
        );
    }

    #[test]
    fn copilot_status_prefers_primary_token_store() {
        let (_dir, store) = temp_store();
        store
            .save("github-copilot", "default", &token("primary-token"))
            .expect("save primary token");
        store
            .save("github-copilot-api", "default", &token("api-token"))
            .expect("save api token");
        let backend = GitHubCopilotBackend;

        let status = backend.get_status(&store).expect("status");

        assert_eq!(
            status.map(|token| token.access_token).as_deref(),
            Some("primary-token")
        );
    }

    #[test]
    fn copilot_logout_clears_primary_and_api_token_stores() {
        let (_dir, store) = temp_store();
        store
            .save("github-copilot", "default", &token("primary-token"))
            .expect("save primary token");
        store
            .save("github-copilot-api", "default", &token("api-token"))
            .expect("save api token");
        let backend = GitHubCopilotBackend;

        backend.logout(&store).expect("logout");

        assert!(store
            .load("github-copilot", "default")
            .expect("load primary")
            .is_none());
        assert!(store
            .load("github-copilot-api", "default")
            .expect("load api")
            .is_none());
    }

    #[test]
    fn copilot_logout_restores_primary_token_when_api_clear_fails() {
        let store: Arc<dyn TokenStore> =
            Arc::new(FailingClearTokenStore::new("github-copilot-api"));
        store
            .save("github-copilot", "default", &token("primary-token"))
            .expect("save primary token");
        store
            .save("github-copilot-api", "default", &token("api-token"))
            .expect("save api token");
        let backend = GitHubCopilotBackend;

        let err = backend.logout(&store).unwrap_err();

        assert!(matches!(err, AuthError::Io(_)));
        assert_eq!(
            store
                .load("github-copilot", "default")
                .expect("load primary")
                .map(|token| token.access_token)
                .as_deref(),
            Some("primary-token")
        );
        assert_eq!(
            store
                .load("github-copilot-api", "default")
                .expect("load api")
                .map(|token| token.access_token)
                .as_deref(),
            Some("api-token")
        );
    }
}
