//! Error types for Roci.

pub mod unified;

pub use unified::{ErrorCategory, ErrorCode, ErrorDetails, RecoverySuggestion};

use thiserror::Error;

/// Primary error type for all Roci operations.
#[derive(Error, Debug)]
pub enum RociError {
    #[error("Configuration error: {0}")]
    Configuration(String),

    #[error("API error (status {status}): {message}")]
    Api {
        status: u16,
        message: String,
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
        details: Option<ErrorDetails>,
    },

    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),

    #[error("Authentication error: {0}")]
    Authentication(String),

    #[error("Rate limited: retry after {retry_after_ms:?}ms")]
    RateLimited { retry_after_ms: Option<u64> },

    #[error("Timeout after {0}ms")]
    Timeout(u64),

    #[error("Stream error: {0}")]
    Stream(String),

    #[error("Tool execution error: {tool_name} — {message}")]
    ToolExecution { tool_name: String, message: String },

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Provider error: {provider} — {message}")]
    Provider { provider: String, message: String },

    #[error("Invalid state: {0}")]
    InvalidState(String),

    #[error("missing credential for provider {provider}")]
    MissingCredential { provider: String },

    #[error("missing configuration key '{key}' for provider {provider}")]
    MissingConfiguration { key: String, provider: String },
}

impl RociError {
    /// Create an API error with details.
    pub fn api(status: u16, message: impl Into<String>) -> Self {
        Self::Api {
            status,
            message: message.into(),
            source: None,
            details: None,
        }
    }

    /// Create an API error with full details.
    pub fn api_with_details(
        status: u16,
        message: impl Into<String>,
        details: ErrorDetails,
    ) -> Self {
        Self::Api {
            status,
            message: message.into(),
            source: None,
            details: Some(details),
        }
    }

    /// Classify this error into a category.
    pub fn category(&self) -> ErrorCategory {
        match self {
            Self::Authentication(_) => ErrorCategory::Authentication,
            Self::RateLimited { .. } => ErrorCategory::RateLimit,
            Self::Network(_) => ErrorCategory::Network,
            Self::Timeout(_) => ErrorCategory::Timeout,
            Self::Configuration(_) => ErrorCategory::Configuration,
            Self::Serialization(_) => ErrorCategory::Serialization,
            Self::Api { status, .. } => match status {
                401 | 403 => ErrorCategory::Authentication,
                429 => ErrorCategory::RateLimit,
                500..=599 => ErrorCategory::Server,
                _ => ErrorCategory::Api,
            },
            Self::ToolExecution { .. } => ErrorCategory::ToolExecution,
            Self::MissingCredential { .. } => ErrorCategory::Authentication,
            Self::MissingConfiguration { .. } => ErrorCategory::Configuration,
            _ => ErrorCategory::Unknown,
        }
    }

    /// Whether this error is potentially retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self.category(),
            ErrorCategory::RateLimit
                | ErrorCategory::Network
                | ErrorCategory::Timeout
                | ErrorCategory::Server
        )
    }

    /// Suggest recovery actions.
    pub fn recovery_suggestion(&self) -> RecoverySuggestion {
        match self.category() {
            ErrorCategory::Authentication => RecoverySuggestion::CheckCredentials,
            ErrorCategory::RateLimit => RecoverySuggestion::RetryWithBackoff,
            ErrorCategory::Network => RecoverySuggestion::RetryWithBackoff,
            ErrorCategory::Timeout => RecoverySuggestion::IncreaseTimeout,
            ErrorCategory::Server => RecoverySuggestion::RetryWithBackoff,
            ErrorCategory::Configuration => RecoverySuggestion::CheckConfiguration,
            ErrorCategory::ToolExecution => RecoverySuggestion::CheckToolImplementation,
            _ => RecoverySuggestion::ContactSupport,
        }
    }
}

/// Convenience alias.
pub type Result<T> = std::result::Result<T, RociError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_credential_has_authentication_category() {
        let err = RociError::MissingCredential {
            provider: "openai".to_string(),
        };
        assert_eq!(err.category(), ErrorCategory::Authentication);
    }

    #[test]
    fn missing_credential_is_not_retryable() {
        let err = RociError::MissingCredential {
            provider: "openai".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn missing_credential_suggests_check_credentials() {
        let err = RociError::MissingCredential {
            provider: "openai".to_string(),
        };
        assert!(matches!(
            err.recovery_suggestion(),
            RecoverySuggestion::CheckCredentials
        ));
    }

    #[test]
    fn missing_configuration_has_configuration_category() {
        let err = RociError::MissingConfiguration {
            key: "api_key".to_string(),
            provider: "openai".to_string(),
        };
        assert_eq!(err.category(), ErrorCategory::Configuration);
    }

    #[test]
    fn missing_configuration_is_not_retryable() {
        let err = RociError::MissingConfiguration {
            key: "api_key".to_string(),
            provider: "openai".to_string(),
        };
        assert!(!err.is_retryable());
    }

    #[test]
    fn missing_configuration_suggests_check_configuration() {
        let err = RociError::MissingConfiguration {
            key: "api_key".to_string(),
            provider: "openai".to_string(),
        };
        assert!(matches!(
            err.recovery_suggestion(),
            RecoverySuggestion::CheckConfiguration
        ));
    }

    #[test]
    fn missing_credential_display_includes_provider() {
        let err = RociError::MissingCredential {
            provider: "anthropic".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("anthropic"),
            "expected provider in message: {msg}"
        );
    }

    #[test]
    fn missing_configuration_display_includes_key_and_provider() {
        let err = RociError::MissingConfiguration {
            key: "base_url".to_string(),
            provider: "copilot".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("base_url"), "expected key in message: {msg}");
        assert!(
            msg.contains("copilot"),
            "expected provider in message: {msg}"
        );
    }
}
