//! Provider factory trait for creating ModelProvider instances.

use super::ModelProvider;
use crate::auth::ProviderDescriptor;
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::{ModelCatalog, ModelListOptions};
use futures::future::BoxFuture;

/// Factory for creating ModelProvider instances from a provider key + model ID.
///
/// Role: own launch-time construction (`create`), catalog discovery
/// (`list_models`), availability checks (`is_available`), and the base
/// host-facing [`descriptor`] for transport credential modes. Registries and
/// hosts must consult the factory instead of re-implementing credential
/// heuristics so alias keys, OAuth token-store entries, and required endpoints
/// stay provider-owned. OAuth backends overlay additional flows in
/// [`crate::auth::ProviderAuthManager`].
pub trait ProviderFactory: Send + Sync {
    /// Provider key(s) this factory handles (e.g., &["openai", "codex"]).
    fn provider_keys(&self) -> &[&str];

    /// Host-safe base descriptor for this factory's canonical provider.
    ///
    /// Default is third-party safe: first provider key, humanized display name,
    /// API-key or local flow from [`requires_credentials`], endpoint allowed.
    /// Built-in factories override with explicit drift-tested descriptors.
    fn descriptor(&self) -> ProviderDescriptor {
        let key = self.provider_keys().first().copied().unwrap_or("unknown");
        ProviderDescriptor::third_party_default(key, self.requires_credentials(key))
    }

    /// Whether this provider key needs credentials before launch-time use.
    fn requires_credentials(&self, _provider_key: &str) -> bool {
        true
    }

    /// Whether this factory can launch for `provider_key` with the given config.
    ///
    /// Default treats a factory as available when it does not require
    /// credentials, or when `config` already has credentials for the provider
    /// key. Override when create viability depends on alias keys, OAuth
    /// token-store entries, required endpoints, or local no-credential launch.
    /// Must stay hermetic: no network I/O.
    fn is_available(&self, config: &RociConfig, provider_key: &str) -> bool {
        !self.requires_credentials(provider_key) || config.has_credentials(provider_key)
    }

    /// List models for the given provider key.
    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_unavailable && !self.is_available(config, provider_key) {
                return Err(RociError::MissingCredential {
                    provider: provider_key.to_string(),
                });
            }
            Ok(ModelCatalog::default())
        })
    }

    /// Create a ModelProvider for the given model ID and config.
    fn create(
        &self,
        config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::CredentialFlow;

    struct DefaultFactory {
        requires_credentials: bool,
    }

    impl ProviderFactory for DefaultFactory {
        fn provider_keys(&self) -> &[&str] {
            &["default"]
        }

        fn requires_credentials(&self, _provider_key: &str) -> bool {
            self.requires_credentials
        }

        fn create(
            &self,
            _config: &RociConfig,
            _provider_key: &str,
            _model_id: &str,
        ) -> Result<Box<dyn ModelProvider>, RociError> {
            panic!("default list_models tests must not create providers")
        }
    }

    #[test]
    fn default_descriptor_is_third_party_safe() {
        let with_creds = DefaultFactory {
            requires_credentials: true,
        }
        .descriptor();
        assert_eq!(with_creds.canonical_key, "default");
        assert_eq!(with_creds.credential_flows, vec![CredentialFlow::ApiKey]);
        assert!(with_creds.endpoint_configurable);

        let local = DefaultFactory {
            requires_credentials: false,
        }
        .descriptor();
        assert_eq!(local.credential_flows, vec![CredentialFlow::Local]);
    }

    #[test]
    fn default_is_available_uses_requires_credentials_and_config() {
        let config = RociConfig::new().with_token_store(None);

        assert!(!DefaultFactory {
            requires_credentials: true,
        }
        .is_available(&config, "default"));

        assert!(DefaultFactory {
            requires_credentials: false,
        }
        .is_available(&config, "default"));

        config.set_api_key("default", "token".to_string());
        assert!(DefaultFactory {
            requires_credentials: true,
        }
        .is_available(&config, "default"));
    }

    #[tokio::test]
    async fn default_list_models_requires_credentials_when_unavailable_hidden() {
        let config = RociConfig::new().with_token_store(None);
        let options = ModelListOptions::default();

        let err = DefaultFactory {
            requires_credentials: true,
        }
        .list_models(&config, "default", &options)
        .await
        .unwrap_err();

        assert!(matches!(
            err,
            RociError::MissingCredential { provider } if provider == "default"
        ));
    }

    #[tokio::test]
    async fn default_list_models_returns_empty_catalog_when_available() {
        let config = RociConfig::new().with_token_store(None);
        let options = ModelListOptions::default();

        let catalog = DefaultFactory {
            requires_credentials: false,
        }
        .list_models(&config, "default", &options)
        .await
        .unwrap();

        assert!(catalog.models().is_empty());
    }
}
