//! Model definitions and selection.

pub mod capabilities;
pub mod provider_key;
pub mod selector;

pub use capabilities::ModelCapabilities;
pub use provider_key::ProviderKey;
pub use selector::ModelSelector;

use serde::{Deserialize, Serialize};
use std::fmt;

/// Top-level language model enum using string-based identifiers.
///
/// Provider-specific model enums live in `roci-providers` and are used
/// internally within each `ProviderFactory::create()`. They do not appear
/// in the core public API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum LanguageModel {
    /// Model identified by provider key + model ID.
    /// Resolved to a concrete provider via ProviderRegistry.
    Known {
        provider_key: String,
        model_id: String,
    },
    /// Unregistered / custom model.
    Custom {
        provider: String,
        model_id: String,
    },
}

impl LanguageModel {
    /// Get the model's API identifier string.
    pub fn model_id(&self) -> &str {
        match self {
            Self::Known { model_id, .. } | Self::Custom { model_id, .. } => model_id,
        }
    }

    /// Get the provider name.
    pub fn provider_name(&self) -> &str {
        match self {
            Self::Known { provider_key, .. } => provider_key,
            Self::Custom { provider, .. } => provider,
        }
    }
}

impl fmt::Display for LanguageModel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.provider_name(), self.model_id())
    }
}
