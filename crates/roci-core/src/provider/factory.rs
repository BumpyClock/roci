//! Provider factory trait for creating ModelProvider instances.

use super::ModelProvider;
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::{ModelCatalog, ModelListOptions};
use futures::future::BoxFuture;

/// Factory for creating ModelProvider instances from a provider key + model ID.
pub trait ProviderFactory: Send + Sync {
    /// Provider key(s) this factory handles (e.g., &["openai", "codex"]).
    fn provider_keys(&self) -> &[&str];

    /// Whether this provider key needs credentials before launch-time use.
    fn requires_credentials(&self, _provider_key: &str) -> bool {
        true
    }

    /// List models for the given provider key.
    fn list_models<'a>(
        &'a self,
        config: &'a RociConfig,
        provider_key: &'a str,
        options: &'a ModelListOptions,
    ) -> BoxFuture<'a, Result<ModelCatalog, RociError>> {
        Box::pin(async move {
            if !options.include_unavailable
                && self.requires_credentials(provider_key)
                && config.get_api_key(provider_key).is_none()
            {
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
