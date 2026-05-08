use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{LogicalPath, PathConventions, SessionError, SessionResult};

/// Resource namespace within a durable session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionResourceNamespace {
    /// Session plan document.
    Plan,
    /// Session workspace YAML.
    Workspace,
    /// Agent-produced artifacts.
    Artifacts,
    /// Session-local temporary files.
    Temp,
    /// Checkpoint snapshots.
    Checkpoints,
    /// User-visible session-owned workspace files.
    Files,
}

/// Metadata returned by session resource commands and queries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionResourceMetadata {
    /// Resource namespace.
    pub namespace: SessionResourceNamespace,
    /// Logical path for path-addressed namespaces.
    pub path: Option<LogicalPath>,
    /// File size in bytes.
    pub len: u64,
    /// Last update time when available.
    pub updated_at: Option<DateTime<Utc>>,
}

/// Local filesystem-backed session resources.
#[derive(Debug, Clone)]
pub struct LocalSessionResources {
    conventions: PathConventions,
}

impl LocalSessionResources {
    /// Create local session resources at `session_root`.
    ///
    /// # Errors
    ///
    /// Returns an error when required directories cannot be created.
    pub fn new(session_root: impl Into<PathBuf>) -> SessionResult<Self> {
        Self::with_conventions(PathConventions::new(session_root))
    }

    /// Create local session resources using explicit path conventions.
    ///
    /// # Errors
    ///
    /// Returns an error when required directories cannot be created.
    pub fn with_conventions(conventions: PathConventions) -> SessionResult<Self> {
        create_dir(conventions.root())?;
        create_namespace_dir(conventions.files_dir())?;
        create_namespace_dir(conventions.artifacts_dir())?;
        create_namespace_dir(conventions.temp_dir())?;
        create_namespace_dir(conventions.checkpoints_dir())?;
        Ok(Self { conventions })
    }

    /// Return path conventions used by this resource store.
    #[must_use]
    pub fn conventions(&self) -> &PathConventions {
        &self.conventions
    }

    /// Write session plan markdown.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan cannot be written.
    pub fn write_plan(&self, content: impl AsRef<str>) -> SessionResult<SessionResourceMetadata> {
        self.write_root_file(
            SessionResourceNamespace::Plan,
            self.conventions.plan_file(),
            content.as_ref().as_bytes(),
        )
    }

    /// Write session workspace YAML.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace file cannot be written.
    pub fn write_workspace_yaml(
        &self,
        content: impl AsRef<str>,
    ) -> SessionResult<SessionResourceMetadata> {
        self.write_root_file(
            SessionResourceNamespace::Workspace,
            self.conventions.workspace_file(),
            content.as_ref().as_bytes(),
        )
    }

    /// Write bytes under `artifacts/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be written.
    pub fn write_artifact(
        &self,
        path: LogicalPath,
        bytes: &[u8],
    ) -> SessionResult<SessionResourceMetadata> {
        self.write_logical_file(SessionResourceNamespace::Artifacts, path, bytes)
    }

    /// Write bytes under `tmp/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be written.
    pub fn write_temp(
        &self,
        path: LogicalPath,
        bytes: &[u8],
    ) -> SessionResult<SessionResourceMetadata> {
        self.write_logical_file(SessionResourceNamespace::Temp, path, bytes)
    }

    /// Write bytes under `checkpoints/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be written.
    pub fn write_checkpoint(
        &self,
        path: LogicalPath,
        bytes: &[u8],
    ) -> SessionResult<SessionResourceMetadata> {
        self.write_logical_file(SessionResourceNamespace::Checkpoints, path, bytes)
    }

    /// Write bytes under `files/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be written.
    pub fn write_file(
        &self,
        path: LogicalPath,
        bytes: &[u8],
    ) -> SessionResult<SessionResourceMetadata> {
        self.write_logical_file(SessionResourceNamespace::Files, path, bytes)
    }

    /// Delete a file or directory under `artifacts/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be deleted.
    pub fn delete_artifact(&self, path: &LogicalPath) -> SessionResult<SessionResourceMetadata> {
        self.delete_logical_path(SessionResourceNamespace::Artifacts, path)
    }

    /// Delete a file or directory under `tmp/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be deleted.
    pub fn delete_temp(&self, path: &LogicalPath) -> SessionResult<SessionResourceMetadata> {
        self.delete_logical_path(SessionResourceNamespace::Temp, path)
    }

    /// Delete a file or directory under `checkpoints/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be deleted.
    pub fn delete_checkpoint(&self, path: &LogicalPath) -> SessionResult<SessionResourceMetadata> {
        self.delete_logical_path(SessionResourceNamespace::Checkpoints, path)
    }

    /// Delete a file or directory under `files/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be deleted.
    pub fn delete_file(&self, path: &LogicalPath) -> SessionResult<SessionResourceMetadata> {
        self.delete_logical_path(SessionResourceNamespace::Files, path)
    }

    /// Read session plan markdown as bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the plan cannot be read.
    pub fn read_plan(&self) -> SessionResult<Vec<u8>> {
        self.read_root_file(self.conventions.plan_file())
    }

    /// Read session workspace YAML as bytes.
    ///
    /// # Errors
    ///
    /// Returns an error when the workspace file cannot be read.
    pub fn read_workspace_yaml(&self) -> SessionResult<Vec<u8>> {
        self.read_root_file(self.conventions.workspace_file())
    }

    /// Read bytes from `artifacts/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be read.
    pub fn read_artifact(&self, path: &LogicalPath) -> SessionResult<Vec<u8>> {
        self.read_logical_file(SessionResourceNamespace::Artifacts, path)
    }

    /// Read bytes from `tmp/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be read.
    pub fn read_temp(&self, path: &LogicalPath) -> SessionResult<Vec<u8>> {
        self.read_logical_file(SessionResourceNamespace::Temp, path)
    }

    /// Read bytes from `checkpoints/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be read.
    pub fn read_checkpoint(&self, path: &LogicalPath) -> SessionResult<Vec<u8>> {
        self.read_logical_file(SessionResourceNamespace::Checkpoints, path)
    }

    /// Read bytes from `files/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the path escapes the namespace or cannot be read.
    pub fn read_file(&self, path: &LogicalPath) -> SessionResult<Vec<u8>> {
        self.read_logical_file(SessionResourceNamespace::Files, path)
    }

    /// List files under `artifacts/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the namespace cannot be listed.
    pub fn list_artifacts(&self) -> SessionResult<Vec<SessionResourceMetadata>> {
        self.list_namespace(SessionResourceNamespace::Artifacts)
    }

    /// List files under `tmp/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the namespace cannot be listed.
    pub fn list_temp(&self) -> SessionResult<Vec<SessionResourceMetadata>> {
        self.list_namespace(SessionResourceNamespace::Temp)
    }

    /// List files under `checkpoints/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the namespace cannot be listed.
    pub fn list_checkpoints(&self) -> SessionResult<Vec<SessionResourceMetadata>> {
        self.list_namespace(SessionResourceNamespace::Checkpoints)
    }

    /// List files under `files/`.
    ///
    /// # Errors
    ///
    /// Returns an error when the namespace cannot be listed.
    pub fn list_files(&self) -> SessionResult<Vec<SessionResourceMetadata>> {
        self.list_namespace(SessionResourceNamespace::Files)
    }

    fn write_root_file(
        &self,
        namespace: SessionResourceNamespace,
        path: PathBuf,
        bytes: &[u8],
    ) -> SessionResult<SessionResourceMetadata> {
        self.ensure_root_write_target(&path)?;
        fs::write(&path, bytes).map_err(|source| SessionError::io(&path, source))?;
        metadata_for(namespace, None, &path)
    }

    fn read_root_file(&self, path: PathBuf) -> SessionResult<Vec<u8>> {
        ensure_existing_non_symlink(&path)?;
        read_file(path)
    }

    fn write_logical_file(
        &self,
        namespace: SessionResourceNamespace,
        path: LogicalPath,
        bytes: &[u8],
    ) -> SessionResult<SessionResourceMetadata> {
        reject_root(&path)?;
        let raw = self.ensure_write_target(namespace, &path)?;
        fs::write(&raw, bytes).map_err(|source| SessionError::io(&raw, source))?;
        metadata_for(namespace, Some(path), &raw)
    }

    fn read_logical_file(
        &self,
        namespace: SessionResourceNamespace,
        path: &LogicalPath,
    ) -> SessionResult<Vec<u8>> {
        let raw = self.ensure_existing(namespace, path)?;
        read_file(raw)
    }

    fn delete_logical_path(
        &self,
        namespace: SessionResourceNamespace,
        path: &LogicalPath,
    ) -> SessionResult<SessionResourceMetadata> {
        reject_root(path)?;
        let raw = self.raw_path(namespace, path);
        self.ensure_existing(namespace, path)?;
        let metadata = metadata_for(namespace, Some(path.clone()), &raw)?;
        let std_metadata =
            fs::symlink_metadata(&raw).map_err(|source| SessionError::io(&raw, source))?;

        if std_metadata.is_dir() && !std_metadata.file_type().is_symlink() {
            fs::remove_dir_all(&raw).map_err(|source| SessionError::io(&raw, source))?;
        } else {
            fs::remove_file(&raw).map_err(|source| SessionError::io(&raw, source))?;
        }

        Ok(metadata)
    }

    fn list_namespace(
        &self,
        namespace: SessionResourceNamespace,
    ) -> SessionResult<Vec<SessionResourceMetadata>> {
        let root = self.namespace_root(namespace);
        let canonical_root = canonical_namespace_root(&root)?;
        let mut entries = Vec::new();
        self.list_recursive(
            namespace,
            &canonical_root,
            &root,
            LogicalPath::root(),
            &mut entries,
        )?;
        entries.sort_by(|left, right| {
            left.path
                .as_ref()
                .map(LogicalPath::as_str)
                .cmp(&right.path.as_ref().map(LogicalPath::as_str))
        });
        Ok(entries)
    }

    fn list_recursive(
        &self,
        namespace: SessionResourceNamespace,
        canonical_root: &Path,
        raw_dir: &Path,
        logical_dir: LogicalPath,
        entries: &mut Vec<SessionResourceMetadata>,
    ) -> SessionResult<()> {
        for entry in fs::read_dir(raw_dir).map_err(|source| SessionError::io(raw_dir, source))? {
            let entry = entry.map_err(|source| SessionError::io(raw_dir, source))?;
            let entry_path = entry.path();
            let name = entry.file_name();
            let name = name
                .to_str()
                .ok_or_else(|| SessionError::InvalidLogicalPath {
                    path: entry_path.display().to_string(),
                    reason: "path must be valid utf-8".to_string(),
                })?;
            let logical_path = logical_dir.join(name)?;
            let metadata = fs::symlink_metadata(&entry_path)
                .map_err(|source| SessionError::io(&entry_path, source))?;
            reject_symlink(&entry_path, &metadata)?;
            let canonical = canonicalize(&entry_path)?;
            ensure_inside(canonical_root, &canonical)?;

            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                self.list_recursive(
                    namespace,
                    canonical_root,
                    &entry_path,
                    logical_path,
                    entries,
                )?;
            } else {
                entries.push(metadata_for(namespace, Some(logical_path), &entry_path)?);
            }
        }

        Ok(())
    }

    fn ensure_existing(
        &self,
        namespace: SessionResourceNamespace,
        path: &LogicalPath,
    ) -> SessionResult<PathBuf> {
        let raw = self.raw_path(namespace, path);
        let canonical = ensure_existing_non_symlink(&raw)?;
        let canonical_root = canonical_namespace_root(self.namespace_root(namespace))?;
        ensure_inside(&canonical_root, &canonical)?;
        Ok(raw)
    }

    fn ensure_write_target(
        &self,
        namespace: SessionResourceNamespace,
        path: &LogicalPath,
    ) -> SessionResult<PathBuf> {
        let raw = self.raw_path(namespace, path);
        match fs::symlink_metadata(&raw) {
            Ok(metadata) => {
                reject_symlink(&raw, &metadata)?;
                self.ensure_existing(namespace, path)?;
                return Ok(raw);
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(SessionError::io(&raw, source)),
        }

        let parent = raw
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.namespace_root(namespace));
        create_dir(&parent)?;
        let canonical_parent = canonicalize(&parent)?;
        let canonical_root = canonical_namespace_root(self.namespace_root(namespace))?;
        ensure_inside(&canonical_root, &canonical_parent)?;
        Ok(raw)
    }

    fn ensure_root_write_target(&self, path: &Path) -> SessionResult<()> {
        match fs::symlink_metadata(path) {
            Ok(metadata) => reject_symlink(path, &metadata),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(SessionError::io(path, source)),
        }
    }

    fn raw_path(&self, namespace: SessionResourceNamespace, path: &LogicalPath) -> PathBuf {
        match namespace {
            SessionResourceNamespace::Plan | SessionResourceNamespace::Workspace => {
                unreachable!("root files do not use logical paths")
            }
            SessionResourceNamespace::Artifacts => self.conventions.artifact_path(path),
            SessionResourceNamespace::Temp => self.conventions.temp_path(path),
            SessionResourceNamespace::Checkpoints => self.conventions.checkpoint_path(path),
            SessionResourceNamespace::Files => self.conventions.file_path(path),
        }
    }

    fn namespace_root(&self, namespace: SessionResourceNamespace) -> PathBuf {
        match namespace {
            SessionResourceNamespace::Plan | SessionResourceNamespace::Workspace => {
                self.conventions.root().to_path_buf()
            }
            SessionResourceNamespace::Artifacts => self.conventions.artifacts_dir(),
            SessionResourceNamespace::Temp => self.conventions.temp_dir(),
            SessionResourceNamespace::Checkpoints => self.conventions.checkpoints_dir(),
            SessionResourceNamespace::Files => self.conventions.files_dir(),
        }
    }
}

fn create_dir(path: impl AsRef<Path>) -> SessionResult<()> {
    let path = path.as_ref();
    fs::create_dir_all(path).map_err(|source| SessionError::io(path, source))
}

fn create_namespace_dir(path: impl AsRef<Path>) -> SessionResult<()> {
    let path = path.as_ref();
    create_dir(path)?;
    let metadata = fs::symlink_metadata(path).map_err(|source| SessionError::io(path, source))?;
    reject_symlink(path, &metadata)?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(SessionError::NotDirectory {
            path: path.to_path_buf(),
        })
    }
}

fn read_file(path: impl AsRef<Path>) -> SessionResult<Vec<u8>> {
    let path = path.as_ref();
    fs::read(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            SessionError::NotFound {
                path: path.to_path_buf(),
            }
        } else {
            SessionError::io(path, source)
        }
    })
}

fn metadata_for(
    namespace: SessionResourceNamespace,
    path: Option<LogicalPath>,
    raw: impl AsRef<Path>,
) -> SessionResult<SessionResourceMetadata> {
    let raw = raw.as_ref();
    let metadata = fs::symlink_metadata(raw).map_err(|source| SessionError::io(raw, source))?;
    let updated_at = metadata.modified().ok().map(DateTime::<Utc>::from);
    Ok(SessionResourceMetadata {
        namespace,
        path,
        len: metadata.len(),
        updated_at,
    })
}

fn canonicalize(path: impl AsRef<Path>) -> SessionResult<PathBuf> {
    let path = path.as_ref();
    fs::canonicalize(path).map_err(|source| SessionError::io(path, source))
}

fn canonicalize_existing(path: impl AsRef<Path>) -> SessionResult<PathBuf> {
    let path = path.as_ref();
    fs::canonicalize(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            SessionError::NotFound {
                path: path.to_path_buf(),
            }
        } else {
            SessionError::io(path, source)
        }
    })
}

fn ensure_existing_non_symlink(path: impl AsRef<Path>) -> SessionResult<PathBuf> {
    let path = path.as_ref();
    let metadata = fs::symlink_metadata(path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            SessionError::NotFound {
                path: path.to_path_buf(),
            }
        } else {
            SessionError::io(path, source)
        }
    })?;
    reject_symlink(path, &metadata)?;
    canonicalize_existing(path)
}

fn canonical_namespace_root(path: impl AsRef<Path>) -> SessionResult<PathBuf> {
    let path = path.as_ref();
    let metadata = fs::symlink_metadata(path).map_err(|source| SessionError::io(path, source))?;
    reject_symlink(path, &metadata)?;
    if metadata.is_dir() {
        canonicalize(path)
    } else {
        Err(SessionError::NotDirectory {
            path: path.to_path_buf(),
        })
    }
}

fn reject_symlink(path: &Path, metadata: &fs::Metadata) -> SessionResult<()> {
    if metadata.file_type().is_symlink() {
        Err(SessionError::PathEscapesFilesRoot {
            path: path.to_path_buf(),
        })
    } else {
        Ok(())
    }
}

fn ensure_inside(root: &Path, path: &Path) -> SessionResult<()> {
    if path.starts_with(root) {
        Ok(())
    } else {
        Err(SessionError::PathEscapesFilesRoot {
            path: path.to_path_buf(),
        })
    }
}

fn reject_root(path: &LogicalPath) -> SessionResult<()> {
    if path.is_root() {
        Err(SessionError::InvalidLogicalPath {
            path: path.to_string(),
            reason: "resource root cannot be used as a file path".to_string(),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::session::{SessionId, SessionMetadata};

    #[test]
    fn local_session_resources_write_files_and_return_metadata() {
        let temp = tempdir().expect("temp dir should be created");
        let resources = LocalSessionResources::new(temp.path().join("session"))
            .expect("resources should be created");
        let artifact_path = LogicalPath::parse("reports/output.txt").expect("path should parse");
        let temp_path = LogicalPath::parse("scratch/cache.bin").expect("path should parse");
        let checkpoint_path = LogicalPath::parse("turn-1/state.json").expect("path should parse");

        let plan = resources
            .write_plan("# Plan\n")
            .expect("plan should be written");
        let workspace = resources
            .write_workspace_yaml("cwd: .\n")
            .expect("workspace should be written");
        let artifact = resources
            .write_artifact(artifact_path.clone(), b"artifact")
            .expect("artifact should be written");
        let temp_file = resources
            .write_temp(temp_path.clone(), b"temp")
            .expect("temp file should be written");
        let checkpoint = resources
            .write_checkpoint(checkpoint_path.clone(), br#"{"ok":true}"#)
            .expect("checkpoint should be written");

        assert!(resources.conventions().plan_file().is_file());
        assert!(resources.conventions().workspace_file().is_file());
        assert!(resources
            .conventions()
            .artifact_path(&artifact_path)
            .is_file());
        assert!(resources.conventions().temp_path(&temp_path).is_file());
        assert!(resources
            .conventions()
            .checkpoint_path(&checkpoint_path)
            .is_file());

        assert_eq!(plan.namespace, SessionResourceNamespace::Plan);
        assert_eq!(plan.path, None);
        assert_eq!(plan.len, 7);
        assert_eq!(workspace.namespace, SessionResourceNamespace::Workspace);
        assert_eq!(workspace.path, None);
        assert_eq!(workspace.len, 7);
        assert_eq!(artifact.namespace, SessionResourceNamespace::Artifacts);
        assert_eq!(artifact.path, Some(artifact_path));
        assert_eq!(artifact.len, 8);
        assert_eq!(temp_file.namespace, SessionResourceNamespace::Temp);
        assert_eq!(temp_file.path, Some(temp_path));
        assert_eq!(temp_file.len, 4);
        assert_eq!(checkpoint.namespace, SessionResourceNamespace::Checkpoints);
        assert_eq!(checkpoint.path, Some(checkpoint_path));
        assert_eq!(checkpoint.len, 11);
    }

    #[test]
    fn local_session_resources_reject_escape_paths() {
        let temp = tempdir().expect("temp dir should be created");
        let resources = LocalSessionResources::new(temp.path().join("session"))
            .expect("resources should be created");

        assert!(LogicalPath::parse("../outside.txt").is_err());
        assert!(LogicalPath::parse("/tmp/out.txt").is_err());
        assert!(LogicalPath::parse("nested\\out.txt").is_err());

        #[cfg(unix)]
        {
            let outside = temp.path().join("outside.txt");
            fs::write(&outside, "outside").expect("outside file should be written");
            std::os::unix::fs::symlink(
                &outside,
                resources.conventions().artifacts_dir().join("out"),
            )
            .expect("symlink should be created");
            let path = LogicalPath::parse("out").expect("logical path should parse");

            let write = resources.write_artifact(path.clone(), b"owned");
            let read = resources.read_artifact(&path);

            assert!(matches!(
                write,
                Err(SessionError::PathEscapesFilesRoot { .. })
            ));
            assert!(matches!(
                read,
                Err(SessionError::PathEscapesFilesRoot { .. })
            ));
            assert_eq!(
                fs::read_to_string(&outside).expect("outside file should be readable"),
                "outside"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn local_session_resources_reject_symlinked_namespace_dir() {
        let temp = tempdir().expect("temp dir should be created");
        let session_root = temp.path().join("session");
        let outside = temp.path().join("outside-artifacts");
        fs::create_dir_all(&session_root).expect("session root should be created");
        fs::create_dir_all(&outside).expect("outside dir should be created");
        std::os::unix::fs::symlink(&outside, session_root.join("artifacts"))
            .expect("namespace symlink should be created");

        let resources = LocalSessionResources::new(session_root);

        assert!(matches!(
            resources,
            Err(SessionError::PathEscapesFilesRoot { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn local_session_resources_reject_broken_final_symlink_write() {
        let temp = tempdir().expect("temp dir should be created");
        let resources = LocalSessionResources::new(temp.path().join("session"))
            .expect("resources should be created");
        let outside = temp.path().join("outside.txt");
        std::os::unix::fs::symlink(
            &outside,
            resources
                .conventions()
                .artifact_path(&LogicalPath::parse("broken").expect("path should parse")),
        )
        .expect("broken symlink should be created");
        let path = LogicalPath::parse("broken").expect("path should parse");

        let write = resources.write_artifact(path, b"outside");

        assert!(matches!(
            write,
            Err(SessionError::PathEscapesFilesRoot { .. })
        ));
        assert!(!outside.exists());
    }

    #[cfg(unix)]
    #[test]
    fn local_session_resources_reject_root_file_symlink() {
        let temp = tempdir().expect("temp dir should be created");
        let resources = LocalSessionResources::new(temp.path().join("session"))
            .expect("resources should be created");
        let outside = temp.path().join("outside-plan.md");
        fs::write(&outside, "outside").expect("outside file should be written");
        std::os::unix::fs::symlink(&outside, resources.conventions().plan_file())
            .expect("plan symlink should be created");

        let write = resources.write_plan("owned");
        let read = resources.read_plan();

        assert!(matches!(
            write,
            Err(SessionError::PathEscapesFilesRoot { .. })
        ));
        assert!(matches!(
            read,
            Err(SessionError::PathEscapesFilesRoot { .. })
        ));
        assert_eq!(
            fs::read_to_string(&outside).expect("outside file should be readable"),
            "outside"
        );
    }

    #[test]
    fn session_metadata_writes_and_reads_from_conventions_path() {
        let temp = tempdir().expect("temp dir should be created");
        let id = SessionId::parse("session-1").expect("session id should parse");
        let conventions = PathConventions::for_session(temp.path(), &id);
        let host_cwd = temp.path().join("host");
        let metadata = SessionMetadata::new(id, Some(host_cwd.clone()), None);

        metadata
            .write_to_path(conventions.metadata_file())
            .expect("metadata should be written");
        let read = SessionMetadata::read_from_path(conventions.metadata_file())
            .expect("metadata should be read");

        assert!(conventions.metadata_file().is_file());
        assert_eq!(read.host_cwd, Some(host_cwd));
    }
}
