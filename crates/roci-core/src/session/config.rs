use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{LogicalPath, PathConventions, SessionError, SessionId, SessionResult};

/// Host-provided durable session configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionConfig {
    /// Stable durable session ID.
    pub id: SessionId,
    /// Host-supplied sessions root.
    pub root: PathBuf,
    /// Current workspace path inside the session-owned files namespace.
    pub cwd: LogicalPath,
}

impl SessionConfig {
    /// Create session config with logical cwd set to workspace root.
    #[must_use]
    pub fn new(id: SessionId, root: impl Into<PathBuf>) -> Self {
        Self {
            id,
            root: root.into(),
            cwd: LogicalPath::root(),
        }
    }

    /// Return durable path conventions for this session.
    #[must_use]
    pub fn conventions(&self) -> PathConventions {
        PathConventions::for_session(&self.root, &self.id)
    }

    /// Resolve filesystem aliases in the session root before comparing a prepared state.
    pub(crate) fn canonicalize_root(&mut self) -> SessionResult<()> {
        let canonical = std::fs::canonicalize(&self.root)
            .map_err(|source| SessionError::io(&self.root, source))?;
        self.root = canonical;
        Ok(())
    }
}
