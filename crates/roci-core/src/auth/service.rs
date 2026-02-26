//! Generic auth service orchestrator using registered backends.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::backend::AuthBackend;
use super::device_code::DeviceCodeSession;
use super::error::AuthError;
use super::store::TokenStore;
use super::token::Token;

/// Initial step returned by [`AuthService::start_login`].
///
/// Describes what the caller must do next (display a URL + code, or redirect
/// the user to a PKCE authorize URL).
#[derive(Debug, Clone)]
pub enum AuthStep {
    /// Device-code flow: show the URL and user code, then poll.
    DeviceCode {
        verification_url: String,
        user_code: String,
        interval: Duration,
        expires_at: DateTime<Utc>,
        session: DeviceCodeSession,
    },
    /// PKCE authorization-code flow: open the URL, collect the response code.
    Pkce {
        authorize_url: String,
        state: String,
        /// Opaque session data needed by `complete_pkce`. Callers should store
        /// this and pass it back when the user supplies the authorization code.
        session_data: serde_json::Value,
    },
    /// Credentials were imported from an existing file; no user interaction needed.
    Imported { token: Token },
}

/// Outcome of a single poll attempt during a device-code flow.
#[derive(Debug, Clone)]
pub enum AuthPollResult {
    /// Authorization still pending; keep polling.
    Pending,
    /// Server asked to slow down; use `new_interval`.
    SlowDown { new_interval: Duration },
    /// User authorized; token is ready.
    Authorized { token: Token },
    /// User denied the request.
    Denied,
    /// The device code expired before the user authorized.
    Expired,
}

/// Generic authentication orchestrator using registered backends.
///
/// `AuthService` is provider-agnostic. Concrete OAuth implementations are
/// registered as [`AuthBackend`]s; dispatching by provider alias replaces
/// the hardcoded `ProviderKind` enum.
pub struct AuthService {
    store: Arc<dyn TokenStore>,
    backends: Vec<Arc<dyn AuthBackend>>,
}

impl AuthService {
    pub fn new(store: Arc<dyn TokenStore>) -> Self {
        Self {
            store,
            backends: Vec::new(),
        }
    }

    /// Register an authentication backend.
    pub fn register_backend(&mut self, backend: Arc<dyn AuthBackend>) {
        self.backends.push(backend);
    }

    /// Begin a login flow for the given provider alias.
    pub async fn start_login(&self, provider: &str) -> Result<AuthStep, AuthError> {
        let backend = self.find_backend(provider)?;
        backend.start_login(&self.store).await
    }

    /// Poll a device-code session for authorization status.
    pub async fn poll_device_code(
        &self,
        provider: &str,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        let backend = self.find_backend(provider)?;
        backend.poll_device_code(&self.store, session).await
    }

    /// Complete a PKCE authorization-code exchange.
    pub async fn complete_pkce(
        &self,
        provider: &str,
        code: &str,
        state: &str,
    ) -> Result<Token, AuthError> {
        let backend = self.find_backend(provider)?;
        backend.complete_pkce(&self.store, code, state).await
    }

    /// Check the stored token status for a provider.
    pub fn get_status(&self, provider: &str) -> Result<Option<Token>, AuthError> {
        match self.find_backend(provider) {
            Ok(backend) => backend.get_status(&self.store),
            // If no backend is registered, try a direct store lookup.
            Err(_) => {
                let key = provider;
                self.store.load(key, "default")
            }
        }
    }

    /// Remove stored credentials for a provider.
    pub fn logout(&self, provider: &str) -> Result<(), AuthError> {
        match self.find_backend(provider) {
            Ok(backend) => backend.logout(&self.store),
            Err(_) => self.store.clear(provider, "default"),
        }
    }

    /// List all registered backends and their stored tokens.
    pub fn all_statuses(&self) -> Vec<(&str, &str, Result<Option<Token>, AuthError>)> {
        self.backends
            .iter()
            .map(|b| {
                let result = b.get_status(&self.store);
                (b.display_name(), b.store_key(), result)
            })
            .collect()
    }

    /// Access the underlying token store.
    pub fn store(&self) -> &Arc<dyn TokenStore> {
        &self.store
    }

    fn find_backend(&self, alias: &str) -> Result<&Arc<dyn AuthBackend>, AuthError> {
        let normalized = alias.to_lowercase();
        self.backends
            .iter()
            .find(|b| b.aliases().iter().any(|a| *a == normalized))
            .ok_or_else(|| {
                let known: Vec<&str> = self
                    .backends
                    .iter()
                    .flat_map(|b| b.aliases().iter().copied())
                    .collect();
                AuthError::Unsupported(format!(
                    "unknown provider: {alias} (registered: {})",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::store::{FileTokenStore, TokenStoreConfig};
    use tempfile::TempDir;

    fn temp_service() -> (TempDir, AuthService) {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().to_path_buf(),
        )));
        let svc = AuthService::new(store);
        (dir, svc)
    }

    fn sample_token() -> Token {
        Token {
            access_token: "test-access-token".to_string(),
            refresh_token: Some("test-refresh".to_string()),
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: None,
            account_id: None,
        }
    }

    #[test]
    fn get_status_falls_back_to_direct_store_lookup() {
        let (dir, svc) = temp_service();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        store
            .save("github-copilot", "default", &sample_token())
            .unwrap();
        // No backends registered, but direct store lookup works
        let result = svc.get_status("github-copilot").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().access_token, "test-access-token");
    }

    #[test]
    fn logout_falls_back_to_direct_store_clear() {
        let (dir, svc) = temp_service();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        store
            .save("github-copilot", "default", &sample_token())
            .unwrap();
        svc.logout("github-copilot").unwrap();
        let result = svc.get_status("github-copilot").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn logout_succeeds_when_already_logged_out() {
        let (_dir, svc) = temp_service();
        svc.logout("copilot").unwrap();
    }

    #[test]
    fn all_statuses_empty_when_no_backends() {
        let (_dir, svc) = temp_service();
        let statuses = svc.all_statuses();
        assert!(statuses.is_empty());
    }

    #[tokio::test]
    async fn start_login_rejects_unknown_provider() {
        let (_dir, svc) = temp_service();
        let result = svc.start_login("unknown-provider").await;
        assert!(result.is_err());
        match result {
            Err(AuthError::Unsupported(msg)) => {
                assert!(msg.contains("unknown-provider"));
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}
