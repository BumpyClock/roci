//! Replicate provider (prediction-based API).
//!
//! Replicate uses a different API pattern (create prediction → poll/stream).
//! This is a stub implementation — full Replicate support requires async polling.

use async_trait::async_trait;
use futures::stream::BoxStream;

use crate::error::RociError;
use crate::models::capabilities::ModelCapabilities;
use crate::types::TextStreamDelta;

use super::{ModelProvider, ProviderRequest, ProviderResponse};

pub struct ReplicateProvider {
    model_id: String,
    _api_key: String,
    capabilities: ModelCapabilities,
}

impl ReplicateProvider {
    pub fn new(model_id: String, api_key: String) -> Self {
        Self {
            model_id,
            _api_key: api_key,
            capabilities: ModelCapabilities::default(),
        }
    }
}

#[async_trait]
impl ModelProvider for ReplicateProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "Replicate provider not yet implemented".into(),
        ))
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        Err(RociError::UnsupportedOperation(
            "Replicate streaming not yet implemented".into(),
        ))
    }
}
