//! Secret-free provider auth status projection for hosts.

use serde::{Deserialize, Serialize};

use super::descriptor::ProviderDescriptor;

/// Where credentials currently come from (values never included).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfiguredSource {
    /// In-process or environment configuration (not Roci-owned persistence).
    ExternallyConfigured,
    /// OAuth token present in the token store.
    OAuthToken,
}

/// Secret-free auth state for a canonical provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderAuthState {
    /// No known credentials for this provider.
    SignedOut,
    /// Only external/env/in-process configuration is present.
    ExternallyConfigured,
    /// Roci-owned OAuth (or equivalent) session is present.
    ///
    /// `label` is generic non-secret text (never derived from tokens).
    SignedIn { label: String },
}

/// Host-safe status row for one canonical provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAuthStatus {
    /// Merged factory + OAuth descriptor.
    pub descriptor: ProviderDescriptor,
    /// Secret-free auth state.
    pub auth_state: ProviderAuthState,
    /// Configured credential sources (no secret values).
    pub configured_sources: Vec<ConfiguredSource>,
    /// Whether the launch factory reports availability for the current config.
    pub launch_available: bool,
}
