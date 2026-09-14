//! Registerable authentication backend trait.

use std::sync::Arc;

use async_trait::async_trait;

use super::descriptor::CredentialFlow;
use super::device_code::DeviceCodeSession;
use super::error::AuthError;
use super::service::{AuthPollResult, AuthStep};
use super::store::TokenStore;
use super::token::Token;

/// Registerable authentication backend for a provider.
///
/// Role: own one OAuth flow (device-code or PKCE) and the token-store key for a
/// canonical launch provider. Implement for each OAuth integration (GitHub
/// Copilot, OpenAI Codex, Claude Code, etc.) and register with
/// [`super::AuthService`]. The host-facing [`super::manager::ProviderAuthManager`]
/// overlays [`oauth_flow`] onto the matching factory descriptor.
#[async_trait]
pub trait AuthBackend: Send + Sync {
    /// Provider aliases this backend handles (e.g., ["copilot", "github-copilot"]).
    fn aliases(&self) -> &[&str];

    /// Display name for UI purposes (e.g., "GitHub Copilot").
    fn display_name(&self) -> &str;

    /// Token store key (e.g., "github-copilot").
    fn store_key(&self) -> &str;

    /// Credential whose refresh lease also protects logout. Derived credentials
    /// may use a different key from the primary login token.
    fn runtime_store_key(&self) -> &str {
        self.store_key()
    }

    /// Explicitly copy external credentials into Roci storage. `None` means no
    /// importable credential was found. Login never invokes this hook.
    fn import_credentials(&self, _store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        Err(AuthError::Unsupported(format!(
            "{} does not support credential import",
            self.display_name()
        )))
    }

    /// Canonical launch provider key this OAuth flow attaches to.
    ///
    /// Must match a registered [`crate::provider::ProviderFactory`] key so the
    /// manager can overlay flows and reject auth-only backends.
    fn canonical_provider_key(&self) -> &str;

    /// OAuth credential flow contributed by this backend.
    fn oauth_flow(&self) -> CredentialFlow;

    /// Supported login flows, with the default first.
    fn oauth_flows(&self) -> Vec<CredentialFlow> {
        vec![self.oauth_flow()]
    }

    async fn start_login_with_flow(
        &self,
        store: &Arc<dyn TokenStore>,
        flow: CredentialFlow,
    ) -> Result<AuthStep, AuthError> {
        if flow != self.oauth_flow() {
            return Err(AuthError::Unsupported(format!(
                "{} does not support the selected login flow",
                self.display_name()
            )));
        }
        self.start_login(store).await
    }

    /// Start a login flow.
    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError>;

    /// Poll browser authorization using manager-owned secret session material.
    async fn poll_browser(
        &self,
        _store: &Arc<dyn TokenStore>,
        _session_data: &serde_json::Value,
    ) -> Result<AuthPollResult, AuthError> {
        Err(AuthError::Unsupported(
            "browser polling is not supported".into(),
        ))
    }

    /// Poll a device-code session.
    async fn poll_device_code(
        &self,
        store: &Arc<dyn TokenStore>,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError>;

    /// Complete a PKCE flow using the opaque session data from `start_login`.
    ///
    /// The caller must preserve this material (including the code verifier)
    /// until authorization completes. The host-facing manager owns it on behalf
    /// of callers using an opaque login session ID.
    async fn complete_pkce(
        &self,
        store: &Arc<dyn TokenStore>,
        code: &str,
        state: &str,
        session_data: &serde_json::Value,
    ) -> Result<Token, AuthError>;

    /// Get current auth status for this backend.
    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError>;

    /// Remove stored tokens for this backend.
    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError>;
}
