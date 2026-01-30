//! Generation settings and related enums.

use bon::Builder;
use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

/// Settings controlling text generation.
#[derive(Debug, Clone, Builder, Serialize, Deserialize, Default)]
pub struct GenerationSettings {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub stop_sequences: Option<Vec<String>>,
    pub presence_penalty: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub seed: Option<u64>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub response_format: Option<ResponseFormat>,
    pub user: Option<String>,
}

/// Reasoning effort level for reasoning models.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Display, EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Low,
    Medium,
    High,
}

/// Requested response format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseFormat {
    Text,
    JsonObject,
    JsonSchema {
        schema: serde_json::Value,
        name: String,
    },
}

/// Why generation finished.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    ContentFilter,
    Error,
}
