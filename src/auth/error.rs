use thiserror::Error;

use crate::error::RociError;

/// Normalized authentication errors across providers.
#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Not logged in")]
    NotLoggedIn,
    #[error("Authorization pending")]
    AuthorizationPending,
    #[error("Access denied")]
    AccessDenied,
    #[error("Expired or invalid grant")]
    ExpiredOrInvalidGrant,
    #[error("Rate limited")]
    RateLimited { retry_after_ms: Option<u64> },
    #[error("Unsupported operation: {0}")]
    Unsupported(String),
    #[error("Invalid response: {0}")]
    InvalidResponse(String),
    #[error("Network error: {0}")]
    Network(String),
    #[error("IO error: {0}")]
    Io(String),
    #[error("Serialization error: {0}")]
    Serialization(String),
}

impl From<reqwest::Error> for AuthError {
    fn from(error: reqwest::Error) -> Self {
        Self::Network(error.to_string())
    }
}

impl From<std::io::Error> for AuthError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<serde_json::Error> for AuthError {
    fn from(error: serde_json::Error) -> Self {
        Self::Serialization(error.to_string())
    }
}

impl From<toml::de::Error> for AuthError {
    fn from(error: toml::de::Error) -> Self {
        Self::Serialization(error.to_string())
    }
}

impl From<toml::ser::Error> for AuthError {
    fn from(error: toml::ser::Error) -> Self {
        Self::Serialization(error.to_string())
    }
}

impl From<AuthError> for RociError {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::RateLimited { retry_after_ms } => RociError::RateLimited { retry_after_ms },
            other => RociError::Authentication(other.to_string()),
        }
    }
}
