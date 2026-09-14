//! Host-facing provider auth manager.
//!
//! Combines [`AuthService`], [`ProviderRegistry`], and [`RociConfig`] into a
//! single secret-safe API: multi-flow descriptors, status projection, and
//! login orchestration with opaque pending session IDs.

use std::collections::HashMap;
use std::sync::Arc;

use uuid::Uuid;

use crate::config::RociConfig;
use crate::provider::ProviderRegistry;

use super::credential::{ProviderApiKey, ProviderCredentialRecord, ProviderEndpoint};
use super::descriptor::{CredentialFlow, ProviderDescriptor};
use super::error::AuthError;
use super::host::{
    duration_secs, HostAuthCompletion, HostAuthPollResult, HostAuthStep, LoginSessionId,
};
use super::pending::{PendingLogin, PendingLoginStore};
use super::service::{AuthPollResult, AuthService, AuthStep};
use super::status::{ConfiguredSource, ProviderAuthState, ProviderAuthStatus};

/// Host-facing auth manager over registry + auth service + config.
///
/// Construction rejects auth backends whose canonical provider has no launch
/// factory. Descriptor assembly and status projection are hermetic (no network).
pub struct ProviderAuthManager {
    auth: AuthService,
    registry: Arc<ProviderRegistry>,
    config: RociConfig,
    /// Canonical key → merged descriptor (factory base + OAuth overlays).
    descriptors: HashMap<String, ProviderDescriptor>,
    /// Alias or canonical key → canonical key.
    key_index: HashMap<String, String>,
    /// Canonical key → preferred auth-service alias for login dispatch.
    login_aliases: HashMap<String, String>,
    pending: PendingLoginStore,
}

impl ProviderAuthManager {
    /// Build a manager, overlaying OAuth flows onto factory descriptors.
    ///
    /// Returns [`AuthError::Unsupported`] when an auth backend's canonical
    /// provider key has no registered launch factory.
    pub fn new(
        auth: AuthService,
        registry: ProviderRegistry,
        config: RociConfig,
    ) -> Result<Self, AuthError> {
        Self::new_shared(auth, Arc::new(registry), config)
    }

    /// Builds a manager over a registry shared with its execution host.
    ///
    /// Use this constructor when model catalog execution and provider auth must
    /// observe the same dynamically registered factories. The auth service's
    /// token store becomes the config's OAuth source so login, status, and
    /// provider launch always observe the same credentials.
    pub fn new_shared(
        mut auth: AuthService,
        registry: Arc<ProviderRegistry>,
        config: RociConfig,
    ) -> Result<Self, AuthError> {
        // AuthService chooses the backing storage; config chooses the account.
        // Unwrap any previous account facade before applying that selection.
        let config = config.with_token_store(Some(auth.store().clone()));
        auth.set_store(
            config
                .token_store()
                .expect("auth store was just configured")
                .clone(),
        );
        let mut descriptors: HashMap<String, ProviderDescriptor> = HashMap::new();
        let mut key_index: HashMap<String, String> = HashMap::new();
        let mut login_aliases: HashMap<String, String> = HashMap::new();

        for key in registry.provider_keys() {
            let Some(factory) = registry.factory(key) else {
                continue;
            };
            let descriptor = factory.descriptor();
            let canonical = descriptor.canonical_key.clone();
            key_index.insert(key.to_string(), canonical.clone());
            key_index
                .entry(canonical.clone())
                .or_insert_with(|| canonical.clone());
            descriptors.entry(canonical).or_insert(descriptor);
        }

        for backend in auth.backends() {
            let canonical = backend.canonical_provider_key().to_string();
            if !registry.has_provider(&canonical) {
                return Err(AuthError::Unsupported(format!(
                    "auth backend '{}' has no launch factory for canonical provider '{canonical}'",
                    backend.display_name()
                )));
            }

            let entry = descriptors.entry(canonical.clone()).or_insert_with(|| {
                registry
                    .factory(&canonical)
                    .map(|f| f.descriptor())
                    .unwrap_or_else(|| ProviderDescriptor::third_party_default(&canonical, true))
            });
            for flow in backend.oauth_flows() {
                *entry = entry.clone().with_flow(flow);
            }

            key_index.insert(canonical.clone(), canonical.clone());
            for alias in backend.aliases() {
                key_index.insert((*alias).to_string(), canonical.clone());
            }

            let preferred = backend
                .aliases()
                .iter()
                .copied()
                .find(|alias| *alias == canonical.as_str())
                .or_else(|| backend.aliases().first().copied())
                .unwrap_or(backend.store_key());
            login_aliases.insert(canonical, preferred.to_string());
        }

        Ok(Self {
            auth,
            registry,
            config,
            descriptors,
            key_index,
            login_aliases,
            pending: PendingLoginStore::new(),
        })
    }

    /// Borrow the underlying config.
    pub fn config(&self) -> &RociConfig {
        &self.config
    }

    /// Borrow the provider registry.
    pub fn registry(&self) -> &ProviderRegistry {
        self.registry.as_ref()
    }

    /// Resolve a user-facing key/alias to a merged descriptor.
    pub fn descriptor(&self, provider: &str) -> Result<&ProviderDescriptor, AuthError> {
        let canonical = self.resolve_canonical(provider)?;
        self.descriptors
            .get(canonical.as_str())
            .ok_or_else(|| AuthError::UnknownProvider(provider.to_string()))
    }

    /// List merged descriptors for every launch-capable canonical provider.
    pub fn list_descriptors(&self) -> Vec<ProviderDescriptor> {
        let mut out: Vec<_> = self.descriptors.values().cloned().collect();
        out.sort_by(|a, b| a.canonical_key.cmp(&b.canonical_key));
        out
    }

    /// Project secret-free status for one provider (alias or canonical).
    pub fn status(&self, provider: &str) -> Result<ProviderAuthStatus, AuthError> {
        let canonical = self.resolve_canonical(provider)?;
        let descriptor = self
            .descriptors
            .get(canonical.as_str())
            .cloned()
            .ok_or_else(|| AuthError::UnknownProvider(provider.to_string()))?;

        let token = self.oauth_token(&canonical)?;
        let configured_sources = self.configured_sources(&canonical, token.is_some())?;
        let auth_state = self.auth_state(&configured_sources, token.as_ref());
        let launch_available = self
            .registry
            .is_available(&canonical, &self.config)
            .unwrap_or(false);

        Ok(ProviderAuthStatus {
            descriptor,
            auth_state,
            configured_sources,
            launch_available,
        })
    }

    /// Project status for every known canonical provider.
    pub fn list_statuses(&self) -> Vec<ProviderAuthStatus> {
        let mut keys: Vec<_> = self.descriptors.keys().cloned().collect();
        keys.sort();
        keys.into_iter()
            .filter_map(|key| self.status(&key).ok())
            .collect()
    }

    /// Explicitly import external credentials and return only a host-safe receipt.
    pub fn import_credentials(&self, provider: &str) -> Result<HostAuthCompletion, AuthError> {
        let canonical = self.resolve_canonical(provider)?;
        let alias = self.login_aliases.get(canonical.as_str()).ok_or_else(|| {
            AuthError::Unsupported(format!(
                "provider '{canonical}' has no credential import backend"
            ))
        })?;
        self.auth.import_credentials(alias)?.ok_or_else(|| {
            AuthError::InvalidResponse(format!(
                "no existing credentials found to import for '{canonical}'"
            ))
        })?;
        Ok(HostAuthCompletion {
            provider: canonical,
        })
    }

    /// Start a login flow; secrets stay in the pending map.
    pub async fn start_login(&self, provider: &str) -> Result<HostAuthStep, AuthError> {
        self.start_login_selected(provider, None).await
    }

    /// Select a supported OAuth flow while retaining secrets inside the manager.
    pub async fn start_login_with_flow(
        &self,
        provider: &str,
        flow: CredentialFlow,
    ) -> Result<HostAuthStep, AuthError> {
        self.start_login_selected(provider, Some(flow)).await
    }

    async fn start_login_selected(
        &self,
        provider: &str,
        flow: Option<CredentialFlow>,
    ) -> Result<HostAuthStep, AuthError> {
        let canonical = self.resolve_canonical(provider)?;
        let alias = self
            .login_aliases
            .get(canonical.as_str())
            .cloned()
            .ok_or_else(|| {
                AuthError::Unsupported(format!("provider '{canonical}' has no OAuth login backend"))
            })?;

        let step = match flow {
            Some(flow) => self.auth.start_login_with_flow(&alias, flow).await?,
            None => self.auth.start_login(&alias).await?,
        };
        match step {
            AuthStep::BrowserPoll {
                authorization_url,
                interval,
                expires_at,
                session_data,
            } => {
                let session_id = new_session_id();
                self.pending.insert(
                    &session_id,
                    PendingLogin::BrowserPoll {
                        provider_alias: alias,
                        canonical,
                        session_data,
                        expires_at,
                    },
                )?;
                Ok(HostAuthStep::BrowserPoll {
                    authorization_url,
                    interval_secs: duration_secs(interval),
                    expires_at,
                    session_id,
                })
            }
            AuthStep::Imported { token: _ } => Ok(HostAuthStep::ImportedAndComplete {
                provider: canonical,
            }),
            AuthStep::DeviceCode {
                verification_url,
                user_code,
                interval,
                expires_at,
                session,
            } => {
                let session_id = new_session_id();
                self.pending.insert(
                    &session_id,
                    PendingLogin::DeviceCode {
                        provider_alias: alias,
                        canonical: canonical.clone(),
                        session,
                    },
                )?;
                Ok(HostAuthStep::DeviceCode {
                    verification_uri: verification_url,
                    user_code,
                    interval_secs: duration_secs(interval),
                    expires_at,
                    session_id,
                })
            }
            AuthStep::Pkce {
                authorize_url,
                state,
                session_data,
            } => {
                let session_id = new_session_id();
                self.pending.insert(
                    &session_id,
                    PendingLogin::pkce(alias, canonical.clone(), state, session_data),
                )?;
                Ok(HostAuthStep::Pkce {
                    authorization_url: authorize_url,
                    session_id,
                })
            }
        }
    }

    /// Advance a browser or device-code login using its opaque session ID.
    ///
    /// The manager selects the provider operation from the stored session. PKCE
    /// sessions require an authorization code and use [`Self::complete_pkce`].
    /// Pending and retryable failures preserve the session; terminal results
    /// consume it. Canceling this future releases its exclusive claim.
    pub async fn advance_login(
        &self,
        session_id: &LoginSessionId,
    ) -> Result<HostAuthPollResult, AuthError> {
        let claim = self.pending.claim(session_id)?;
        let (canonical, result) = match claim.pending() {
            PendingLogin::BrowserPoll {
                provider_alias,
                canonical,
                session_data,
                ..
            } => (
                canonical,
                self.auth.poll_browser(provider_alias, session_data).await,
            ),
            PendingLogin::DeviceCode {
                provider_alias,
                canonical,
                session,
            } => (
                canonical,
                self.auth.poll_device_code(provider_alias, session).await,
            ),
            PendingLogin::Pkce { .. } => {
                return Err(AuthError::Unsupported(
                    "PKCE login requires an authorization code".into(),
                ));
            }
        };

        let result = match result {
            Ok(result) => result,
            Err(error @ (AuthError::Network(_) | AuthError::RateLimited { .. })) => {
                return Err(error);
            }
            Err(error) => {
                claim.consume();
                return Err(error);
            }
        };
        match result {
            AuthPollResult::Pending => Ok(HostAuthPollResult::Pending),
            AuthPollResult::SlowDown { new_interval } => Ok(HostAuthPollResult::SlowDown {
                interval_secs: duration_secs(new_interval),
            }),
            AuthPollResult::Authorized { token: _ } => {
                let provider = canonical.clone();
                claim.consume();
                Ok(HostAuthPollResult::Authorized { provider })
            }
            AuthPollResult::Denied => {
                claim.consume();
                Ok(HostAuthPollResult::Denied)
            }
            AuthPollResult::Expired => {
                claim.consume();
                Ok(HostAuthPollResult::Expired)
            }
        }
    }

    /// Complete a PKCE login by opaque session id + authorization code.
    pub async fn complete_pkce(
        &self,
        session_id: &LoginSessionId,
        code: &str,
    ) -> Result<HostAuthCompletion, AuthError> {
        let claim = self.pending.claim(session_id)?;
        let (alias, canonical, state, session_data) = match claim.pending() {
            PendingLogin::Pkce {
                provider_alias,
                canonical,
                state,
                session_data,
                ..
            } => (provider_alias, canonical, state, session_data),
            PendingLogin::DeviceCode { .. } | PendingLogin::BrowserPoll { .. } => {
                return Err(AuthError::Unsupported("session is not a PKCE login".into()));
            }
        };

        match self
            .auth
            .complete_pkce(alias, code, state, session_data)
            .await
        {
            Ok(_) => {
                let provider = canonical.clone();
                claim.consume();
                Ok(HostAuthCompletion { provider })
            }
            Err(error @ (AuthError::Network(_) | AuthError::RateLimited { .. })) => Err(error),
            Err(error) => {
                claim.consume();
                Err(error)
            }
        }
    }

    /// Persist an API key and optional endpoint for a known provider.
    ///
    /// Validation happens before protected persistence. The config resolves the
    /// stored record only after `save` succeeds, so persistence failure leaves
    /// active explicit/environment configuration unchanged.
    pub fn configure_api_key(
        &self,
        provider: &str,
        api_key: ProviderApiKey,
        endpoint: Option<ProviderEndpoint>,
    ) -> Result<(), AuthError> {
        let canonical = self.resolve_canonical(provider)?;
        if api_key.expose_secret().trim().is_empty() {
            return Err(AuthError::InvalidResponse(format!(
                "provider '{canonical}' API key must not be empty"
            )));
        }
        let descriptor = self
            .descriptors
            .get(canonical.as_str())
            .ok_or_else(|| AuthError::UnknownProvider(provider.to_string()))?;
        if !descriptor
            .credential_flows
            .contains(&CredentialFlow::ApiKey)
        {
            return Err(AuthError::Unsupported(format!(
                "provider '{canonical}' does not support API-key configuration"
            )));
        }
        if endpoint.is_some() && !descriptor.endpoint_configurable {
            return Err(AuthError::Unsupported(format!(
                "provider '{canonical}' does not support a custom endpoint"
            )));
        }
        let store = self.config.provider_credential_store().ok_or_else(|| {
            AuthError::Unsupported("provider credential storage is disabled".into())
        })?;
        store
            .save(
                &canonical,
                &ProviderCredentialRecord::new(api_key, endpoint),
            )
            .map_err(|_| {
                AuthError::Io(format!(
                    "failed to persist protected credentials for provider '{canonical}'"
                ))
            })
    }

    /// Logout all Roci-owned API-key and OAuth credentials for a provider.
    ///
    /// External/in-process config is never mutated. Credential removal occurs
    /// first; if any OAuth backend logout fails, the prior provider record is
    /// restored best-effort so status does not falsely report full logout.
    pub fn logout(&self, provider: &str) -> Result<(), AuthError> {
        let canonical = self.resolve_canonical(provider)?;
        let credential_store = self.config.provider_credential_store();
        let previous_record = credential_store
            .map(|store| store.load(&canonical))
            .transpose()
            .map_err(|_| {
                AuthError::Io(format!(
                    "failed to access protected credentials for provider '{canonical}'"
                ))
            })?
            .flatten();

        if let Some(store) = credential_store {
            store.clear(&canonical).map_err(|_| {
                AuthError::Io(format!(
                    "failed to clear protected credentials for provider '{canonical}'"
                ))
            })?;
        }

        let mut oauth_failed = false;
        for backend in self.auth.backends() {
            if backend.canonical_provider_key() == canonical
                && backend.logout(self.auth.store()).is_err()
            {
                oauth_failed = true;
            }
        }
        if !oauth_failed {
            return Ok(());
        }

        if let (Some(store), Some(record)) = (credential_store, previous_record.as_ref()) {
            if store.save(&canonical, record).is_err() {
                return Err(AuthError::Io(format!(
                    "OAuth logout and protected credential rollback failed for provider '{canonical}'"
                )));
            }
        }
        Err(AuthError::Io(format!(
            "failed to clear OAuth credentials for provider '{canonical}'"
        )))
    }

    fn resolve_canonical(&self, provider: &str) -> Result<String, AuthError> {
        let normalized = provider.to_lowercase();
        if let Some(canonical) = self.key_index.get(&normalized) {
            return Ok(canonical.clone());
        }
        // Registry may know the key even if descriptor map used a different canonical.
        if self.registry.has_provider(&normalized) {
            if let Some(factory) = self.registry.factory(&normalized) {
                return Ok(factory.descriptor().canonical_key);
            }
            return Ok(normalized);
        }
        Err(AuthError::UnknownProvider(provider.to_string()))
    }

    fn configured_sources(
        &self,
        canonical: &str,
        has_token: bool,
    ) -> Result<Vec<ConfiguredSource>, AuthError> {
        let mut sources = Vec::new();
        if self.config.has_explicit_api_key(canonical) {
            sources.push(ConfiguredSource::ExternallyConfigured);
        }
        if let Some(store) = self.config.provider_credential_store() {
            if store
                .load(canonical)
                .map_err(|_| AuthError::Io("provider credential store could not be read".into()))?
                .is_some()
            {
                sources.push(ConfiguredSource::StoredApiKey);
            }
        }
        if has_token {
            sources.push(ConfiguredSource::OAuthToken);
        }
        Ok(sources)
    }

    fn oauth_token(&self, canonical: &str) -> Result<Option<super::Token>, AuthError> {
        for backend in self.auth.backends() {
            if backend.canonical_provider_key() == canonical {
                if let Some(token) = backend.get_status(self.auth.store())? {
                    return Ok(Some(token));
                }
            }
        }
        Ok(None)
    }

    fn auth_state(
        &self,
        sources: &[ConfiguredSource],
        token: Option<&super::Token>,
    ) -> ProviderAuthState {
        if sources.contains(&ConfiguredSource::StoredApiKey) {
            ProviderAuthState::SignedIn {
                label: "Signed in".into(),
            }
        } else if sources.contains(&ConfiguredSource::ExternallyConfigured) {
            ProviderAuthState::ExternallyConfigured
        } else if let Some(token) = token {
            if token.is_valid() {
                ProviderAuthState::SignedIn {
                    label: "Signed in".into(),
                }
            } else if token
                .refresh_token
                .as_ref()
                .is_some_and(|value| !value.is_empty())
            {
                ProviderAuthState::RefreshNeeded
            } else {
                ProviderAuthState::ReauthRequired
            }
        } else {
            ProviderAuthState::SignedOut
        }
    }
}

fn new_session_id() -> LoginSessionId {
    LoginSessionId::new(Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::backend::AuthBackend;
    use crate::auth::credential::{
        InMemoryProviderCredentialStore, ProviderCredentialStore, ProviderCredentialStoreError,
    };
    use crate::auth::descriptor::CredentialFlow;
    use crate::auth::device_code::DeviceCodeSession;
    use crate::auth::store::{FileTokenStore, TokenStore, TokenStoreConfig};
    use crate::auth::token::Token;
    use crate::error::RociError;
    use crate::provider::{ModelProvider, ProviderFactory};
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use tokio::sync::Notify;

    struct LaunchFactory {
        keys: &'static [&'static str],
        display: &'static str,
        flows: Vec<CredentialFlow>,
        endpoint: bool,
    }

    impl ProviderFactory for LaunchFactory {
        fn provider_keys(&self) -> &[&str] {
            self.keys
        }

        fn descriptor(&self) -> ProviderDescriptor {
            ProviderDescriptor::new(
                self.keys[0],
                self.display,
                self.flows.clone(),
                self.endpoint,
            )
        }

        fn create(
            &self,
            _config: &RociConfig,
            _provider_key: &str,
            _model_id: &str,
        ) -> Result<Box<dyn ModelProvider>, RociError> {
            unreachable!("manager tests do not create providers")
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CompletePkceFailure {
        None,
        NetworkOnce,
        InvalidResponseOnce,
    }

    #[derive(Debug, Clone, Copy)]
    enum DevicePollBehavior {
        Pending,
        SlowDown,
        Authorized,
        Denied,
        Expired,
        Network,
        RateLimited,
        InvalidResponse,
    }

    enum DevicePollGate {
        Open,
        BlockFirst {
            started: Arc<Notify>,
            release: Arc<Notify>,
        },
    }

    struct StubBackend {
        aliases: &'static [&'static str],
        canonical: &'static str,
        flow: CredentialFlow,
        store_key: &'static str,
        fail_logout: bool,
        complete_pkce_failure: Mutex<CompletePkceFailure>,
        complete_pkce_calls: Option<Arc<AtomicUsize>>,
        complete_pkce_started: Option<Arc<Notify>>,
        complete_pkce_release: Option<Arc<Notify>>,
        device_poll_behavior: Option<DevicePollBehavior>,
        device_poll_calls: Option<Arc<AtomicUsize>>,
        device_poll_started: Option<Arc<Notify>>,
        device_poll_release: Option<Arc<Notify>>,
    }

    impl StubBackend {
        async fn poll_result(&self) -> Result<AuthPollResult, AuthError> {
            let Some(behavior) = self.device_poll_behavior else {
                return Err(AuthError::Unsupported("not device code".into()));
            };
            let call = self
                .device_poll_calls
                .as_ref()
                .map(|calls| calls.fetch_add(1, Ordering::SeqCst));
            if call == Some(0) {
                if let Some(started) = &self.device_poll_started {
                    started.notify_one();
                }
                if let Some(release) = &self.device_poll_release {
                    release.notified().await;
                }
            }
            match behavior {
                DevicePollBehavior::Pending => Ok(AuthPollResult::Pending),
                DevicePollBehavior::SlowDown => Ok(AuthPollResult::SlowDown {
                    new_interval: std::time::Duration::from_secs(2),
                }),
                DevicePollBehavior::Authorized => Ok(AuthPollResult::Authorized {
                    token: sample_token(),
                }),
                DevicePollBehavior::Denied => Ok(AuthPollResult::Denied),
                DevicePollBehavior::Expired => Ok(AuthPollResult::Expired),
                DevicePollBehavior::Network => Err(AuthError::Network("retryable".into())),
                DevicePollBehavior::RateLimited => Err(AuthError::RateLimited {
                    retry_after_ms: Some(10),
                }),
                DevicePollBehavior::InvalidResponse => {
                    Err(AuthError::InvalidResponse("terminal".into()))
                }
            }
        }
    }

    #[async_trait]
    impl AuthBackend for StubBackend {
        fn import_credentials(
            &self,
            store: &Arc<dyn TokenStore>,
        ) -> Result<Option<Token>, AuthError> {
            let token = store.load("external-import", "default")?;
            if let Some(token) = &token {
                store.save(self.store_key, "default", token)?;
            }
            Ok(token)
        }

        fn aliases(&self) -> &[&str] {
            self.aliases
        }

        fn display_name(&self) -> &str {
            self.canonical
        }

        fn store_key(&self) -> &str {
            self.store_key
        }

        fn canonical_provider_key(&self) -> &str {
            self.canonical
        }

        fn oauth_flow(&self) -> CredentialFlow {
            self.flow
        }

        async fn start_login(&self, _store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
            if self.flow == CredentialFlow::BrowserPoll {
                return Ok(AuthStep::BrowserPoll {
                    authorization_url: "https://example.com/browser?challenge=public".into(),
                    interval: std::time::Duration::from_secs(1),
                    expires_at: Utc::now() + chrono::Duration::minutes(5),
                    session_data: serde_json::json!({ "verifier": "browser-secret" }),
                });
            }
            if self.flow == CredentialFlow::DeviceCode {
                return Ok(AuthStep::DeviceCode {
                    verification_url: "https://example.com/device".into(),
                    user_code: "TEST-CODE".into(),
                    interval: std::time::Duration::from_secs(1),
                    expires_at: Utc::now() + chrono::Duration::minutes(5),
                    session: DeviceCodeSession {
                        provider: self.canonical.into(),
                        verification_url: "https://example.com/device".into(),
                        user_code: "TEST-CODE".into(),
                        device_code: "device-secret".into(),
                        interval_secs: 1,
                        expires_at: Utc::now() + chrono::Duration::minutes(5),
                    },
                });
            }
            Ok(AuthStep::Pkce {
                authorize_url: "https://example.com/authorize".into(),
                state: "state-secret".into(),
                session_data: serde_json::json!({
                    "code_verifier": "verifier-secret",
                    "access_token": "should-not-leak",
                }),
            })
        }

        async fn poll_browser(
            &self,
            _store: &Arc<dyn TokenStore>,
            session_data: &serde_json::Value,
        ) -> Result<AuthPollResult, AuthError> {
            assert_eq!(session_data["verifier"], "browser-secret");
            self.poll_result().await
        }

        async fn poll_device_code(
            &self,
            _store: &Arc<dyn TokenStore>,
            _session: &DeviceCodeSession,
        ) -> Result<AuthPollResult, AuthError> {
            self.poll_result().await
        }

        async fn complete_pkce(
            &self,
            _store: &Arc<dyn TokenStore>,
            _code: &str,
            state: &str,
            session_data: &serde_json::Value,
        ) -> Result<Token, AuthError> {
            assert_eq!(state, "state-secret");
            assert_eq!(session_data["code_verifier"], "verifier-secret");
            let call = self
                .complete_pkce_calls
                .as_ref()
                .map(|calls| calls.fetch_add(1, Ordering::SeqCst));
            if call == Some(0) {
                if let Some(started) = &self.complete_pkce_started {
                    started.notify_one();
                }
                if let Some(release) = &self.complete_pkce_release {
                    release.notified().await;
                }
            }

            let failure = std::mem::replace(
                &mut *self.complete_pkce_failure.lock().unwrap(),
                CompletePkceFailure::None,
            );
            match failure {
                CompletePkceFailure::None => Ok(sample_token()),
                CompletePkceFailure::NetworkOnce => {
                    Err(AuthError::Network("retryable test failure".into()))
                }
                CompletePkceFailure::InvalidResponseOnce => {
                    Err(AuthError::InvalidResponse("terminal test failure".into()))
                }
            }
        }

        fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
            store.load(self.store_key, "default")
        }

        fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError> {
            if self.fail_logout {
                Err(AuthError::Io("backend detail must not escape".into()))
            } else {
                store.clear(self.store_key, "default")
            }
        }
    }

    fn sample_token() -> Token {
        Token {
            provider_metadata: None,
            access_token: "access-secret".into(),
            refresh_token: Some("refresh-secret".into()),
            id_token: Some("id-secret".into()),
            expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            last_refresh: None,
            scopes: None,
            account_id: Some("https://endpoint.example/account".into()),
        }
    }

    fn temp_store() -> (TempDir, Arc<dyn TokenStore>) {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().to_path_buf(),
        )));
        (dir, store)
    }

    #[test]
    fn explicit_import_returns_canonical_receipt_and_missing_is_an_error() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager(store.clone());
        assert!(manager
            .import_credentials("claude")
            .unwrap_err()
            .to_string()
            .contains("no existing credentials"));
        store
            .save("external-import", "default", &sample_token())
            .unwrap();
        let receipt = manager.import_credentials("claude").unwrap();
        assert_eq!(receipt.provider, "anthropic");
        assert!(!format!("{receipt:?}").contains("secret"));
        assert!(store.load("claude-code", "default").unwrap().is_some());
        assert!(matches!(
            manager.import_credentials("missing"),
            Err(AuthError::UnknownProvider(_))
        ));
    }

    struct FailingCredentialStore;

    impl ProviderCredentialStore for FailingCredentialStore {
        fn load(
            &self,
            _provider: &str,
        ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
            Ok(None)
        }

        fn save(
            &self,
            _provider: &str,
            _record: &ProviderCredentialRecord,
        ) -> Result<(), ProviderCredentialStoreError> {
            Err(ProviderCredentialStoreError::Unavailable)
        }

        fn clear(&self, _provider: &str) -> Result<(), ProviderCredentialStoreError> {
            Ok(())
        }
    }

    fn anthropic_manager(store: Arc<dyn TokenStore>) -> ProviderAuthManager {
        anthropic_manager_with(
            store,
            Arc::new(InMemoryProviderCredentialStore::default()),
            false,
            CompletePkceFailure::None,
        )
    }

    fn anthropic_manager_with(
        store: Arc<dyn TokenStore>,
        credential_store: Arc<dyn ProviderCredentialStore>,
        fail_logout: bool,
        complete_pkce_failure: CompletePkceFailure,
    ) -> ProviderAuthManager {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(LaunchFactory {
            keys: &["anthropic"],
            display: "Anthropic",
            flows: vec![CredentialFlow::ApiKey],
            endpoint: true,
        }));

        let mut auth = AuthService::new(store.clone());
        auth.register_backend(Arc::new(StubBackend {
            aliases: &["claude", "anthropic", "claude-code"],
            canonical: "anthropic",
            flow: CredentialFlow::Pkce,
            store_key: "claude-code",
            fail_logout,
            complete_pkce_failure: Mutex::new(complete_pkce_failure),
            complete_pkce_calls: None,
            complete_pkce_started: None,
            complete_pkce_release: None,
            device_poll_behavior: None,
            device_poll_calls: None,
            device_poll_started: None,
            device_poll_release: None,
        }));

        let config = RociConfig::new()
            .with_token_store(Some(store))
            .with_provider_credential_store(Some(credential_store));
        ProviderAuthManager::new(auth, registry, config).unwrap()
    }

    fn blocking_pkce_manager(
        calls: Arc<AtomicUsize>,
        started: Arc<Notify>,
        release: Arc<Notify>,
    ) -> (TempDir, Arc<ProviderAuthManager>) {
        let (dir, store) = temp_store();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(LaunchFactory {
            keys: &["anthropic"],
            display: "Anthropic",
            flows: vec![CredentialFlow::ApiKey],
            endpoint: true,
        }));
        let mut auth = AuthService::new(store.clone());
        auth.register_backend(Arc::new(StubBackend {
            aliases: &["anthropic"],
            canonical: "anthropic",
            flow: CredentialFlow::Pkce,
            store_key: "claude-code",
            fail_logout: false,
            complete_pkce_failure: Mutex::new(CompletePkceFailure::None),
            complete_pkce_calls: Some(calls),
            complete_pkce_started: Some(started),
            complete_pkce_release: Some(release),
            device_poll_behavior: None,
            device_poll_calls: None,
            device_poll_started: None,
            device_poll_release: None,
        }));
        let manager = ProviderAuthManager::new(
            auth,
            registry,
            RociConfig::new().with_token_store(Some(store)),
        )
        .unwrap();
        (dir, Arc::new(manager))
    }

    fn device_manager(
        behavior: DevicePollBehavior,
        calls: Arc<AtomicUsize>,
        gate: DevicePollGate,
    ) -> (TempDir, Arc<ProviderAuthManager>) {
        polling_manager(behavior, calls, gate, CredentialFlow::DeviceCode)
    }

    fn polling_manager(
        behavior: DevicePollBehavior,
        calls: Arc<AtomicUsize>,
        gate: DevicePollGate,
        flow: CredentialFlow,
    ) -> (TempDir, Arc<ProviderAuthManager>) {
        let (started, release) = match gate {
            DevicePollGate::Open => (None, None),
            DevicePollGate::BlockFirst { started, release } => (Some(started), Some(release)),
        };
        let keys: &'static [&'static str] = if flow == CredentialFlow::BrowserPoll {
            &["browser"]
        } else {
            &["device"]
        };
        let (dir, store) = temp_store();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(LaunchFactory {
            keys,
            display: "Device",
            flows: vec![CredentialFlow::ApiKey],
            endpoint: false,
        }));
        let mut auth = AuthService::new(store.clone());
        auth.register_backend(Arc::new(StubBackend {
            aliases: keys,
            canonical: keys[0],
            flow,
            store_key: keys[0],
            fail_logout: false,
            complete_pkce_failure: Mutex::new(CompletePkceFailure::None),
            complete_pkce_calls: None,
            complete_pkce_started: None,
            complete_pkce_release: None,
            device_poll_behavior: Some(behavior),
            device_poll_calls: Some(calls),
            device_poll_started: started,
            device_poll_release: release,
        }));
        let manager = ProviderAuthManager::new(
            auth,
            registry,
            RociConfig::new().with_token_store(Some(store)),
        )
        .unwrap();
        (dir, Arc::new(manager))
    }

    async fn start_device_session(manager: &ProviderAuthManager) -> LoginSessionId {
        match manager.start_login("device").await.unwrap() {
            HostAuthStep::DeviceCode { session_id, .. } => session_id,
            other => panic!("expected DeviceCode, got {other:?}"),
        }
    }

    #[test]
    fn rejects_auth_only_backend_without_launch_factory() {
        let (_dir, store) = temp_store();
        let registry = ProviderRegistry::new();
        let mut auth = AuthService::new(store.clone());
        auth.register_backend(Arc::new(StubBackend {
            aliases: &["orphan"],
            canonical: "orphan",
            flow: CredentialFlow::DeviceCode,
            store_key: "orphan",
            fail_logout: false,
            complete_pkce_failure: Mutex::new(CompletePkceFailure::None),
            complete_pkce_calls: None,
            complete_pkce_started: None,
            complete_pkce_release: None,
            device_poll_behavior: None,
            device_poll_calls: None,
            device_poll_started: None,
            device_poll_release: None,
        }));
        let config = RociConfig::new().with_token_store(Some(store));
        let result = ProviderAuthManager::new(auth, registry, config);
        match result {
            Err(AuthError::Unsupported(msg)) => {
                assert!(msg.contains("orphan"), "{msg}");
                assert!(msg.contains("no launch factory"), "{msg}");
            }
            Err(other) => panic!("expected Unsupported, got {other:?}"),
            Ok(_) => panic!("expected auth-only backend rejection"),
        }
    }

    #[test]
    fn anthropic_overlay_exposes_api_key_and_pkce() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager(store);
        let descriptor = manager.descriptor("anthropic").unwrap();
        assert_eq!(descriptor.canonical_key, "anthropic");
        assert_eq!(descriptor.display_name, "Anthropic");
        assert_eq!(
            descriptor.credential_flows,
            vec![CredentialFlow::ApiKey, CredentialFlow::Pkce]
        );
        assert!(descriptor.endpoint_configurable);

        // Alias resolves to same canonical descriptor.
        let via_alias = manager.descriptor("claude").unwrap();
        assert_eq!(via_alias.canonical_key, "anthropic");
    }

    #[test]
    fn unknown_provider_distinct_from_known_unconfigured() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager(store);

        let unknown = manager.status("not-a-provider").unwrap_err();
        assert!(matches!(unknown, AuthError::UnknownProvider(ref k) if k == "not-a-provider"));

        let status = manager.status("anthropic").unwrap();
        assert_eq!(status.auth_state, ProviderAuthState::SignedOut);
        assert!(status.configured_sources.is_empty());
        assert!(!status.launch_available);
    }

    #[test]
    fn configured_source_and_launch_availability_projection() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager(store.clone());

        manager
            .config()
            .set_api_key("anthropic", "sk-external".into());
        let external = manager.status("anthropic").unwrap();
        assert_eq!(
            external.configured_sources,
            vec![ConfiguredSource::ExternallyConfigured]
        );
        assert_eq!(external.auth_state, ProviderAuthState::ExternallyConfigured);
        assert!(external.launch_available);

        store
            .save("claude-code", "default", &sample_token())
            .unwrap();
        // New manager views same store+config through fresh status call.
        let both = manager.status("anthropic").unwrap();
        assert!(both
            .configured_sources
            .contains(&ConfiguredSource::ExternallyConfigured));
        assert!(both
            .configured_sources
            .contains(&ConfiguredSource::OAuthToken));
        assert_eq!(both.auth_state, ProviderAuthState::ExternallyConfigured);
        assert!(both.launch_available);
        // No secret leakage in status debug/serde.
        let debug = format!("{both:?}");
        assert!(!debug.contains("sk-external"));
        assert!(!debug.contains("access-secret"));
        let json = serde_json::to_string(&both).unwrap();
        assert!(!json.contains("sk-external"));
        assert!(!json.contains("access-secret"));
    }

    #[test]
    fn expired_oauth_token_is_not_reported_as_signed_in() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager(store.clone());
        let mut token = sample_token();
        token.expires_at = Some(Utc::now() - chrono::Duration::seconds(1));
        store.save("claude-code", "default", &token).unwrap();

        let status = manager.status("anthropic").unwrap();

        assert_eq!(
            status.configured_sources,
            vec![ConfiguredSource::OAuthToken]
        );
        assert_eq!(status.auth_state, ProviderAuthState::RefreshNeeded);
    }

    #[tokio::test]
    async fn start_login_returns_host_safe_pkce_step() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager(store);
        let step = manager.start_login("anthropic").await.unwrap();
        match &step {
            HostAuthStep::Pkce {
                authorization_url,
                session_id,
            } => {
                assert_eq!(authorization_url, "https://example.com/authorize");
                assert!(!session_id.as_str().is_empty());
            }
            other => panic!("expected Pkce, got {other:?}"),
        }
        let debug = format!("{step:?}");
        assert!(!debug.contains("verifier-secret"));
        assert!(!debug.contains("should-not-leak"));
        assert!(!debug.contains("state-secret"));
        let json = serde_json::to_string(&step).unwrap();
        assert!(!json.contains("verifier-secret"));
        assert!(!json.contains("should-not-leak"));
        assert!(!json.contains("state-secret"));
        assert!(!json.contains("session_data"));
    }

    #[test]
    fn configure_rejects_empty_api_key_without_persisting() {
        let (_dir, token_store) = temp_store();
        let credential_store = Arc::new(InMemoryProviderCredentialStore::default());
        let manager = anthropic_manager_with(
            token_store,
            credential_store.clone(),
            /*fail_logout*/ false,
            CompletePkceFailure::None,
        );

        let result = manager.configure_api_key(
            "anthropic",
            ProviderApiKey::new("  "),
            /*endpoint*/ None,
        );

        assert!(matches!(result, Err(AuthError::InvalidResponse(_))));
        assert_eq!(credential_store.load("anthropic").unwrap(), None);
    }

    #[test]
    fn configure_persistence_failure_leaves_active_config_unchanged() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager_with(
            store,
            Arc::new(FailingCredentialStore),
            /*fail_logout*/ false,
            CompletePkceFailure::None,
        );
        manager
            .config()
            .set_api_key("anthropic", "external-secret".into());

        let error = manager
            .configure_api_key(
                "anthropic",
                ProviderApiKey::new("new-secret"),
                Some(ProviderEndpoint::new("https://new.example")),
            )
            .unwrap_err();

        let credential = manager
            .config()
            .resolve_provider_credential("anthropic")
            .unwrap()
            .unwrap();
        let crate::auth::CredentialMaterial::ApiKey(key) = credential.material else {
            panic!("expected configured API key");
        };
        assert_eq!(key.expose_secret(), "external-secret");
        let rendered = error.to_string();
        assert!(!rendered.contains("external-secret"));
        assert!(!rendered.contains("new-secret"));
        assert!(!rendered.contains("new.example"));
    }

    #[test]
    fn configure_success_survives_new_config_and_manager() {
        let (_dir, token_store) = temp_store();
        let credential_store = Arc::new(InMemoryProviderCredentialStore::default());
        let manager = anthropic_manager_with(
            token_store.clone(),
            credential_store.clone(),
            /*fail_logout*/ false,
            CompletePkceFailure::None,
        );
        let expected = ProviderCredentialRecord::new(
            ProviderApiKey::new("stored-secret"),
            Some(ProviderEndpoint::new("https://stored.example")),
        );

        manager
            .configure_api_key(
                "claude",
                expected.api_key.clone(),
                expected.endpoint.clone(),
            )
            .unwrap();
        assert_eq!(credential_store.load("anthropic").unwrap(), Some(expected));
        drop(manager);

        let restarted = anthropic_manager_with(
            token_store,
            credential_store,
            /*fail_logout*/ false,
            CompletePkceFailure::None,
        );
        let credential = restarted
            .config()
            .resolve_provider_credential("anthropic")
            .unwrap()
            .unwrap();
        let crate::auth::CredentialMaterial::ApiKey(key) = credential.material else {
            panic!("expected configured API key");
        };
        assert_eq!(key.expose_secret(), "stored-secret");
        assert_eq!(
            credential.endpoint.unwrap().as_str(),
            "https://stored.example"
        );
        let status = restarted.status("anthropic").unwrap();
        assert_eq!(
            status.configured_sources,
            vec![ConfiguredSource::StoredApiKey]
        );
        assert!(matches!(
            status.auth_state,
            ProviderAuthState::SignedIn { .. }
        ));
        assert!(status.launch_available);
    }

    #[test]
    fn logout_clears_roci_owned_sources_and_preserves_external_config() {
        let (_dir, token_store) = temp_store();
        let credential_store = Arc::new(InMemoryProviderCredentialStore::default());
        let manager = anthropic_manager_with(
            token_store.clone(),
            credential_store.clone(),
            /*fail_logout*/ false,
            CompletePkceFailure::None,
        );
        manager
            .config()
            .set_api_key("anthropic", "external-secret".into());
        manager
            .configure_api_key("anthropic", ProviderApiKey::new("stored-secret"), None)
            .unwrap();
        token_store
            .save("claude-code", "default", &sample_token())
            .unwrap();

        manager.logout("anthropic").unwrap();

        assert_eq!(credential_store.load("anthropic").unwrap(), None);
        assert!(token_store
            .load("claude-code", "default")
            .unwrap()
            .is_none());
        let credential = manager
            .config()
            .resolve_provider_credential("anthropic")
            .unwrap()
            .unwrap();
        let crate::auth::CredentialMaterial::ApiKey(key) = credential.material else {
            panic!("expected configured API key");
        };
        assert_eq!(key.expose_secret(), "external-secret");
        let status = manager.status("anthropic").unwrap();
        assert_eq!(
            status.configured_sources,
            vec![ConfiguredSource::ExternallyConfigured]
        );
        assert_eq!(status.auth_state, ProviderAuthState::ExternallyConfigured);
        assert!(status.launch_available);
    }

    #[test]
    fn logout_rolls_back_provider_record_when_oauth_logout_fails() {
        let (_dir, token_store) = temp_store();
        let credential_store = Arc::new(InMemoryProviderCredentialStore::default());
        let manager = anthropic_manager_with(
            token_store.clone(),
            credential_store.clone(),
            /*fail_logout*/ true,
            CompletePkceFailure::None,
        );
        manager
            .configure_api_key("anthropic", ProviderApiKey::new("stored-secret"), None)
            .unwrap();
        token_store
            .save("claude-code", "default", &sample_token())
            .unwrap();

        let error = manager.logout("anthropic").unwrap_err();

        assert!(credential_store.load("anthropic").unwrap().is_some());
        assert!(token_store
            .load("claude-code", "default")
            .unwrap()
            .is_some());
        let rendered = error.to_string();
        assert!(!rendered.contains("backend detail"));
        assert!(!rendered.contains("stored-secret"));
        let status = manager.status("anthropic").unwrap();
        assert!(status
            .configured_sources
            .contains(&ConfiguredSource::StoredApiKey));
        assert!(status
            .configured_sources
            .contains(&ConfiguredSource::OAuthToken));
    }

    #[test]
    fn manager_uses_auth_service_token_store_for_launch_config() {
        let (_auth_dir, auth_store) = temp_store();
        let (_config_dir, config_store) = temp_store();
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(LaunchFactory {
            keys: &["anthropic"],
            display: "Anthropic",
            flows: vec![CredentialFlow::ApiKey],
            endpoint: true,
        }));
        let mut auth = AuthService::new(auth_store.clone());
        auth.register_backend(Arc::new(StubBackend {
            aliases: &["anthropic"],
            canonical: "anthropic",
            flow: CredentialFlow::Pkce,
            store_key: "claude-code",
            fail_logout: false,
            complete_pkce_failure: Mutex::new(CompletePkceFailure::None),
            complete_pkce_calls: None,
            complete_pkce_started: None,
            complete_pkce_release: None,
            device_poll_behavior: None,
            device_poll_calls: None,
            device_poll_started: None,
            device_poll_release: None,
        }));
        let config = RociConfig::new().with_token_store(Some(config_store));
        let manager = ProviderAuthManager::new(auth, registry, config).unwrap();
        auth_store
            .save("claude-code", "default", &sample_token())
            .unwrap();

        let status = manager.status("anthropic").unwrap();

        assert!(matches!(
            status.auth_state,
            ProviderAuthState::SignedIn { .. }
        ));
        assert!(status.launch_available);
        let credential = manager
            .config()
            .resolve_provider_credential("anthropic")
            .unwrap()
            .unwrap();
        let crate::auth::CredentialMaterial::OAuth(token) = credential.material else {
            panic!("expected OAuth credential");
        };
        assert_eq!(token.access_token, "access-secret");
    }

    #[tokio::test]
    async fn concurrent_pkce_completion_dispatches_backend_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (_dir, manager) =
            blocking_pkce_manager(calls.clone(), started.clone(), release.clone());
        let session_id = match manager.start_login("anthropic").await.unwrap() {
            HostAuthStep::Pkce { session_id, .. } => session_id,
            other => panic!("expected Pkce, got {other:?}"),
        };
        let first = tokio::spawn({
            let manager = manager.clone();
            let session_id = session_id.clone();
            async move { manager.complete_pkce(&session_id, "first-code").await }
        });
        started.notified().await;

        let second = manager.complete_pkce(&session_id, "second-code").await;
        release.notify_one();
        let first = first.await.unwrap();

        assert!(first.is_ok());
        assert!(matches!(
            second,
            Err(AuthError::InvalidResponse(ref message))
                if message == "login session is already in progress"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_pkce_completion_releases_claim_for_retry() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (_dir, manager) = blocking_pkce_manager(calls.clone(), started.clone(), release);
        let session_id = match manager.start_login("anthropic").await.unwrap() {
            HostAuthStep::Pkce { session_id, .. } => session_id,
            other => panic!("expected Pkce, got {other:?}"),
        };
        let first = tokio::spawn({
            let manager = manager.clone();
            let session_id = session_id.clone();
            async move { manager.complete_pkce(&session_id, "first-code").await }
        });
        started.notified().await;

        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert!(manager
            .complete_pkce(&session_id, "retry-code")
            .await
            .is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn concurrent_device_poll_dispatches_backend_once() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (_dir, manager) = device_manager(
            DevicePollBehavior::Pending,
            calls.clone(),
            DevicePollGate::BlockFirst {
                started: started.clone(),
                release: release.clone(),
            },
        );
        let session_id = start_device_session(&manager).await;
        let first = tokio::spawn({
            let manager = manager.clone();
            let session_id = session_id.clone();
            async move { manager.advance_login(&session_id).await }
        });
        started.notified().await;

        let second = manager.advance_login(&session_id).await;
        release.notify_one();
        let first = first.await.unwrap();

        assert!(matches!(first, Ok(HostAuthPollResult::Pending)));
        assert!(matches!(
            second,
            Err(AuthError::InvalidResponse(ref message))
                if message == "login session is already in progress"
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn device_poll_restores_retryable_and_consumes_terminal_sessions() {
        for behavior in [
            DevicePollBehavior::Pending,
            DevicePollBehavior::SlowDown,
            DevicePollBehavior::Network,
            DevicePollBehavior::RateLimited,
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let (_dir, manager) = device_manager(behavior, calls.clone(), DevicePollGate::Open);
            let session_id = start_device_session(&manager).await;

            let _ = manager.advance_login(&session_id).await;
            let second = manager.advance_login(&session_id).await;

            assert!(!matches!(
                second,
                Err(AuthError::InvalidResponse(ref message))
                    if message == "unknown or expired login session"
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }

        for behavior in [
            DevicePollBehavior::Authorized,
            DevicePollBehavior::Denied,
            DevicePollBehavior::Expired,
            DevicePollBehavior::InvalidResponse,
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let (_dir, manager) = device_manager(behavior, calls.clone(), DevicePollGate::Open);
            let session_id = start_device_session(&manager).await;

            let _ = manager.advance_login(&session_id).await;
            let second = manager.advance_login(&session_id).await;

            assert!(matches!(
                second,
                Err(AuthError::InvalidResponse(ref message))
                    if message == "unknown or expired login session"
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn retryable_pkce_error_retains_pending_session() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager_with(
            store,
            Arc::new(InMemoryProviderCredentialStore::default()),
            /*fail_logout*/ false,
            CompletePkceFailure::NetworkOnce,
        );
        let session_id = match manager.start_login("anthropic").await.unwrap() {
            HostAuthStep::Pkce { session_id, .. } => session_id,
            other => panic!("expected Pkce, got {other:?}"),
        };

        assert!(matches!(
            manager.complete_pkce(&session_id, "auth-code").await,
            Err(AuthError::Network(_))
        ));
        assert!(manager
            .complete_pkce(&session_id, "auth-code")
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn terminal_pkce_error_consumes_pending_session() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager_with(
            store,
            Arc::new(InMemoryProviderCredentialStore::default()),
            /*fail_logout*/ false,
            CompletePkceFailure::InvalidResponseOnce,
        );
        let session_id = match manager.start_login("anthropic").await.unwrap() {
            HostAuthStep::Pkce { session_id, .. } => session_id,
            other => panic!("expected Pkce, got {other:?}"),
        };

        let first_error = manager
            .complete_pkce(&session_id, "auth-code")
            .await
            .unwrap_err();
        let second_error = manager
            .complete_pkce(&session_id, "auth-code")
            .await
            .unwrap_err();

        assert!(
            matches!(first_error, AuthError::InvalidResponse(ref message) if message == "terminal test failure")
        );
        assert!(
            matches!(second_error, AuthError::InvalidResponse(ref message) if message == "unknown or expired login session")
        );
    }

    #[tokio::test]
    async fn complete_pkce_uses_pending_map_and_drops_secrets() {
        let (_dir, store) = temp_store();
        let manager = anthropic_manager(store);
        let step = manager.start_login("anthropic").await.unwrap();
        let session_id = match step {
            HostAuthStep::Pkce { session_id, .. } => session_id,
            other => panic!("expected Pkce, got {other:?}"),
        };
        assert!(matches!(
            manager.advance_login(&session_id).await,
            Err(AuthError::Unsupported(_))
        ));
        let done = manager
            .complete_pkce(&session_id, "auth-code")
            .await
            .unwrap();
        assert_eq!(done.provider, "anthropic");
        // Session consumed.
        let err = manager
            .complete_pkce(&session_id, "auth-code")
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidResponse(_)));
    }

    #[tokio::test]
    async fn browser_poll_keeps_secrets_inside_manager_and_consumes_success() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (_dir, manager) = polling_manager(
            DevicePollBehavior::Authorized,
            calls.clone(),
            DevicePollGate::Open,
            CredentialFlow::BrowserPoll,
        );
        let step = manager.start_login("browser").await.unwrap();
        for output in [format!("{step:?}"), serde_json::to_string(&step).unwrap()] {
            assert!(!output.contains("browser-secret"));
            assert!(!output.contains("session_data"));
            assert!(!output.contains("verifier"));
        }
        let HostAuthStep::BrowserPoll { session_id, .. } = step else {
            panic!("expected browser flow")
        };
        // Calling the wrong flow must not discard the pending session.
        assert!(matches!(
            manager.complete_pkce(&session_id, "code").await,
            Err(AuthError::Unsupported(_))
        ));
        let result = manager.advance_login(&session_id).await.unwrap();
        assert!(
            matches!(result, HostAuthPollResult::Authorized { ref provider } if provider == "browser")
        );
        assert!(!serde_json::to_string(&result)
            .unwrap()
            .contains("access_token"));
        assert!(manager.advance_login(&session_id).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cancelled_browser_poll_releases_exclusive_claim() {
        let calls = Arc::new(AtomicUsize::new(0));
        let started = Arc::new(Notify::new());
        let (_dir, manager) = polling_manager(
            DevicePollBehavior::Authorized,
            calls.clone(),
            DevicePollGate::BlockFirst {
                started: started.clone(),
                release: Arc::new(Notify::new()),
            },
            CredentialFlow::BrowserPoll,
        );
        let HostAuthStep::BrowserPoll { session_id, .. } =
            manager.start_login("browser").await.unwrap()
        else {
            panic!("expected browser flow")
        };
        let first = tokio::spawn({
            let manager = manager.clone();
            let session_id = session_id.clone();
            async move { manager.advance_login(&session_id).await }
        });
        started.notified().await;
        assert!(
            matches!(manager.advance_login(&session_id).await, Err(AuthError::InvalidResponse(ref message)) if message == "login session is already in progress")
        );
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert!(matches!(
            manager.advance_login(&session_id).await,
            Ok(HostAuthPollResult::Authorized { .. })
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn browser_poll_releases_retryable_results_and_consumes_terminal_results() {
        for (behavior, retryable) in [
            (DevicePollBehavior::Pending, true),
            (DevicePollBehavior::SlowDown, true),
            (DevicePollBehavior::Network, true),
            (DevicePollBehavior::RateLimited, true),
            (DevicePollBehavior::Denied, false),
            (DevicePollBehavior::Expired, false),
            (DevicePollBehavior::InvalidResponse, false),
        ] {
            let calls = Arc::new(AtomicUsize::new(0));
            let (_dir, manager) = polling_manager(
                behavior,
                calls.clone(),
                DevicePollGate::Open,
                CredentialFlow::BrowserPoll,
            );
            let HostAuthStep::BrowserPoll { session_id, .. } =
                manager.start_login("browser").await.unwrap()
            else {
                panic!("expected browser flow")
            };
            let _ = manager.advance_login(&session_id).await;
            let _ = manager.advance_login(&session_id).await;
            assert_eq!(calls.load(Ordering::SeqCst), if retryable { 2 } else { 1 });
        }
    }

    #[test]
    fn manager_rebinds_raw_or_previously_scoped_auth_store_to_selected_account() {
        for auth_account in ["default", "personal"] {
            let (_dir, raw) = temp_store();
            let mut default_token = sample_token();
            default_token.access_token = "default-token".into();
            let mut work_token = sample_token();
            work_token.access_token = "work-token".into();
            let mut personal_token = sample_token();
            personal_token.access_token = "personal-token".into();
            raw.save("claude-code", "default", &default_token).unwrap();
            raw.save("claude-code", "work", &work_token).unwrap();
            raw.save("claude-code", "personal", &personal_token)
                .unwrap();
            let mut original = anthropic_manager(raw.clone());
            let selected = original.config.clone().with_account("work").unwrap();
            let auth_store = if auth_account == "default" {
                raw.clone()
            } else {
                original
                    .config
                    .clone()
                    .with_account(auth_account)
                    .unwrap()
                    .token_store()
                    .unwrap()
                    .clone()
            };
            original.auth.set_store(auth_store);
            let manager =
                ProviderAuthManager::new_shared(original.auth, original.registry, selected)
                    .unwrap();
            assert_eq!(manager.config().account(), "work");
            assert!(Arc::ptr_eq(
                manager.auth.store(),
                manager.config().token_store().unwrap()
            ));
            assert_eq!(
                manager.auth.store().load("claude-code", "default").unwrap(),
                Some(work_token)
            );
            let credential = manager
                .config()
                .resolve_provider_credential("anthropic")
                .unwrap()
                .unwrap();
            let crate::auth::CredentialMaterial::OAuth(token) = credential.material else {
                panic!("expected selected account OAuth credential");
            };
            assert_eq!(token.access_token, "work-token");
            manager.logout("anthropic").unwrap();
            assert!(raw.load("claude-code", "work").unwrap().is_none());
            assert_eq!(
                raw.load("claude-code", "default").unwrap(),
                Some(default_token)
            );
            assert_eq!(
                raw.load("claude-code", "personal").unwrap(),
                Some(personal_token)
            );
        }
    }
}
