//! Provider factory trait for creating ModelProvider instances.

use std::any::Any;

use super::ModelProvider;
use crate::config::RociConfig;
use crate::error::RociError;

/// Factory for creating ModelProvider instances from a provider key + model ID.
pub trait ProviderFactory: Send + Sync {
    /// Provider key(s) this factory handles (e.g., &["openai", "codex"]).
    fn provider_keys(&self) -> &[&str];

    /// Create a ModelProvider for the given model ID and config.
    fn create(
        &self,
        config: &RociConfig,
        provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError>;

    /// Parse a model ID string into provider-specific representation.
    /// Returns None if this factory does not recognize the model ID.
    fn parse_model(&self, provider_key: &str, model_id: &str)
        -> Option<Box<dyn Any + Send + Sync>>;
}
