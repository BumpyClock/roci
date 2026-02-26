//! OpenAI-compatible model definitions.

use serde::{Deserialize, Serialize};

use super::openai::OpenAiModel;
use roci_core::models::ModelCapabilities;

/// OpenAI-compatible model configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct OpenAiCompatibleModel {
    pub model_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
}

impl OpenAiCompatibleModel {
    pub fn new(model_id: impl Into<String>, base_url: Option<String>) -> Self {
        Self {
            model_id: model_id.into(),
            base_url,
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        OpenAiModel::Custom(self.model_id.clone()).capabilities()
    }
}
