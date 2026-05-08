use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Deserializer, Serialize};

use super::{SessionError, SessionId, SessionResult};

/// Normalized path relative to the session-owned `files/` workspace.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct LogicalPath(String);

impl LogicalPath {
    /// Return the root logical path.
    #[must_use]
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Parse a user-supplied path into a normalized logical path.
    ///
    /// # Errors
    ///
    /// Returns an error for absolute paths, parent-directory traversal,
    /// Windows-style separators, drive prefixes, or non-UTF-8 path segments.
    pub fn parse(path: impl AsRef<Path>) -> SessionResult<Self> {
        let path = path.as_ref();
        let original = path.display().to_string();

        if original.contains('\\') {
            return Err(SessionError::InvalidLogicalPath {
                path: original,
                reason: "backslash separators are not allowed".to_string(),
            });
        }

        let mut parts = Vec::new();
        for component in path.components() {
            match component {
                Component::CurDir => {}
                Component::Normal(value) => {
                    let value = value
                        .to_str()
                        .ok_or_else(|| SessionError::InvalidLogicalPath {
                            path: original.clone(),
                            reason: "path must be valid utf-8".to_string(),
                        })?;
                    if value.is_empty() {
                        continue;
                    }
                    if value.ends_with(':') {
                        return Err(SessionError::InvalidLogicalPath {
                            path: original,
                            reason: "drive prefixes are not allowed".to_string(),
                        });
                    }
                    parts.push(value.to_string());
                }
                Component::ParentDir => {
                    return Err(SessionError::InvalidLogicalPath {
                        path: original,
                        reason: "parent directory traversal is not allowed".to_string(),
                    });
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(SessionError::InvalidLogicalPath {
                        path: original,
                        reason: "absolute paths are not allowed".to_string(),
                    });
                }
            }
        }

        Ok(Self(parts.join("/")))
    }

    /// Borrow this path as a slash-separated string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Return true when this path points at the session workspace root.
    #[must_use]
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// Convert to an OS path relative to `files/`.
    #[must_use]
    pub fn to_path_buf(&self) -> PathBuf {
        if self.0.is_empty() {
            PathBuf::new()
        } else {
            self.0.split('/').collect()
        }
    }

    /// Join another relative path onto this path.
    ///
    /// # Errors
    ///
    /// Returns an error when the joined path would be invalid.
    pub fn join(&self, path: impl AsRef<Path>) -> SessionResult<Self> {
        let next = LogicalPath::parse(path)?;
        if self.is_root() {
            return Ok(next);
        }
        if next.is_root() {
            return Ok(self.clone());
        }
        LogicalPath::parse(format!("{}/{}", self.0, next.0))
    }
}

impl fmt::Display for LogicalPath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            formatter.write_str(".")
        } else {
            formatter.write_str(&self.0)
        }
    }
}

impl<'de> Deserialize<'de> for LogicalPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value.as_str()).map_err(serde::de::Error::custom)
    }
}

/// Durable session storage namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PathNamespace {
    /// Session metadata JSON.
    Metadata,
    /// Durable event log storage.
    Events,
    /// User-visible session-owned workspace.
    Files,
    /// Agent-produced artifacts.
    Artifacts,
    /// Session-local temporary files.
    Temp,
    /// Checkpoint snapshots.
    Checkpoints,
}

/// Directory and file conventions for a durable session root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathConventions {
    root: PathBuf,
}

impl PathConventions {
    /// Create conventions rooted at one durable session directory.
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Create conventions for `sessions_root/<session_id>`.
    #[must_use]
    pub fn for_session(sessions_root: impl AsRef<Path>, session_id: &SessionId) -> Self {
        Self::new(sessions_root.as_ref().join(session_id.as_str()))
    }

    /// Durable session root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Session metadata path.
    #[must_use]
    pub fn metadata_file(&self) -> PathBuf {
        self.root.join("metadata.json")
    }

    /// Event log path reserved for runtime JSONL.
    #[must_use]
    pub fn events_file(&self) -> PathBuf {
        self.root.join("events.jsonl")
    }

    /// Session plan document path.
    #[must_use]
    pub fn plan_file(&self) -> PathBuf {
        self.root.join("plan.md")
    }

    /// Session workspace YAML path.
    #[must_use]
    pub fn workspace_file(&self) -> PathBuf {
        self.root.join("workspace.yaml")
    }

    /// Session-owned workspace root.
    #[must_use]
    pub fn files_dir(&self) -> PathBuf {
        self.root.join("files")
    }

    /// Agent artifacts directory.
    #[must_use]
    pub fn artifacts_dir(&self) -> PathBuf {
        self.root.join("artifacts")
    }

    /// Session-local temporary directory.
    #[must_use]
    pub fn temp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// Session checkpoints directory.
    #[must_use]
    pub fn checkpoints_dir(&self) -> PathBuf {
        self.root.join("checkpoints")
    }

    /// Directory or file path for a namespace.
    #[must_use]
    pub fn namespace_path(&self, namespace: PathNamespace) -> PathBuf {
        match namespace {
            PathNamespace::Metadata => self.metadata_file(),
            PathNamespace::Events => self.events_file(),
            PathNamespace::Files => self.files_dir(),
            PathNamespace::Artifacts => self.artifacts_dir(),
            PathNamespace::Temp => self.temp_dir(),
            PathNamespace::Checkpoints => self.checkpoints_dir(),
        }
    }

    /// Resolve a logical path under the session-owned `files/` workspace.
    #[must_use]
    pub fn file_path(&self, path: &LogicalPath) -> PathBuf {
        self.files_dir().join(path.to_path_buf())
    }

    /// Resolve a logical path under `artifacts/`.
    #[must_use]
    pub fn artifact_path(&self, path: &LogicalPath) -> PathBuf {
        self.artifacts_dir().join(path.to_path_buf())
    }

    /// Resolve a logical path under `tmp/`.
    #[must_use]
    pub fn temp_path(&self, path: &LogicalPath) -> PathBuf {
        self.temp_dir().join(path.to_path_buf())
    }

    /// Resolve a logical path under `checkpoints/`.
    #[must_use]
    pub fn checkpoint_path(&self, path: &LogicalPath) -> PathBuf {
        self.checkpoints_dir().join(path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logical_path_normalizes_current_dir_and_rejects_escape() {
        let normalized =
            LogicalPath::parse("./alpha//beta/./file.txt").expect("logical path should normalize");

        assert_eq!(normalized.as_str(), "alpha/beta/file.txt");
        assert!(LogicalPath::parse("/tmp/file.txt").is_err());
        assert!(LogicalPath::parse("../file.txt").is_err());
        assert!(LogicalPath::parse("alpha/../file.txt").is_err());
        assert!(LogicalPath::parse("C:/tmp/file.txt").is_err());
        assert!(LogicalPath::parse("alpha\\file.txt").is_err());
    }

    #[test]
    fn logical_path_deserializes_through_validator() {
        let path: LogicalPath = serde_json::from_str(r#""alpha/beta.txt""#)
            .expect("valid logical path should deserialize");

        assert_eq!(path.as_str(), "alpha/beta.txt");
        assert!(serde_json::from_str::<LogicalPath>(r#""../outside.txt""#).is_err());
        assert!(serde_json::from_str::<LogicalPath>(r#""/tmp/out.txt""#).is_err());
        assert!(serde_json::from_str::<LogicalPath>(r#""nested\\out.txt""#).is_err());
    }
}
