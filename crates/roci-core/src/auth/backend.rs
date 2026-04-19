//! Registerable authentication backend trait.

use std::sync::Arc;

use async_trait::async_trait;

use super::device_code::DeviceCodeSession;
use super::error::AuthError;
use super::service::{AuthPollResult, AuthStep};
use super::store::TokenStore;
use super::token::Token;

/// Registerable authentication backend for a provider.
///
/// Implement this trait for each OAuth provider (GitHub Copilot, OpenAI Codex,
/// Claude Code, etc.) and register it with [`super::AuthService`].
#[async_trait]
pub trait AuthBackend: Send + Sync {
    /// Provider aliases this backend handles (e.g., ["copilot", "github-copilot"]).
    fn aliases(&self) -> &[&str];

    /// Display name for UI purposes (e.g., "GitHub Copilot").
    fn display_name(&self) -> &str;

    /// Token store key (e.g., "github-copilot").
    fn store_key(&self) -> &str;

    /// Start a login flow.
    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError>;

    /// Poll a device-code session.
    async fn poll_device_code(
        &self,
        store: &Arc<dyn TokenStore>,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError>;

    /// Complete a PKCE flow with the authorization code.
    async fn complete_pkce(
        &self,
        store: &Arc<dyn TokenStore>,
        code: &str,
        state: &str,
    ) -> Result<Token, AuthError>;

    /// Complete a PKCE flow with preserved session data.
    ///
    /// Backends that require opaque session state (e.g. a PKCE `code_verifier`)
    /// should override this method. The default delegates to [`complete_pkce`]
    /// and ignores `session_data`, so existing implementers are unaffected.
    async fn complete_pkce_with_session(
        &self,
        store: &Arc<dyn TokenStore>,
        code: &str,
        state: &str,
        session_data: Option<&serde_json::Value>,
    ) -> Result<Token, AuthError> {
        let _ = session_data;
        self.complete_pkce(store, code, state).await
    }

    /// Get current auth status for this backend.
    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError>;

    /// Remove stored tokens for this backend.
    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError>;
}
