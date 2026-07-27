//! Multi-flow provider descriptors for host-facing auth surfaces.
//!
//! Factories own the base descriptor (transport credential modes). OAuth backends
//! overlay additional flows onto the same canonical provider key inside
//! [`super::manager::ProviderAuthManager`].

use serde::{Deserialize, Serialize};

/// How a host can supply credentials for a provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialFlow {
    /// Local/no-credential launch (e.g. Ollama, LM Studio).
    Local,
    /// API key (and optional endpoint) configuration.
    ApiKey,
    /// OAuth device-code flow.
    DeviceCode,
    /// OAuth PKCE authorization-code flow.
    Pkce,
}

/// Secret-free provider descriptor shared by factories and the auth manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    /// Canonical provider key used for launch and status (e.g. `"anthropic"`).
    pub canonical_key: String,
    /// Human-readable display name.
    pub display_name: String,
    /// Supported credential flows for this provider.
    pub credential_flows: Vec<CredentialFlow>,
    /// Whether an endpoint/base URL may be configured.
    pub endpoint_configurable: bool,
}

impl ProviderDescriptor {
    /// Build a descriptor with the given fields.
    pub fn new(
        canonical_key: impl Into<String>,
        display_name: impl Into<String>,
        credential_flows: Vec<CredentialFlow>,
        endpoint_configurable: bool,
    ) -> Self {
        Self {
            canonical_key: canonical_key.into(),
            display_name: display_name.into(),
            credential_flows,
            endpoint_configurable,
        }
    }

    /// Default third-party factory descriptor from a provider key.
    ///
    /// Uses the key as display name material, API-key flow when credentials are
    /// required, otherwise local, and allows endpoint configuration.
    pub fn third_party_default(provider_key: &str, requires_credentials: bool) -> Self {
        let flow = if requires_credentials {
            CredentialFlow::ApiKey
        } else {
            CredentialFlow::Local
        };
        Self {
            canonical_key: provider_key.to_string(),
            display_name: humanize_provider_key(provider_key),
            credential_flows: vec![flow],
            endpoint_configurable: true,
        }
    }

    /// Insert `flow` if not already present (stable order: existing then new).
    pub fn with_flow(mut self, flow: CredentialFlow) -> Self {
        if !self.credential_flows.contains(&flow) {
            self.credential_flows.push(flow);
        }
        self
    }
}

fn humanize_provider_key(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut capitalize = true;
    for ch in key.chars() {
        match ch {
            '-' | '_' => {
                out.push(' ');
                capitalize = true;
            }
            c if capitalize => {
                out.extend(c.to_uppercase());
                capitalize = false;
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn third_party_default_uses_api_key_when_credentials_required() {
        let d = ProviderDescriptor::third_party_default("acme-llm", true);
        assert_eq!(d.canonical_key, "acme-llm");
        assert_eq!(d.display_name, "Acme Llm");
        assert_eq!(d.credential_flows, vec![CredentialFlow::ApiKey]);
        assert!(d.endpoint_configurable);
    }

    #[test]
    fn third_party_default_uses_local_when_no_credentials() {
        let d = ProviderDescriptor::third_party_default("local-box", false);
        assert_eq!(d.credential_flows, vec![CredentialFlow::Local]);
    }

    #[test]
    fn with_flow_dedupes() {
        let d =
            ProviderDescriptor::new("anthropic", "Anthropic", vec![CredentialFlow::ApiKey], true)
                .with_flow(CredentialFlow::Pkce)
                .with_flow(CredentialFlow::ApiKey);
        assert_eq!(
            d.credential_flows,
            vec![CredentialFlow::ApiKey, CredentialFlow::Pkce]
        );
    }
}
