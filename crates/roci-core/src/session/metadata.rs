use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{locks::tighten_file, SessionError, SessionId, SessionResult};
use crate::{models::LanguageModel, types::ReasoningEffort};

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
    /// Timestamp when a host archived this session.
    pub archived_at: Option<DateTime<Utc>>,
    /// Model selected for subsequent turns in this session.
    pub selected_model: Option<LanguageModel>,
    /// Reasoning effort selected for subsequent turns in this session.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Main-agent profile override selected for subsequent turns in this session.
    pub agent_profile: Option<String>,
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
    #[serde(default)]
    archived_at: Option<DateTime<Utc>>,
    #[serde(default)]
    selected_model: Option<LanguageModel>,
    #[serde(default)]
    reasoning_effort: Option<ReasoningEffort>,
    #[serde(default)]
    agent_profile: Option<String>,
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
            archived_at: wire.archived_at,
            selected_model: wire.selected_model,
            reasoning_effort: wire.reasoning_effort,
            agent_profile: wire.agent_profile,
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
            archived_at: None,
            selected_model: None,
            reasoning_effort: None,
            agent_profile: None,
            host_cwd,
            import_source,
        }
    }

    /// Update the title and metadata timestamp.
    #[cfg(feature = "agent")]
    pub(crate) fn set_title(&mut self, title: Option<String>) {
        self.title = title;
        self.updated_at = Utc::now();
    }

    /// Mark the session archived and update the metadata timestamp.
    #[cfg(feature = "agent")]
    pub(crate) fn archive(&mut self) {
        let now = Utc::now();
        self.archived_at = Some(now);
        self.updated_at = now;
    }

    /// Clear the archive marker and update the metadata timestamp.
    #[cfg(feature = "agent")]
    pub(crate) fn unarchive(&mut self) {
        self.archived_at = None;
        self.updated_at = Utc::now();
    }

    /// Update the model preferences and metadata timestamp together.
    #[cfg(feature = "agent")]
    pub(crate) fn set_model_preferences(&mut self, preferences: SessionModelPreferences) {
        self.selected_model = preferences.selected_model;
        self.reasoning_effort = preferences.reasoning_effort;
        self.agent_profile = preferences.agent_profile;
        self.updated_at = Utc::now();
    }

    /// Update only the selected agent profile, preserving concurrent model settings.
    #[cfg(feature = "agent")]
    pub(crate) fn set_agent_profile(&mut self, agent_profile: Option<String>) {
        self.agent_profile = agent_profile;
        self.updated_at = Utc::now();
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

    #[cfg_attr(not(feature = "agent"), allow(dead_code))]
    pub(crate) fn write_new_to_path(&self, path: impl AsRef<Path>) -> SessionResult<()> {
        let path = path.as_ref();
        let json = self.serialize(path)?;
        let parent = path.parent().ok_or_else(|| SessionError::InvalidMetadata {
            path: path.to_path_buf(),
            message: "metadata path has no parent directory".to_string(),
        })?;
        let parent_metadata =
            fs::symlink_metadata(parent).map_err(|source| SessionError::io(parent, source))?;
        if !parent_metadata.file_type().is_dir() {
            return Err(SessionError::NotDirectory {
                path: parent.to_path_buf(),
            });
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|source| SessionError::io(path, source))?;
        tighten_file(path)?;
        if let Err(source) = file.write_all(&json).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(path);
            return Err(SessionError::io(path, source));
        }
        Ok(())
    }

    #[cfg(feature = "agent")]
    pub(crate) fn replace_existing_at(&self, path: impl AsRef<Path>) -> SessionResult<()> {
        let path = path.as_ref();
        let existing =
            fs::symlink_metadata(path).map_err(|source| SessionError::io(path, source))?;
        if !existing.file_type().is_file() {
            return Err(SessionError::InvalidMetadata {
                path: path.to_path_buf(),
                message: "metadata path must be a regular file".to_string(),
            });
        }
        let json = self.serialize(path)?;
        let temporary = path.with_file_name(format!(
            ".{}.{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("metadata"),
            uuid::Uuid::new_v4()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|source| SessionError::io(&temporary, source))?;
        if let Err(source) = file.write_all(&json).and_then(|()| file.sync_all()) {
            let _ = fs::remove_file(&temporary);
            return Err(SessionError::io(&temporary, source));
        }
        copy_restrictive_permissions(path, &temporary)?;
        if let Err(error) = replace_metadata_file(&temporary, path) {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(())
    }

    #[cfg_attr(not(feature = "agent"), allow(dead_code))]
    fn serialize(&self, path: &Path) -> SessionResult<Vec<u8>> {
        serde_json::to_vec_pretty(self).map_err(|source| SessionError::InvalidMetadata {
            path: path.to_path_buf(),
            message: source.to_string(),
        })
    }
}

#[cfg(feature = "agent")]
fn copy_restrictive_permissions(existing: &Path, temporary: &Path) -> SessionResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = fs::metadata(existing)
            .map_err(|source| SessionError::io(existing, source))?
            .permissions()
            .mode()
            & 0o600;
        fs::set_permissions(temporary, fs::Permissions::from_mode(mode))
            .map_err(|source| SessionError::io(temporary, source))?;
    }
    #[cfg(not(unix))]
    tighten_file(temporary)?;
    Ok(())
}

#[cfg(feature = "agent")]
fn replace_metadata_file(temporary: &Path, path: &Path) -> SessionResult<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;

        let existing = fs::symlink_metadata(path)
            .map(|_| true)
            .or_else(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    Ok(false)
                } else {
                    Err(error)
                }
            })
            .map_err(|source| SessionError::io(path, source))?;
        if !existing {
            return fs::rename(temporary, path).map_err(|source| SessionError::io(path, source));
        }
        let replaced = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let replacement = temporary
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // SAFETY: Both paths are NUL-terminated UTF-16 buffers that remain alive
        // for the call. No optional backup, exclude, or reserved pointer is used.
        if unsafe {
            ReplaceFileW(
                replaced.as_ptr(),
                replacement.as_ptr(),
                std::ptr::null(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        } == 0
        {
            return Err(SessionError::io(path, std::io::Error::last_os_error()));
        }
        return Ok(());
    }
    #[cfg(not(windows))]
    fs::rename(temporary, path).map_err(|source| SessionError::io(path, source))
}

/// Per-session model settings applied by a host to subsequent turns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SessionModelPreferences {
    /// Selected model, if the host has chosen one for this session.
    pub selected_model: Option<LanguageModel>,
    /// Selected reasoning effort, if the selected model supports one.
    pub reasoning_effort: Option<ReasoningEffort>,
    /// Main-agent profile override, or `None` for registry default resolution.
    pub agent_profile: Option<String>,
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
        assert_eq!(metadata.agent_profile, None);
    }
}
