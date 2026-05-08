use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::{SessionError, SessionResult};

/// Stable ID for a durable Roci session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Create a random UUID-backed session ID.
    #[must_use]
    pub fn new_v4() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    /// Parse and validate an existing session ID.
    ///
    /// # Errors
    ///
    /// Returns an error when the ID is empty or contains path separator
    /// characters.
    pub fn parse(value: impl Into<String>) -> SessionResult<Self> {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(SessionError::InvalidSessionId {
                value,
                reason: "id must not be empty".to_string(),
            });
        }
        if trimmed != value {
            return Err(SessionError::InvalidSessionId {
                value,
                reason: "id must not have leading or trailing whitespace".to_string(),
            });
        }
        if trimmed == "." || trimmed == ".." {
            return Err(SessionError::InvalidSessionId {
                value,
                reason: "id must not be a relative path segment".to_string(),
            });
        }
        if trimmed.contains('/') || trimmed.contains('\\') {
            return Err(SessionError::InvalidSessionId {
                value,
                reason: "id must not contain path separators".to_string(),
            });
        }

        Ok(Self(trimmed.to_string()))
    }

    /// Borrow the session ID string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for SessionId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_deserializes_through_validator() {
        let id: SessionId =
            serde_json::from_str(r#""session-1""#).expect("valid session id should deserialize");

        assert_eq!(id.as_str(), "session-1");
        assert!(serde_json::from_str::<SessionId>(r#""../outside""#).is_err());
        assert!(serde_json::from_str::<SessionId>(r#"" session ""#).is_err());
    }
}
