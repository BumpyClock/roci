//! Token usage types.

use serde::{Deserialize, Serialize};

/// Token usage for a generation.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub total_tokens: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_creation_tokens: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u32>,
}

impl Usage {
    /// Merge another usage into this one (accumulate).
    pub fn merge(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.total_tokens += other.total_tokens;
        if let Some(v) = other.cache_read_tokens {
            *self.cache_read_tokens.get_or_insert(0) += v;
        }
        if let Some(v) = other.cache_creation_tokens {
            *self.cache_creation_tokens.get_or_insert(0) += v;
        }
        if let Some(v) = other.reasoning_tokens {
            *self.reasoning_tokens.get_or_insert(0) += v;
        }
    }
}
