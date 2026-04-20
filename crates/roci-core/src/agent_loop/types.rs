//! Core run types for the agent loop.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::types::{ModelMessage, Usage};

/// Unique run identifier.
pub type RunId = Uuid;

/// Run lifecycle status.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Canceled,
}

/// Result of a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunResult {
    pub status: RunStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<ModelMessage>,
    #[serde(default)]
    pub finished_at: DateTime<Utc>,
    /// Accumulated token usage for the run (input + output across all LLM calls).
    ///
    /// `Some` only when the run accrued nonzero observed or estimated usage.
    /// The value may be exact (provider-reported) or a heuristic estimate
    /// when the provider did not report usage, and it may appear on
    /// completed, failed, or canceled runs after provider work began.
    /// `None` for pre-provider failures and zero-usage cases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_delta: Option<Usage>,
}

impl RunResult {
    pub fn completed() -> Self {
        Self::completed_with_messages(Vec::new())
    }

    pub fn completed_with_messages(messages: Vec<ModelMessage>) -> Self {
        Self {
            status: RunStatus::Completed,
            error: None,
            messages,
            finished_at: Utc::now(),
            usage_delta: None,
        }
    }

    pub fn canceled() -> Self {
        Self::canceled_with_messages(Vec::new())
    }

    pub fn canceled_with_messages(messages: Vec<ModelMessage>) -> Self {
        Self {
            status: RunStatus::Canceled,
            error: None,
            messages,
            finished_at: Utc::now(),
            usage_delta: None,
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self::failed_with_messages(error, Vec::new())
    }

    pub fn failed_with_messages(error: impl Into<String>, messages: Vec<ModelMessage>) -> Self {
        Self {
            status: RunStatus::Failed,
            error: Some(error.into()),
            messages,
            finished_at: Utc::now(),
            usage_delta: None,
        }
    }

    /// Attach accumulated usage to this result.
    ///
    /// Only sets `usage_delta` when the run accrued nonzero observed or
    /// estimated usage. Pre-provider failures and zero-usage cases leave
    /// `usage_delta` as `None`.
    pub fn with_usage_delta(mut self, usage: Usage) -> Self {
        if usage.input_tokens > 0 || usage.output_tokens > 0 || usage.total_tokens > 0 {
            self.usage_delta = Some(usage);
        }
        self
    }
}
