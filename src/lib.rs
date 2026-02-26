//! Roci -- batteries-included AI agent SDK.
//!
//! Re-exports `roci-core` (provider-agnostic abstractions) and `roci-providers`
//! (built-in transports + OAuth flows) with default wiring.
//!
//! For explicit control, depend on `roci-core` + `roci-providers` directly.

pub use roci_core::*;
pub use roci_providers;

use std::sync::Arc;

/// Create a default provider registry with all enabled built-in providers.
pub fn default_registry() -> roci_core::provider::ProviderRegistry {
    let mut registry = roci_core::provider::ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);
    registry
}

/// Create a default AuthService with all built-in auth backends registered.
pub fn default_auth_service(
    store: Arc<dyn roci_core::auth::TokenStore>,
) -> roci_core::auth::AuthService {
    let mut svc = roci_core::auth::AuthService::new(store);
    roci_providers::register_default_auth_backends(&mut svc);
    svc
}
