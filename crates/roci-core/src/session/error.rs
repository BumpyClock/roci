use std::path::PathBuf;

use thiserror::Error;

/// Result type for session filesystem operations.
pub type SessionResult<T> = std::result::Result<T, SessionError>;

/// Errors raised by session path parsing and local filesystem access.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("invalid session id '{value}': {reason}")]
    InvalidSessionId { value: String, reason: String },
    #[error("invalid logical path '{path}': {reason}")]
    InvalidLogicalPath { path: String, reason: String },
    #[error("invalid session metadata at {path}: {message}")]
    InvalidMetadata { path: PathBuf, message: String },
    #[error("session path escapes files root: {path}")]
    PathEscapesFilesRoot { path: PathBuf },
    #[error("session path not found: {path}")]
    NotFound { path: PathBuf },
    #[error("session path is not a directory: {path}")]
    NotDirectory { path: PathBuf },
    #[error("session filesystem io error for {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl SessionError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}
