//! Core run types for the agent loop.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

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
    #[serde(default)]
    pub finished_at: DateTime<Utc>,
}

impl RunResult {
    pub fn completed() -> Self {
        Self {
            status: RunStatus::Completed,
            error: None,
            finished_at: Utc::now(),
        }
    }

    pub fn canceled() -> Self {
        Self {
            status: RunStatus::Canceled,
            error: None,
            finished_at: Utc::now(),
        }
    }

    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            status: RunStatus::Failed,
            error: Some(error.into()),
            finished_at: Utc::now(),
        }
    }
}
