//! Generation settings and related enums.

use bon::Builder;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use strum::{Display, EnumString};

/// Settings controlling text generation.
#[derive(Debug, Clone, Builder, Serialize, Deserialize, Default)]
pub struct GenerationSettings {
    pub max_tokens: Option<u32>,
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub top_k: Option<u32>,
    pub stop_sequences: Option<Vec<String>>,
    pub presence_penalty: Option<f64>,
    pub frequency_penalty: Option<f64>,
    pub seed: Option<u64>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub text_verbosity: Option<TextVerbosity>,
    pub response_format: Option<ResponseFormat>,
    pub openai_responses: Option<OpenAiResponsesOptions>,
    pub user: Option<String>,
}

/// OpenAI Responses API request options.
///
/// Example:
/// ```
/// use roci::types::{GenerationSettings, OpenAiResponsesOptions, OpenAiServiceTier};
///
/// let settings = GenerationSettings {
///     openai_responses: Some(OpenAiResponsesOptions {
///         parallel_tool_calls: Some(false),
///         service_tier: Some(OpenAiServiceTier::Priority),
///         ..Default::default()
///     }),
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OpenAiResponsesOptions {
    pub parallel_tool_calls: Option<bool>,
    pub previous_response_id: Option<String>,
    pub instructions: Option<String>,
    pub metadata: Option<HashMap<String, String>>,
    pub service_tier: Option<OpenAiServiceTier>,
    pub truncation: Option<OpenAiTruncation>,
    pub store: Option<bool>,
}

/// OpenAI service tier for Responses API requests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum OpenAiServiceTier {
    Auto,
    Default,
    Flex,
    Priority,
}

/// OpenAI truncation strategy for Responses API requests.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Display, EnumString)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum OpenAiTruncation {
    Auto,
    Disabled,
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

/// Text verbosity level for GPT-5 responses.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Display, EnumString)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum TextVerbosity {
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
