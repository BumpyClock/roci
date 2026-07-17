use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::{
    locks::{ensure_store_root, validate_session_directory, SessionFileLock, SessionLockKind},
    LocalSessionStore, SessionError, SessionId, SessionMetadata, SessionModelPreferences,
    SessionResult,
};

/// Archive visibility used when listing durable sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SessionArchiveFilter {
    /// Return active sessions only.
    #[default]
    Active,
    /// Return archived sessions only.
    Archived,
    /// Return both active and archived sessions.
    All,
}

/// Filters for a durable session catalog query.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SessionCatalogQuery {
    /// Match one exact durable session ID.
    pub id: Option<SessionId>,
    /// Match the host cwd recorded when the session was created or imported.
    pub host_cwd: Option<PathBuf>,
    /// Case-insensitive title substring.
    pub title_contains: Option<String>,
    /// Case-insensitive substring across ID, title, and host cwd.
    pub search: Option<String>,
    /// Include active, archived, or both kinds of sessions.
    pub archive: SessionArchiveFilter,
}

/// One durable session visible to a host catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCatalogEntry {
    /// Durable session metadata.
    pub metadata: SessionMetadata,
    /// Absolute or host-relative path to the session directory under the store root.
    pub path: PathBuf,
}

impl SessionCatalogEntry {
    /// Timestamp used for deterministic most-recent-first ordering.
    #[must_use]
    pub fn last_activity_at(&self) -> DateTime<Utc> {
        self.metadata.last_activity_at
    }
}

impl LocalSessionStore {
    /// List valid durable sessions matching `query`.
    ///
    /// Results are ordered by most recent activity, then session ID. Invalid,
    /// malformed, and symlinked child directories are ignored.
    ///
    /// # Errors
    ///
    /// Returns an error when the configured root cannot be read.
    pub fn list(&self, query: &SessionCatalogQuery) -> SessionResult<Vec<SessionCatalogEntry>> {
        let root = match fs::metadata(&self.root) {
            Ok(metadata) if metadata.file_type().is_dir() => fs::canonicalize(&self.root)
                .map_err(|source| SessionError::io(&self.root, source))?,
            Ok(_) => {
                return Err(SessionError::NotDirectory {
                    path: self.root.clone(),
                });
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => return Err(SessionError::io(&self.root, source)),
        };

        let mut sessions = Vec::new();
        for entry in fs::read_dir(&root).map_err(|source| SessionError::io(&root, source))? {
            let entry = entry.map_err(|source| SessionError::io(&self.root, source))?;
            let file_type = entry
                .file_type()
                .map_err(|source| SessionError::io(entry.path(), source))?;
            if !file_type.is_dir() {
                continue;
            }
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(id) = SessionId::parse(name) else {
                continue;
            };
            let metadata_path = entry.path().join("metadata.json");
            let Ok(metadata_file_type) =
                fs::symlink_metadata(&metadata_path).map(|entry| entry.file_type())
            else {
                continue;
            };
            if !metadata_file_type.is_file() {
                continue;
            }
            let Ok(metadata) = SessionMetadata::read_from_path(&metadata_path) else {
                continue;
            };
            if metadata.id != id || !matches_query(&metadata, query) {
                continue;
            }
            sessions.push(SessionCatalogEntry {
                metadata,
                path: entry.path(),
            });
        }
        sessions.sort_by(|left, right| {
            right
                .last_activity_at()
                .cmp(&left.last_activity_at())
                .then_with(|| left.metadata.id.as_str().cmp(right.metadata.id.as_str()))
        });
        Ok(sessions)
    }

    /// Update a durable session title. `None` clears the title.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is missing, invalid, or cannot be written.
    pub fn update_title(
        &self,
        id: &SessionId,
        title: Option<String>,
    ) -> SessionResult<SessionMetadata> {
        self.update_metadata(id, |metadata| metadata.set_title(title))
    }

    /// Set a durable session title only when it is currently unset.
    ///
    /// The metadata lock covers the read and replacement, so a concurrent
    /// manual rename wins over this initial title assignment.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is missing, invalid, or cannot be written.
    pub fn set_title_if_unset(
        &self,
        id: &SessionId,
        title: String,
    ) -> SessionResult<Option<SessionMetadata>> {
        let root = ensure_store_root(&self.root)?;
        let _metadata_lock =
            SessionFileLock::acquire_blocking(&root, id, SessionLockKind::Metadata)?;
        let entry = self.validated_entry(&root, id)?;
        if entry.metadata.title.is_some() {
            return Ok(None);
        }
        let mut metadata = entry.metadata;
        metadata.set_title(Some(title));
        metadata.replace_existing_at(entry.path.join("metadata.json"))?;
        Ok(Some(metadata))
    }

    /// Mark a durable session as archived.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is missing, invalid, or cannot be written.
    pub fn archive(&self, id: &SessionId) -> SessionResult<SessionMetadata> {
        self.update_metadata(id, SessionMetadata::archive)
    }

    /// Clear a durable session archive marker.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is missing, invalid, or cannot be written.
    pub fn unarchive(&self, id: &SessionId) -> SessionResult<SessionMetadata> {
        self.update_metadata(id, SessionMetadata::unarchive)
    }

    /// Persist model and reasoning-effort selections together for future turns.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is missing, invalid, or cannot be written.
    pub fn update_model_preferences(
        &self,
        id: &SessionId,
        preferences: SessionModelPreferences,
    ) -> SessionResult<SessionMetadata> {
        self.update_metadata(id, |metadata| metadata.set_model_preferences(preferences))
    }

    /// Persist only the selected agent profile for future turns.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is missing, invalid, or cannot be written.
    pub fn update_agent_profile(
        &self,
        id: &SessionId,
        agent_profile: Option<String>,
    ) -> SessionResult<SessionMetadata> {
        self.update_metadata(id, |metadata| metadata.set_agent_profile(agent_profile))
    }

    /// Delete one validated durable session directory.
    ///
    /// # Errors
    ///
    /// Returns an error when the session is missing, invalid, or cannot be removed.
    pub fn delete(&self, id: &SessionId) -> SessionResult<SessionCatalogEntry> {
        let root = ensure_store_root(&self.root)?;
        let _metadata_lock =
            SessionFileLock::acquire_blocking(&root, id, SessionLockKind::Metadata)?;
        let entry = self.validated_entry(&root, id)?;
        let _runtime_lock =
            SessionFileLock::try_acquire(&root, id, SessionLockKind::Runtime, &entry.path)?;
        fs::remove_dir_all(&entry.path).map_err(|source| SessionError::io(&entry.path, source))?;
        Ok(entry)
    }

    fn update_metadata(
        &self,
        id: &SessionId,
        update: impl FnOnce(&mut SessionMetadata),
    ) -> SessionResult<SessionMetadata> {
        let root = ensure_store_root(&self.root)?;
        let _metadata_lock =
            SessionFileLock::acquire_blocking(&root, id, SessionLockKind::Metadata)?;
        let entry = self.validated_entry(&root, id)?;
        let mut metadata = entry.metadata;
        update(&mut metadata);
        metadata.replace_existing_at(entry.path.join("metadata.json"))?;
        Ok(metadata)
    }

    fn validated_entry(&self, root: &Path, id: &SessionId) -> SessionResult<SessionCatalogEntry> {
        let path = validate_session_directory(root, id)?;
        let metadata_path = path.join("metadata.json");
        let metadata_file_type = fs::symlink_metadata(&metadata_path)
            .map_err(|source| session_entry_error(&metadata_path, source))?
            .file_type();
        if !metadata_file_type.is_file() {
            return Err(SessionError::InvalidMetadata {
                path: metadata_path,
                message: "metadata path must be a regular file".to_string(),
            });
        }
        let metadata = SessionMetadata::read_from_path(&metadata_path)?;
        if metadata.id != *id {
            return Err(SessionError::InvalidMetadata {
                path: metadata_path,
                message: "metadata id does not match session directory".to_string(),
            });
        }
        Ok(SessionCatalogEntry { metadata, path })
    }
}

fn session_entry_error(path: &Path, source: std::io::Error) -> SessionError {
    if source.kind() == std::io::ErrorKind::NotFound {
        SessionError::NotFound {
            path: path.to_path_buf(),
        }
    } else {
        SessionError::io(path, source)
    }
}

fn matches_query(metadata: &SessionMetadata, query: &SessionCatalogQuery) -> bool {
    if query.id.as_ref().is_some_and(|id| id != &metadata.id) {
        return false;
    }
    if query
        .host_cwd
        .as_ref()
        .is_some_and(|host_cwd| metadata.host_cwd.as_ref() != Some(host_cwd))
    {
        return false;
    }
    if !matches_archive(metadata, query.archive) {
        return false;
    }
    if query.title_contains.as_deref().is_some_and(|needle| {
        !contains_case_insensitive(metadata.title.as_deref().unwrap_or_default(), needle)
    }) {
        return false;
    }
    let Some(needle) = query.search.as_deref() else {
        return true;
    };
    contains_case_insensitive(metadata.id.as_str(), needle)
        || metadata
            .title
            .as_deref()
            .is_some_and(|title| contains_case_insensitive(title, needle))
        || metadata
            .host_cwd
            .as_ref()
            .is_some_and(|cwd| contains_case_insensitive(&cwd.display().to_string(), needle))
}

fn matches_archive(metadata: &SessionMetadata, filter: SessionArchiveFilter) -> bool {
    match filter {
        SessionArchiveFilter::Active => metadata.archived_at.is_none(),
        SessionArchiveFilter::Archived => metadata.archived_at.is_some(),
        SessionArchiveFilter::All => true,
    }
}

fn contains_case_insensitive(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::symlink;
    use std::path::Path;
    use std::process::{Child, Command};
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    use tempfile::tempdir;

    use super::*;
    use crate::session::{CreateSessionOptions, SessionLease};

    fn id(value: &str) -> SessionId {
        SessionId::parse(value).expect("valid session id")
    }

    #[tokio::test]
    async fn catalog_filters_and_orders_persisted_sessions() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let first = store
            .create(CreateSessionOptions {
                id: Some(id("alpha")),
                title: Some("Build Andromeda".into()),
                host_cwd: Some(PathBuf::from("/projects/andromeda")),
                ..Default::default()
            })
            .await
            .expect("first session creates");
        drop(first);
        let second = store
            .create(CreateSessionOptions {
                id: Some(id("beta")),
                title: Some("Review Roci".into()),
                host_cwd: Some(PathBuf::from("/projects/roci")),
                ..Default::default()
            })
            .await
            .expect("second session creates");
        drop(second);

        let entries = store
            .list(&SessionCatalogQuery {
                search: Some("roci".into()),
                ..Default::default()
            })
            .expect("catalog lists");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].metadata.id, id("beta"));

        let entries = store
            .list(&SessionCatalogQuery {
                host_cwd: Some(PathBuf::from("/projects/andromeda")),
                title_contains: Some("build".into()),
                ..Default::default()
            })
            .expect("catalog filters");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].metadata.id, id("alpha"));
    }

    #[tokio::test]
    async fn catalog_rename_archive_and_unarchive_persist() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let state = store
            .create(CreateSessionOptions {
                id: Some(id("rename-me")),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let renamed = store
            .update_title(&id("rename-me"), Some("Renamed session".into()))
            .expect("title updates");
        assert_eq!(renamed.title.as_deref(), Some("Renamed session"));

        let archived = store.archive(&id("rename-me")).expect("session archives");
        assert!(archived.archived_at.is_some());
        assert!(store
            .list(&SessionCatalogQuery::default())
            .expect("active catalog lists")
            .is_empty());
        assert_eq!(
            store
                .list(&SessionCatalogQuery {
                    archive: SessionArchiveFilter::Archived,
                    ..Default::default()
                })
                .expect("archived catalog lists")[0]
                .metadata
                .title
                .as_deref(),
            Some("Renamed session")
        );

        let restored = store
            .unarchive(&id("rename-me"))
            .expect("session unarchives");
        assert!(restored.archived_at.is_none());
    }

    #[tokio::test]
    async fn set_title_if_unset_is_compare_and_set() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let session_id = id("initial-title");
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let first = store
            .set_title_if_unset(&session_id, "Inferred title".into())
            .expect("initial title updates");
        assert_eq!(
            first
                .as_ref()
                .and_then(|metadata| metadata.title.as_deref()),
            Some("Inferred title")
        );
        assert!(store
            .set_title_if_unset(&session_id, "Should not replace".into())
            .expect("second initial title checks")
            .is_none());

        store
            .update_title(&session_id, Some("Manual title".into()))
            .expect("manual title updates");
        assert!(store
            .set_title_if_unset(&session_id, "Should still not replace".into())
            .expect("manual title wins")
            .is_none());
    }

    #[tokio::test]
    async fn catalog_persists_model_preferences_for_session_resume() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let state = store
            .create(CreateSessionOptions {
                id: Some(id("model-settings")),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let preferences = SessionModelPreferences {
            selected_model: Some(crate::models::LanguageModel::Known {
                provider_key: "openai".into(),
                model_id: "gpt-5".into(),
            }),
            reasoning_effort: Some(crate::types::ReasoningEffort::High),
            agent_profile: Some("builtin:developer".to_string()),
        };
        store
            .update_model_preferences(&id("model-settings"), preferences.clone())
            .expect("model preferences update");

        let reopened = store
            .open(id("model-settings"))
            .await
            .expect("session opens");
        assert_eq!(reopened.metadata.selected_model, preferences.selected_model);
        assert_eq!(
            reopened.metadata.reasoning_effort,
            preferences.reasoning_effort
        );
        assert_eq!(reopened.metadata.agent_profile, preferences.agent_profile);
    }

    #[tokio::test]
    async fn catalog_delete_rejects_invalid_or_untrusted_paths() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let state = store
            .create(CreateSessionOptions {
                id: Some(id("delete-me")),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let deleted = store.delete(&id("delete-me")).expect("session deletes");
        assert_eq!(deleted.metadata.id, id("delete-me"));
        assert!(!deleted.path.exists());

        let alias = root.path().join("alias");
        fs::create_dir(&alias).expect("alias dir creates");
        SessionMetadata::new(id("other"), None, None)
            .write_new_to_path(alias.join("metadata.json"))
            .expect("alias metadata writes");
        assert!(store.delete(&id("alias")).is_err());
        assert!(alias.is_dir());
        assert!(SessionId::parse("../outside").is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn catalog_never_follows_metadata_symlinks() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let state = store
            .create(CreateSessionOptions {
                id: Some(id("symlinked-metadata")),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let sentinel = root.path().join("sentinel.json");
        fs::write(&sentinel, "external sentinel").expect("sentinel writes");
        let metadata = root.path().join("symlinked-metadata").join("metadata.json");
        fs::remove_file(&metadata).expect("metadata removes");
        symlink(&sentinel, &metadata).expect("metadata symlink creates");

        assert!(store
            .update_title(&id("symlinked-metadata"), Some("must not write".into()))
            .is_err());
        assert_eq!(
            fs::read_to_string(&sentinel).expect("sentinel reads"),
            "external sentinel"
        );

        assert!(store.delete(&id("symlinked-metadata")).is_err());
        assert_eq!(
            fs::read_to_string(&sentinel).expect("sentinel reads"),
            "external sentinel"
        );
    }

    #[tokio::test]
    async fn concurrent_metadata_updates_preserve_distinct_fields() {
        let root = tempdir().expect("session root");
        let store = Arc::new(LocalSessionStore::new(root.path()));
        let session_id = id("concurrent-update");
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let barrier = Arc::new(Barrier::new(3));
        let title_store = Arc::clone(&store);
        let title_id = session_id.clone();
        let title_barrier = Arc::clone(&barrier);
        let title = std::thread::spawn(move || {
            title_barrier.wait();
            title_store.update_title(&title_id, Some("Concurrent title".into()))
        });
        let archive_store = Arc::clone(&store);
        let archive_id = session_id.clone();
        let archive_barrier = Arc::clone(&barrier);
        let archive = std::thread::spawn(move || {
            archive_barrier.wait();
            archive_store.archive(&archive_id)
        });
        barrier.wait();

        title
            .join()
            .expect("title thread joins")
            .expect("title updates");
        archive
            .join()
            .expect("archive thread joins")
            .expect("session archives");
        let metadata = SessionMetadata::read_from_path(
            root.path().join(session_id.as_str()).join("metadata.json"),
        )
        .expect("metadata reads");
        assert_eq!(metadata.title.as_deref(), Some("Concurrent title"));
        assert!(metadata.archived_at.is_some());
    }

    #[tokio::test]
    async fn update_and_delete_cannot_recreate_a_session() {
        let root = tempdir().expect("session root");
        let store = Arc::new(LocalSessionStore::new(root.path()));
        let session_id = id("delete-race");
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let barrier = Arc::new(Barrier::new(3));
        let update_store = Arc::clone(&store);
        let update_id = session_id.clone();
        let update_barrier = Arc::clone(&barrier);
        let update = std::thread::spawn(move || {
            update_barrier.wait();
            update_store.update_title(&update_id, Some("racing update".into()))
        });
        let delete_store = Arc::clone(&store);
        let delete_id = session_id.clone();
        let delete_barrier = Arc::clone(&barrier);
        let delete = std::thread::spawn(move || {
            delete_barrier.wait();
            delete_store.delete(&delete_id)
        });
        barrier.wait();

        let _ = update.join().expect("update thread joins");
        delete
            .join()
            .expect("delete thread joins")
            .expect("delete succeeds");
        assert!(!root.path().join(session_id.as_str()).exists());
    }

    #[tokio::test]
    async fn delete_cannot_remove_a_session_with_an_acquired_lease() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let session_id = id("active-delete");
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                ..Default::default()
            })
            .await
            .expect("session creates");

        let error = store
            .delete(&session_id)
            .expect_err("active session cannot delete");
        assert!(matches!(error, SessionError::AlreadyOpen { .. }));
        assert!(root.path().join(session_id.as_str()).is_dir());
        drop(state);
    }

    #[tokio::test]
    async fn open_and_delete_interleaving_keeps_the_acquired_lease_safe() {
        let root = tempdir().expect("session root");
        let store = Arc::new(LocalSessionStore::new(root.path()));
        let session_id = id("open-delete-race");
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let barrier = Arc::new(Barrier::new(3));
        let open_store = Arc::clone(&store);
        let open_id = session_id.clone();
        let open_barrier = Arc::clone(&barrier);
        let open = std::thread::spawn(move || {
            open_barrier.wait();
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime builds")
                .block_on(open_store.open(open_id))
        });
        let delete_store = Arc::clone(&store);
        let delete_id = session_id.clone();
        let delete_barrier = Arc::clone(&barrier);
        let delete = std::thread::spawn(move || {
            delete_barrier.wait();
            delete_store.delete(&delete_id)
        });
        barrier.wait();

        let open = open.join().expect("open thread joins");
        let delete = delete.join().expect("delete thread joins");
        match (open, delete) {
            (Ok(_state), Err(SessionError::AlreadyOpen { .. })) => {
                assert!(root.path().join(session_id.as_str()).is_dir());
            }
            (Err(_), Ok(_)) => {
                assert!(!root.path().join(session_id.as_str()).exists());
            }
            (open, delete) => panic!("invalid open/delete outcome: {open:?}, {delete:?}"),
        }
    }

    #[test]
    fn session_lock_test_child() {
        if env::var_os("ROCI_SESSION_LOCK_TEST_CHILD").is_none() {
            return;
        }
        let root = env_path("ROCI_SESSION_LOCK_ROOT");
        let session_id = SessionId::parse(env::var("ROCI_SESSION_LOCK_ID").expect("session id"))
            .expect("valid session id");
        let action = env::var("ROCI_SESSION_LOCK_ACTION").expect("action");
        let ready = env_path("ROCI_SESSION_LOCK_READY");
        let start = env_path("ROCI_SESSION_LOCK_START");
        let release = env_path("ROCI_SESSION_LOCK_RELEASE");
        let done = env_path("ROCI_SESSION_LOCK_DONE");

        let result = match action.as_str() {
            "runtime" => {
                let lease = SessionLease::acquire(&root, &session_id);
                if lease.is_ok() {
                    write_marker(&ready);
                    wait_for_marker(&release);
                }
                lease.map(drop)
            }
            "title" => {
                write_marker(&ready);
                wait_for_marker(&start);
                LocalSessionStore::new(root)
                    .update_title(&session_id, Some("child title".into()))
                    .map(|_| ())
            }
            "archive" => {
                write_marker(&ready);
                wait_for_marker(&start);
                LocalSessionStore::new(root)
                    .archive(&session_id)
                    .map(|_| ())
            }
            "delete" => {
                write_marker(&ready);
                wait_for_marker(&start);
                LocalSessionStore::new(root).delete(&session_id).map(|_| ())
            }
            other => panic!("unknown child action {other}"),
        };
        fs::write(done, if result.is_ok() { "ok" } else { "err" }).expect("done marker writes");
    }

    #[tokio::test]
    async fn cross_process_metadata_updates_preserve_distinct_fields() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let session_id = id("process-updates");
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let start = root.path().join("start");
        let mut title = spawn_lock_child(root.path(), &session_id, "title", &start);
        let mut archive = spawn_lock_child(root.path(), &session_id, "archive", &start);
        wait_for_marker(&child_marker(root.path(), "title", "ready"));
        wait_for_marker(&child_marker(root.path(), "archive", "ready"));
        write_marker(&start);
        wait_for_child(&mut title);
        wait_for_child(&mut archive);

        let metadata = SessionMetadata::read_from_path(
            root.path().join(session_id.as_str()).join("metadata.json"),
        )
        .expect("metadata reads");
        assert_eq!(metadata.title.as_deref(), Some("child title"));
        assert!(metadata.archived_at.is_some());
    }

    #[tokio::test]
    async fn cross_process_update_and_delete_cannot_ghost_a_session() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let session_id = id("process-delete");
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let start = root.path().join("start");
        let mut update = spawn_lock_child(root.path(), &session_id, "title", &start);
        let mut delete = spawn_lock_child(root.path(), &session_id, "delete", &start);
        wait_for_marker(&child_marker(root.path(), "title", "ready"));
        wait_for_marker(&child_marker(root.path(), "delete", "ready"));
        write_marker(&start);
        wait_for_child(&mut update);
        wait_for_child(&mut delete);

        assert_eq!(
            fs::read_to_string(child_marker(root.path(), "delete", "done")).unwrap(),
            "ok"
        );
        assert!(!root.path().join(session_id.as_str()).exists());
    }

    #[tokio::test]
    async fn cross_process_runtime_lease_blocks_delete() {
        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let session_id = id("process-runtime");
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        let start = root.path().join("unused-start");
        let mut child = spawn_lock_child(root.path(), &session_id, "runtime", &start);
        wait_for_marker(&child_marker(root.path(), "runtime", "ready"));
        let error = store
            .delete(&session_id)
            .expect_err("runtime lock blocks delete");
        assert!(matches!(error, SessionError::AlreadyOpen { .. }));
        assert!(root.path().join(session_id.as_str()).is_dir());
        write_marker(&child_marker(root.path(), "runtime", "release"));
        wait_for_child(&mut child);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn session_store_files_are_private_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempdir().expect("session root");
        let store = LocalSessionStore::new(root.path());
        let session_id = id("private-store");
        let state = store
            .create(CreateSessionOptions {
                id: Some(session_id.clone()),
                ..Default::default()
            })
            .await
            .expect("session creates");
        drop(state);

        assert_eq!(
            fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join(session_id.as_str()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join(".locks"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join(session_id.as_str()).join("metadata.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        for suffix in ["runtime", "metadata"] {
            assert_eq!(
                fs::metadata(root.path().join(".locks").join(format!(
                    "{}.{}.lock",
                    session_id.as_str(),
                    suffix
                )))
                .unwrap()
                .permissions()
                .mode()
                    & 0o777,
                0o600
            );
        }
    }

    fn env_path(name: &str) -> std::path::PathBuf {
        env::var_os(name)
            .map(std::path::PathBuf::from)
            .expect("path environment variable")
    }

    fn spawn_lock_child(root: &Path, id: &SessionId, action: &str, start: &Path) -> Child {
        let ready = child_marker(root, action, "ready");
        let release = child_marker(root, action, "release");
        let done = child_marker(root, action, "done");
        Command::new(env::current_exe().expect("test executable"))
            .arg("--exact")
            .arg("session::catalog::tests::session_lock_test_child")
            .env("ROCI_SESSION_LOCK_TEST_CHILD", "1")
            .env("ROCI_SESSION_LOCK_ROOT", root)
            .env("ROCI_SESSION_LOCK_ID", id.as_str())
            .env("ROCI_SESSION_LOCK_ACTION", action)
            .env("ROCI_SESSION_LOCK_READY", ready)
            .env("ROCI_SESSION_LOCK_START", start)
            .env("ROCI_SESSION_LOCK_RELEASE", release)
            .env("ROCI_SESSION_LOCK_DONE", done)
            .spawn()
            .expect("child test starts")
    }

    fn child_marker(root: &Path, action: &str, state: &str) -> std::path::PathBuf {
        root.join(format!("child-{action}-{state}"))
    }

    fn write_marker(path: &Path) {
        fs::write(path, []).expect("marker writes");
    }

    fn wait_for_marker(path: &Path) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !path.exists() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {}",
                path.display()
            );
            std::thread::yield_now();
        }
    }

    fn wait_for_child(child: &mut Child) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait().expect("child status reads") {
                assert!(status.success(), "child fails: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "child test timed out");
            std::thread::yield_now();
        }
    }
}
