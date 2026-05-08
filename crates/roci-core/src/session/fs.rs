use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{LogicalPath, PathConventions, SessionError, SessionResult};

/// Type of a file visible through a session filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionFileKind {
    /// Regular file.
    File,
    /// Directory.
    Directory,
    /// Symbolic link.
    Symlink,
}

/// Metadata for a session workspace path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFileMetadata {
    /// File kind.
    pub kind: SessionFileKind,
    /// Size reported by the local filesystem.
    pub len: u64,
    /// Last modification time when available.
    pub modified_at: Option<DateTime<Utc>>,
}

impl SessionFileMetadata {
    fn from_std(metadata: fs::Metadata) -> Self {
        let file_type = metadata.file_type();
        let kind = if file_type.is_symlink() {
            SessionFileKind::Symlink
        } else if metadata.is_dir() {
            SessionFileKind::Directory
        } else {
            SessionFileKind::File
        };
        let modified_at = metadata.modified().ok().map(DateTime::<Utc>::from);

        Self {
            kind,
            len: metadata.len(),
            modified_at,
        }
    }
}

/// Directory entry returned by [`SessionFs::list`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionDirEntry {
    /// Logical path for this entry.
    pub path: LogicalPath,
    /// File metadata for this entry.
    pub metadata: SessionFileMetadata,
}

/// Filesystem contract for a session-owned workspace.
pub trait SessionFs {
    /// Durable session root.
    fn root(&self) -> &Path;

    /// Session-owned `files/` root.
    fn files_root(&self) -> &Path;

    /// Read bytes from a logical path.
    ///
    /// # Errors
    ///
    /// Returns an error when the path is missing, escapes the `files/` root,
    /// or cannot be read.
    fn read(&self, path: &LogicalPath) -> SessionResult<Vec<u8>>;

    /// Write bytes to a logical path, creating parents as needed.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the `files/` root or cannot be
    /// written.
    fn write(&self, path: &LogicalPath, contents: &[u8]) -> SessionResult<()>;

    /// Append bytes to a logical path, creating parents and file as needed.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the `files/` root or cannot be
    /// written.
    fn append(&self, path: &LogicalPath, contents: &[u8]) -> SessionResult<()>;

    /// List one directory level at a logical path.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes `files/`, is missing, or is not a
    /// directory.
    fn list(&self, path: &LogicalPath) -> SessionResult<Vec<SessionDirEntry>>;

    /// Remove a file, symlink, or directory tree.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes `files/` or cannot be removed.
    fn remove(&self, path: &LogicalPath) -> SessionResult<()>;

    /// Return local filesystem metadata for a logical path.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes `files/` or metadata cannot be
    /// read.
    fn metadata(&self, path: &LogicalPath) -> SessionResult<SessionFileMetadata>;
}

/// Local filesystem-backed session workspace.
#[derive(Debug, Clone)]
pub struct LocalSessionFs {
    conventions: PathConventions,
    canonical_files_root: PathBuf,
}

impl LocalSessionFs {
    /// Create a local session filesystem at `session_root`.
    ///
    /// # Errors
    ///
    /// Returns an error when the session root or `files/` directory cannot be
    /// created.
    pub fn new(session_root: impl Into<PathBuf>) -> SessionResult<Self> {
        Self::with_conventions(PathConventions::new(session_root))
    }

    /// Create a local session filesystem using explicit conventions.
    ///
    /// # Errors
    ///
    /// Returns an error when required directories cannot be created.
    pub fn with_conventions(conventions: PathConventions) -> SessionResult<Self> {
        fs::create_dir_all(conventions.root())
            .map_err(|source| SessionError::io(conventions.root(), source))?;
        fs::create_dir_all(conventions.files_dir())
            .map_err(|source| SessionError::io(conventions.files_dir(), source))?;
        let canonical_files_root = fs::canonicalize(conventions.files_dir())
            .map_err(|source| SessionError::io(conventions.files_dir(), source))?;

        Ok(Self {
            conventions,
            canonical_files_root,
        })
    }

    /// Return path conventions used by this filesystem.
    #[must_use]
    pub fn conventions(&self) -> &PathConventions {
        &self.conventions
    }

    /// Parse a user path and read bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when parsing or reading fails.
    pub fn read_path(&self, path: impl AsRef<Path>) -> SessionResult<Vec<u8>> {
        let path = LogicalPath::parse(path)?;
        self.read(&path)
    }

    /// Parse a user path and write bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when parsing or writing fails.
    pub fn write_path(&self, path: impl AsRef<Path>, contents: &[u8]) -> SessionResult<()> {
        let path = LogicalPath::parse(path)?;
        self.write(&path, contents)
    }

    fn raw_path(&self, path: &LogicalPath) -> PathBuf {
        self.conventions.file_path(path)
    }

    fn ensure_existing_inside_files(&self, path: &LogicalPath) -> SessionResult<PathBuf> {
        let raw = self.raw_path(path);
        let canonical = fs::canonicalize(&raw).map_err(|source| {
            if source.kind() == std::io::ErrorKind::NotFound {
                SessionError::NotFound { path: raw.clone() }
            } else {
                SessionError::io(&raw, source)
            }
        })?;
        self.ensure_inside_files(&canonical)?;
        Ok(canonical)
    }

    fn ensure_write_target_inside_files(&self, path: &LogicalPath) -> SessionResult<PathBuf> {
        let raw = self.raw_path(path);
        if raw.exists() {
            self.ensure_existing_inside_files(path)?;
            return Ok(raw);
        }

        let parent = raw
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.conventions.files_dir());
        fs::create_dir_all(&parent).map_err(|source| SessionError::io(&parent, source))?;
        let canonical_parent =
            fs::canonicalize(&parent).map_err(|source| SessionError::io(&parent, source))?;
        self.ensure_inside_files(&canonical_parent)?;
        Ok(raw)
    }

    fn ensure_inside_files(&self, path: &Path) -> SessionResult<()> {
        if path.starts_with(&self.canonical_files_root) {
            Ok(())
        } else {
            Err(SessionError::PathEscapesFilesRoot {
                path: path.to_path_buf(),
            })
        }
    }
}

impl SessionFs for LocalSessionFs {
    fn root(&self) -> &Path {
        self.conventions.root()
    }

    fn files_root(&self) -> &Path {
        &self.canonical_files_root
    }

    fn read(&self, path: &LogicalPath) -> SessionResult<Vec<u8>> {
        let canonical = self.ensure_existing_inside_files(path)?;
        fs::read(&canonical).map_err(|source| SessionError::io(canonical, source))
    }

    fn write(&self, path: &LogicalPath, contents: &[u8]) -> SessionResult<()> {
        let raw = self.ensure_write_target_inside_files(path)?;
        fs::write(&raw, contents).map_err(|source| SessionError::io(raw, source))
    }

    fn append(&self, path: &LogicalPath, contents: &[u8]) -> SessionResult<()> {
        let raw = self.ensure_write_target_inside_files(path)?;
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&raw)
            .map_err(|source| SessionError::io(&raw, source))?;
        file.write_all(contents)
            .map_err(|source| SessionError::io(raw, source))
    }

    fn list(&self, path: &LogicalPath) -> SessionResult<Vec<SessionDirEntry>> {
        let canonical = self.ensure_existing_inside_files(path)?;
        let metadata =
            fs::metadata(&canonical).map_err(|source| SessionError::io(&canonical, source))?;
        if !metadata.is_dir() {
            return Err(SessionError::NotDirectory { path: canonical });
        }

        let mut entries = Vec::new();
        let read_dir =
            fs::read_dir(&canonical).map_err(|source| SessionError::io(&canonical, source))?;
        for entry in read_dir {
            let entry = entry.map_err(|source| SessionError::io(&canonical, source))?;
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| SessionError::InvalidLogicalPath {
                    path: entry.path().display().to_string(),
                    reason: "path must be valid utf-8".to_string(),
                })?;
            let entry_path = path.join(name)?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|source| SessionError::io(entry.path(), source))?;

            entries.push(SessionDirEntry {
                path: entry_path,
                metadata: SessionFileMetadata::from_std(metadata),
            });
        }
        entries.sort_by(|left, right| left.path.as_str().cmp(right.path.as_str()));
        Ok(entries)
    }

    fn remove(&self, path: &LogicalPath) -> SessionResult<()> {
        if path.is_root() {
            return Err(SessionError::InvalidLogicalPath {
                path: path.to_string(),
                reason: "workspace root cannot be removed".to_string(),
            });
        }

        let raw = self.raw_path(path);
        let _canonical = self.ensure_existing_inside_files(path)?;
        let metadata =
            fs::symlink_metadata(&raw).map_err(|source| SessionError::io(&raw, source))?;

        if metadata.is_dir() && !metadata.file_type().is_symlink() {
            fs::remove_dir_all(&raw).map_err(|source| SessionError::io(raw, source))
        } else {
            fs::remove_file(&raw).map_err(|source| SessionError::io(raw, source))
        }
    }

    fn metadata(&self, path: &LogicalPath) -> SessionResult<SessionFileMetadata> {
        let raw = self.raw_path(path);
        self.ensure_existing_inside_files(path)?;
        let metadata =
            fs::symlink_metadata(&raw).map_err(|source| SessionError::io(raw, source))?;
        Ok(SessionFileMetadata::from_std(metadata))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn local_session_fs_reads_writes_appends_lists_and_removes_files() {
        let temp = tempdir().expect("temp dir should be created");
        let session_fs =
            LocalSessionFs::new(temp.path().join("session")).expect("session fs should be created");
        let file = LogicalPath::parse("notes/today.txt").expect("logical path should parse");

        session_fs
            .write(&file, b"hello")
            .expect("file should be written");
        session_fs
            .append(&file, b" world")
            .expect("file should be appended");
        let contents = session_fs.read(&file).expect("file should be read");
        let entries = session_fs
            .list(&LogicalPath::parse("notes").expect("logical path should parse"))
            .expect("directory should be listed");
        let metadata = session_fs.metadata(&file).expect("metadata should be read");

        assert_eq!(contents, b"hello world");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path.as_str(), "notes/today.txt");
        assert_eq!(metadata.kind, SessionFileKind::File);

        session_fs.remove(&file).expect("file should be removed");
        assert!(matches!(
            session_fs.read(&file),
            Err(SessionError::NotFound { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn local_session_fs_denies_symlink_escape() {
        let temp = tempdir().expect("temp dir should be created");
        let outside = temp.path().join("outside.txt");
        fs::write(&outside, "outside").expect("outside file should be written");
        let session_fs =
            LocalSessionFs::new(temp.path().join("session")).expect("session fs should be created");
        std::os::unix::fs::symlink(
            &outside,
            session_fs.conventions().files_dir().join("escape.txt"),
        )
        .expect("symlink should be created");
        let escape = LogicalPath::parse("escape.txt").expect("logical path should parse");

        let read = session_fs.read(&escape);
        let write = session_fs.write(&escape, b"overwrite");
        let metadata = session_fs.metadata(&escape);

        assert!(matches!(
            read,
            Err(SessionError::PathEscapesFilesRoot { .. })
        ));
        assert!(matches!(
            write,
            Err(SessionError::PathEscapesFilesRoot { .. })
        ));
        assert!(matches!(
            metadata,
            Err(SessionError::PathEscapesFilesRoot { .. })
        ));
        assert_eq!(
            fs::read_to_string(&outside).expect("outside file should be readable"),
            "outside"
        );
    }
}
