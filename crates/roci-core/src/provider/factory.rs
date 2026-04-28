//! Provider factory trait for creating ModelProvider instances.

use super::ModelProvider;
use crate::config::RociConfig;
use crate::error::RociError;

/// Factory for creating ModelProvider instances from a provider key + model ID.
pub trait ProviderFactory: Send + Sync {
    /// Provider key(s) this factory handles (e.g., &["openai", "codex"]).
    fn provider_keys(&self) -> &[&str];

    /// Whether this provider key needs credentials before launch-time use.
    fn requires_credentials(&self, _provider_key: &str) -> bool {
        true
    }

    /// Create a ModelProvider for the given model ID and config.
    fn create(
        &self,
        config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError>;
}
