//! Ollama local model definitions.

use serde::{Deserialize, Serialize};
use strum::{Display, EnumString};

use roci_core::models::ModelCapabilities;

/// Ollama models (local, OpenAI-compatible API).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Display, EnumString)]
pub enum OllamaModel {
    #[strum(serialize = "llama3.3")]
    Llama33,
    #[strum(serialize = "llama3.1")]
    Llama31,
    #[strum(serialize = "mistral")]
    Mistral,
    #[strum(serialize = "codellama")]
    CodeLlama,
    #[strum(serialize = "deepseek-r1")]
    DeepseekR1,
    #[strum(serialize = "qwen2.5")]
    Qwen25,
    #[strum(default)]
    Custom(String),
}

impl OllamaModel {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Llama33 => "llama3.3",
            Self::Llama31 => "llama3.1",
            Self::Mistral => "mistral",
            Self::CodeLlama => "codellama",
            Self::DeepseekR1 => "deepseek-r1",
            Self::Qwen25 => "qwen2.5",
            Self::Custom(s) => s,
        }
    }

    pub fn capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            supports_vision: false,
            supports_tools: true,
            supports_streaming: true,
            supports_json_mode: true,
            supports_json_schema: false,
            supports_reasoning: matches!(self, Self::DeepseekR1),
            supports_system_messages: true,
            context_length: 128_000,
            max_output_tokens: None,
        }
    }
}
