//! Authentication value types and credential management.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::RociError;

/// An authentication value (API key, token, etc.).
#[derive(Debug, Clone)]
pub enum AuthValue {
    /// Plain API key.
    ApiKey(String),
    /// Bearer token.
    BearerToken(String),
    /// Environment variable name to read at runtime.
    EnvVar(String),
}

impl AuthValue {
    /// Resolve to the actual secret string.
    pub fn resolve(&self) -> Result<String, RociError> {
        match self {
            Self::ApiKey(k) => Ok(k.clone()),
            Self::BearerToken(t) => Ok(t.clone()),
            Self::EnvVar(var) => std::env::var(var).map_err(|_| {
                RociError::Authentication(format!("Environment variable {var} not set"))
            }),
        }
    }
}

/// Manages credential storage and retrieval.
#[derive(Debug, Default)]
pub struct AuthManager {
    credentials: HashMap<String, AuthValue>,
}

impl AuthManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a credential for a provider.
    pub fn set(&mut self, provider: impl Into<String>, value: AuthValue) {
        self.credentials.insert(provider.into(), value);
    }

    /// Get a credential for a provider.
    pub fn get(&self, provider: &str) -> Option<&AuthValue> {
        self.credentials.get(provider)
    }

    /// Resolve a credential to its string value.
    pub fn resolve(&self, provider: &str) -> Result<String, RociError> {
        self.get(provider)
            .ok_or_else(|| {
                RociError::Authentication(format!("No credentials for provider: {provider}"))
            })?
            .resolve()
    }

    /// Default credential file path (~/.roci/credentials.json).
    pub fn default_credential_path() -> PathBuf {
        dirs_path().join("credentials.json")
    }
}

fn dirs_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".roci")
    } else {
        PathBuf::from(".roci")
    }
}
