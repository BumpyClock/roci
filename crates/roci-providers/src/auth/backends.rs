//! AuthBackend implementations for built-in OAuth providers.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;

use roci_core::auth::{
    AuthBackend, AuthError, AuthPollResult, AuthStep, DeviceCodePoll, DeviceCodeSession, Token,
    TokenStore,
};

use super::claude_code::{ClaudeCodeAuth, PkceSession};
use super::github_copilot::GitHubCopilotAuth;
use super::openai_codex::OpenAiCodexAuth;

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

        // Attempt Copilot JWT exchange on success; store as github-copilot-api
        if let DeviceCodePoll::Authorized { .. } = &result {
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
                let _ = store.save("github-copilot-api", "default", &api_token);
            }
        }

        Ok(map_poll_result(result))
    }

    async fn complete_pkce(
        &self,
        _store: &Arc<dyn TokenStore>,
        _code: &str,
        _state: &str,
    ) -> Result<Token, AuthError> {
        Err(AuthError::Unsupported(
            "GitHub Copilot uses device-code flow, not PKCE".into(),
        ))
    }

    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        store.load(self.store_key(), "default")
    }

    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError> {
        store.clear(self.store_key(), "default")
    }
}

// ---------------------------------------------------------------------------
// OpenAI Codex
// ---------------------------------------------------------------------------

pub struct OpenAiCodexBackend;

#[async_trait]
impl AuthBackend for OpenAiCodexBackend {
    fn aliases(&self) -> &[&str] {
        &["chatgpt", "codex", "openai-codex"]
    }

    fn display_name(&self) -> &str {
        "Codex"
    }

    fn store_key(&self) -> &str {
        "openai-codex"
    }

    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
        let auth = OpenAiCodexAuth::new(store.clone());
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

    async fn poll_device_code(
        &self,
        store: &Arc<dyn TokenStore>,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        let auth = OpenAiCodexAuth::new(store.clone());
        let result = auth.poll_device_code(session).await?;
        Ok(map_poll_result(result))
    }

    async fn complete_pkce(
        &self,
        _store: &Arc<dyn TokenStore>,
        _code: &str,
        _state: &str,
    ) -> Result<Token, AuthError> {
        Err(AuthError::Unsupported(
            "OpenAI Codex uses device-code flow, not PKCE".into(),
        ))
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

    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
        let auth = ClaudeCodeAuth::new(store.clone());
        if let Ok(Some(token)) = auth.import_cli_credentials(None) {
            return Ok(AuthStep::Imported { token });
        }
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
        state: &str,
    ) -> Result<Token, AuthError> {
        let session = pkce_session_from_state(state)?;
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

fn pkce_session_to_json(session: &PkceSession) -> serde_json::Value {
    serde_json::json!({
        "authorize_url": session.authorize_url,
        "state": session.state,
        "code_verifier": session.code_verifier,
    })
}

fn pkce_session_from_state(state: &str) -> Result<PkceSession, AuthError> {
    // The caller provides the state string; we reconstruct a minimal session.
    // In practice, the complete session data (including code_verifier) must be
    // preserved by the caller between start_login and complete_pkce.
    // This is a fallback that works when the session_data JSON is passed via state.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(state) {
        let authorize_url = v
            .get("authorize_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let st = v
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let code_verifier = v
            .get("code_verifier")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        return Ok(PkceSession {
            authorize_url,
            state: st,
            code_verifier,
        });
    }
    Err(AuthError::InvalidResponse(
        "Failed to reconstruct PKCE session from state".into(),
    ))
}
