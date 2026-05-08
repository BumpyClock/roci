use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{SessionError, SessionId, SessionResult};

/// Metadata recorded for a durable session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SessionMetadata {
    /// Stable durable session ID.
    pub id: SessionId,
    /// Optional human-readable title.
    pub title: Option<String>,
    /// Session creation timestamp.
    pub created_at: DateTime<Utc>,
    /// Last metadata update timestamp.
    pub updated_at: DateTime<Utc>,
    /// Last user/runtime activity timestamp.
    pub last_activity_at: DateTime<Utc>,
    /// Host cwd used when the session was created or imported.
    pub host_cwd: Option<PathBuf>,
    /// Optional source path imported into the session workspace.
    pub import_source: Option<PathBuf>,
}

#[derive(Deserialize)]
struct SessionMetadataWire {
    id: SessionId,
    title: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    #[serde(default)]
    last_activity_at: Option<DateTime<Utc>>,
    host_cwd: Option<PathBuf>,
    import_source: Option<PathBuf>,
}

impl<'de> Deserialize<'de> for SessionMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = SessionMetadataWire::deserialize(deserializer)?;
        Ok(Self {
            id: wire.id,
            title: wire.title,
            created_at: wire.created_at,
            updated_at: wire.updated_at,
            last_activity_at: wire.last_activity_at.unwrap_or(wire.updated_at),
            host_cwd: wire.host_cwd,
            import_source: wire.import_source,
        })
    }
}

impl SessionMetadata {
    /// Create metadata for a new session. Host paths are metadata only.
    #[must_use]
    pub fn new(id: SessionId, host_cwd: Option<PathBuf>, import_source: Option<PathBuf>) -> Self {
        let now = Utc::now();
        Self {
            id,
            title: None,
            created_at: now,
            updated_at: now,
            last_activity_at: now,
            host_cwd,
            import_source,
        }
    }

    /// Read session metadata from JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or decoded.
    pub fn read_from_path(path: impl AsRef<Path>) -> SessionResult<Self> {
        let path = path.as_ref();
        let bytes = fs::read(path).map_err(|source| SessionError::io(path, source))?;
        serde_json::from_slice(&bytes).map_err(|source| SessionError::InvalidMetadata {
            path: path.to_path_buf(),
            message: source.to_string(),
        })
    }

    /// Write session metadata as pretty JSON.
    ///
    /// # Errors
    ///
    /// Returns an error when the metadata cannot be encoded or written.
    pub fn write_to_path(&self, path: impl AsRef<Path>) -> SessionResult<()> {
        let path = path.as_ref();
        let json =
            serde_json::to_vec_pretty(self).map_err(|source| SessionError::InvalidMetadata {
                path: path.to_path_buf(),
                message: source.to_string(),
            })?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| SessionError::io(parent, source))?;
        }
        fs::write(path, json).map_err(|source| SessionError::io(path, source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_metadata_defaults_last_activity_to_updated_at() {
        let updated_at = "2026-05-08T03:00:00Z";
        let json = format!(
            r#"{{
              "id":"session-old",
              "title":null,
              "created_at":"2026-05-08T02:00:00Z",
              "updated_at":"{updated_at}",
              "host_cwd":null,
              "import_source":null
            }}"#
        );

        let metadata: SessionMetadata =
            serde_json::from_str(&json).expect("old metadata should deserialize");

        assert_eq!(metadata.last_activity_at, metadata.updated_at);
    }
}
