//! Unified error classification and recovery.

use serde::{Deserialize, Serialize};

/// Machine-readable error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidApiKey,
    InsufficientQuota,
    RateLimitExceeded,
    ModelNotFound,
    InvalidRequest,
    ContentFiltered,
    ContextLengthExceeded,
    ServerError,
    ServiceUnavailable,
    Timeout,
    NetworkError,
    Unknown,
}

/// Broad error category for routing recovery logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCategory {
    Authentication,
    RateLimit,
    Network,
    Timeout,
    Server,
    Api,
    Configuration,
    Serialization,
    ToolExecution,
    Unknown,
}

/// Structured details returned by a provider API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorDetails {
    pub code: Option<ErrorCode>,
    pub provider_code: Option<String>,
    pub param: Option<String>,
    pub request_id: Option<String>,
}

/// Suggested recovery action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverySuggestion {
    RetryWithBackoff,
    CheckCredentials,
    CheckConfiguration,
    IncreaseTimeout,
    ReduceInputSize,
    CheckToolImplementation,
    ContactSupport,
}
