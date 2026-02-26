use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};

use super::device_code::{DeviceCodePoll, DeviceCodeSession};
use super::error::AuthError;
use super::providers::claude_code::{ClaudeCodeAuth, PkceSession};
use super::providers::github_copilot::GitHubCopilotAuth;
use super::providers::openai_codex::OpenAiCodexAuth;
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
        session: PkceSession,
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

/// Pure service facade for authentication flows.
///
/// All I/O decisions (printing, prompting, exit codes) belong to the caller.
/// `AuthService` only returns typed results and errors.
///
/// # Example
/// ```no_run
/// use std::sync::Arc;
/// use roci::auth::{FileTokenStore, TokenStoreConfig};
/// use roci::auth::service::AuthService;
///
/// let store = Arc::new(FileTokenStore::new(
///     TokenStoreConfig::new(std::path::PathBuf::from("/tmp")),
/// ));
/// let svc = AuthService::new(store);
/// ```
pub struct AuthService {
    store: Arc<dyn TokenStore>,
}

impl AuthService {
    pub fn new(store: Arc<dyn TokenStore>) -> Self {
        Self { store }
    }

    /// Begin a login flow for the given provider alias.
    ///
    /// Returns an [`AuthStep`] describing the next action the caller must take.
    /// For device-code providers, the caller should display the URL/code and
    /// start polling. For PKCE providers, the caller should open the URL and
    /// collect the authorization code.
    pub async fn start_login(&self, provider: &str) -> Result<AuthStep, AuthError> {
        match normalize_provider(provider)? {
            ProviderKind::GithubCopilot => {
                let auth = GitHubCopilotAuth::new(self.store.clone());
                let session = auth.start_device_code().await?;
                Ok(AuthStep::DeviceCode {
                    verification_url: session.verification_url.clone(),
                    user_code: session.user_code.clone(),
                    interval: Duration::from_secs(session.interval_secs),
                    expires_at: session.expires_at,
                    session,
                })
            }
            ProviderKind::OpenAiCodex => {
                let auth = OpenAiCodexAuth::new(self.store.clone());
                if let Ok(Some(token)) = auth.import_codex_auth_json(None) {
                    return Ok(AuthStep::Imported { token });
                }
                let session = auth.start_device_code().await?;
                Ok(AuthStep::DeviceCode {
                    verification_url: session.verification_url.clone(),
                    user_code: session.user_code.clone(),
                    interval: Duration::from_secs(session.interval_secs),
                    expires_at: session.expires_at,
                    session,
                })
            }
            ProviderKind::Claude => {
                let auth = ClaudeCodeAuth::new(self.store.clone());
                if let Ok(Some(token)) = auth.import_cli_credentials(None) {
                    return Ok(AuthStep::Imported { token });
                }
                let session = auth.start_auth()?;
                Ok(AuthStep::Pkce {
                    authorize_url: session.authorize_url.clone(),
                    state: session.state.clone(),
                    session,
                })
            }
        }
    }

    /// Poll a device-code session for authorization status.
    ///
    /// Maps the provider-specific [`DeviceCodePoll`] into the
    /// provider-agnostic [`AuthPollResult`].
    pub async fn poll_device_code(
        &self,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        let poll = match session.provider.as_str() {
            "github-copilot" => {
                let auth = GitHubCopilotAuth::new(self.store.clone());
                let result = auth.poll_device_code(session).await?;
                if let DeviceCodePoll::Authorized { .. } = &result {
                    // Attempt Copilot JWT exchange; store as github-copilot-api
                    if let Ok(copilot_token) = auth.exchange_copilot_token().await {
                        let api_token = Token {
                            access_token: copilot_token.token,
                            refresh_token: None,
                            id_token: None,
                            expires_at: Some(copilot_token.expires_at),
                            last_refresh: Some(Utc::now()),
                            scopes: None,
                            account_id: Some(copilot_token.base_url),
                        };
                        let _ = self.store.save("github-copilot-api", "default", &api_token);
                    }
                }
                result
            }
            "openai-codex" => {
                let auth = OpenAiCodexAuth::new(self.store.clone());
                auth.poll_device_code(session).await?
            }
            other => {
                return Err(AuthError::Unsupported(format!(
                    "device-code polling not supported for {other}"
                )));
            }
        };
        Ok(map_poll_result(poll))
    }

    /// Complete a PKCE authorization-code exchange for a provider.
    ///
    /// `code` is the authorization code (or `code#state`) pasted by the user.
    pub async fn complete_pkce(
        &self,
        session: &PkceSession,
        code: &str,
    ) -> Result<Token, AuthError> {
        let auth = ClaudeCodeAuth::new(self.store.clone());
        auth.exchange_code(session, code).await
    }

    /// Check the stored token status for a provider.
    ///
    /// Returns `Some(token)` if a token exists, `None` if not logged in.
    pub fn get_status(&self, provider: &str) -> Result<Option<Token>, AuthError> {
        let key = provider_store_key(provider);
        self.store.load(key, "default")
    }

    /// Remove stored credentials for a provider.
    pub fn logout(&self, provider: &str) -> Result<(), AuthError> {
        let key = provider_store_key(provider);
        self.store.clear(key, "default")
    }

    /// List all known provider keys and their stored tokens.
    pub fn all_statuses(&self) -> Vec<(&'static str, &'static str, Result<Option<Token>, AuthError>)> {
        PROVIDER_ENTRIES
            .iter()
            .map(|(display_name, store_key)| {
                let result = self.store.load(store_key, "default");
                (*display_name, *store_key, result)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Known provider entries: (display name, store key).
const PROVIDER_ENTRIES: &[(&str, &str)] = &[
    ("GitHub Copilot", "github-copilot"),
    ("Codex", "openai-codex"),
    ("Claude", "claude-code"),
];

#[derive(Debug, Clone, Copy)]
enum ProviderKind {
    GithubCopilot,
    OpenAiCodex,
    Claude,
}

fn normalize_provider(alias: &str) -> Result<ProviderKind, AuthError> {
    match alias {
        "copilot" | "github-copilot" | "github" => Ok(ProviderKind::GithubCopilot),
        "chatgpt" | "codex" => Ok(ProviderKind::OpenAiCodex),
        "claude" | "anthropic" => Ok(ProviderKind::Claude),
        other => Err(AuthError::Unsupported(format!(
            "unknown provider: {other} (supported: copilot, codex, claude)"
        ))),
    }
}

fn provider_store_key(alias: &str) -> &str {
    match alias {
        "copilot" | "github-copilot" | "github" => "github-copilot",
        "chatgpt" | "codex" | "openai-codex" => "openai-codex",
        "claude" | "anthropic" | "claude-code" => "claude-code",
        other => other,
    }
}

fn map_poll_result(poll: DeviceCodePoll) -> AuthPollResult {
    match poll {
        DeviceCodePoll::Pending { .. } => AuthPollResult::Pending,
        DeviceCodePoll::SlowDown { interval_secs } => AuthPollResult::SlowDown {
            new_interval: Duration::from_secs(interval_secs),
        },
        DeviceCodePoll::Authorized { token } => AuthPollResult::Authorized { token },
        DeviceCodePoll::AccessDenied => AuthPollResult::Denied,
        DeviceCodePoll::Expired => AuthPollResult::Expired,
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
    fn normalize_provider_resolves_copilot_aliases() {
        assert!(matches!(
            normalize_provider("copilot"),
            Ok(ProviderKind::GithubCopilot)
        ));
        assert!(matches!(
            normalize_provider("github-copilot"),
            Ok(ProviderKind::GithubCopilot)
        ));
        assert!(matches!(
            normalize_provider("github"),
            Ok(ProviderKind::GithubCopilot)
        ));
    }

    #[test]
    fn normalize_provider_resolves_codex_aliases() {
        assert!(matches!(
            normalize_provider("chatgpt"),
            Ok(ProviderKind::OpenAiCodex)
        ));
        assert!(matches!(
            normalize_provider("codex"),
            Ok(ProviderKind::OpenAiCodex)
        ));
    }

    #[test]
    fn normalize_provider_resolves_claude_aliases() {
        assert!(matches!(
            normalize_provider("claude"),
            Ok(ProviderKind::Claude)
        ));
        assert!(matches!(
            normalize_provider("anthropic"),
            Ok(ProviderKind::Claude)
        ));
    }

    #[test]
    fn normalize_provider_rejects_unknown_provider() {
        let result = normalize_provider("foobar");
        assert!(result.is_err());
        match result {
            Err(AuthError::Unsupported(msg)) => {
                assert!(msg.contains("foobar"));
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }

    #[test]
    fn get_status_returns_none_when_not_logged_in() {
        let (_dir, svc) = temp_service();
        let result = svc.get_status("copilot").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_status_returns_token_after_store_save() {
        let (dir, svc) = temp_service();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        store
            .save("github-copilot", "default", &sample_token())
            .unwrap();
        let result = svc.get_status("copilot").unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().access_token, "test-access-token");
    }

    #[test]
    fn logout_clears_stored_token() {
        let (dir, svc) = temp_service();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        store
            .save("github-copilot", "default", &sample_token())
            .unwrap();
        assert!(svc.get_status("copilot").unwrap().is_some());
        svc.logout("copilot").unwrap();
        assert!(svc.get_status("copilot").unwrap().is_none());
    }

    #[test]
    fn logout_succeeds_when_already_logged_out() {
        let (_dir, svc) = temp_service();
        // Should not error on missing token
        svc.logout("copilot").unwrap();
    }

    #[test]
    fn all_statuses_lists_all_providers() {
        let (_dir, svc) = temp_service();
        let statuses = svc.all_statuses();
        assert_eq!(statuses.len(), 3);
        let names: Vec<&str> = statuses.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(names, vec!["GitHub Copilot", "Codex", "Claude"]);
    }

    #[test]
    fn all_statuses_shows_saved_token() {
        let (dir, svc) = temp_service();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        store
            .save("openai-codex", "default", &sample_token())
            .unwrap();
        let statuses = svc.all_statuses();
        let codex_entry = statuses
            .iter()
            .find(|(name, _, _)| *name == "Codex")
            .unwrap();
        assert!(codex_entry.2.as_ref().unwrap().is_some());
    }

    #[test]
    fn map_poll_result_maps_pending() {
        let result = map_poll_result(DeviceCodePoll::Pending { interval_secs: 5 });
        assert!(matches!(result, AuthPollResult::Pending));
    }

    #[test]
    fn map_poll_result_maps_slow_down() {
        let result = map_poll_result(DeviceCodePoll::SlowDown { interval_secs: 10 });
        match result {
            AuthPollResult::SlowDown { new_interval } => {
                assert_eq!(new_interval, Duration::from_secs(10));
            }
            other => panic!("expected SlowDown, got {other:?}"),
        }
    }

    #[test]
    fn map_poll_result_maps_authorized() {
        let token = sample_token();
        let result = map_poll_result(DeviceCodePoll::Authorized {
            token: token.clone(),
        });
        match result {
            AuthPollResult::Authorized { token: t } => {
                assert_eq!(t.access_token, token.access_token);
            }
            other => panic!("expected Authorized, got {other:?}"),
        }
    }

    #[test]
    fn map_poll_result_maps_access_denied() {
        let result = map_poll_result(DeviceCodePoll::AccessDenied);
        assert!(matches!(result, AuthPollResult::Denied));
    }

    #[test]
    fn map_poll_result_maps_expired() {
        let result = map_poll_result(DeviceCodePoll::Expired);
        assert!(matches!(result, AuthPollResult::Expired));
    }

    #[test]
    fn provider_store_key_maps_aliases_correctly() {
        assert_eq!(provider_store_key("copilot"), "github-copilot");
        assert_eq!(provider_store_key("github"), "github-copilot");
        assert_eq!(provider_store_key("chatgpt"), "openai-codex");
        assert_eq!(provider_store_key("codex"), "openai-codex");
        assert_eq!(provider_store_key("claude"), "claude-code");
        assert_eq!(provider_store_key("anthropic"), "claude-code");
    }

    #[test]
    fn provider_store_key_passes_through_direct_keys() {
        assert_eq!(provider_store_key("github-copilot"), "github-copilot");
        assert_eq!(provider_store_key("openai-codex"), "openai-codex");
        assert_eq!(provider_store_key("claude-code"), "claude-code");
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
