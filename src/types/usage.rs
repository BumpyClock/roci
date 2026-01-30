//! Token usage and cost tracking types.

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

/// Estimated cost for a generation.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Cost {
    pub input_cost: f64,
    pub output_cost: f64,
    pub total_cost: f64,
    pub currency: String,
}

impl Cost {
    /// Compute cost from usage and per-token pricing.
    pub fn from_usage(usage: &Usage, input_price_per_m: f64, output_price_per_m: f64) -> Self {
        let input_cost = (usage.input_tokens as f64 / 1_000_000.0) * input_price_per_m;
        let output_cost = (usage.output_tokens as f64 / 1_000_000.0) * output_price_per_m;
        Self {
            input_cost,
            output_cost,
            total_cost: input_cost + output_cost,
            currency: "USD".to_string(),
        }
    }
}
