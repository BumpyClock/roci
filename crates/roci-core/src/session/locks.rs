#[cfg(feature = "agent")]
use std::collections::HashSet;
use std::fs;
#[cfg(feature = "agent")]
use std::fs::{File, OpenOptions};
use std::path::Path;
#[cfg(feature = "agent")]
use std::path::PathBuf;
#[cfg(feature = "agent")]
use std::sync::{Condvar, Mutex, OnceLock};

#[cfg(feature = "agent")]
use super::SessionId;
use super::{SessionError, SessionResult};

#[cfg(feature = "agent")]
const LOCKS_DIRECTORY: &str = ".locks";

#[cfg(feature = "agent")]
#[derive(Clone, Copy)]
pub(crate) enum SessionLockKind {
    Runtime,
    Metadata,
}

#[cfg(feature = "agent")]
impl SessionLockKind {
    fn file_name(self, id: &SessionId) -> String {
        let suffix = match self {
            Self::Runtime => "runtime",
            Self::Metadata => "metadata",
        };
        format!("{}.{}.lock", id.as_str(), suffix)
    }
}

#[cfg(feature = "agent")]
#[derive(Debug)]
pub(crate) struct SessionFileLock {
    file: File,
    reservation: InProcessReservation,
}

#[cfg(feature = "agent")]
impl SessionFileLock {
    pub(crate) async fn acquire(
        root: &Path,
        id: &SessionId,
        kind: SessionLockKind,
    ) -> SessionResult<Self> {
        let root = root.to_path_buf();
        let id = id.clone();
        let error_path = lock_path(&root, &id, kind);
        tokio::task::spawn_blocking(move || Self::acquire_blocking(&root, &id, kind))
            .await
            .map_err(|source| {
                SessionError::io(
                    error_path,
                    std::io::Error::other(format!("session lock task failed: {source}")),
                )
            })?
    }

    pub(crate) fn acquire_blocking(
        root: &Path,
        id: &SessionId,
        kind: SessionLockKind,
    ) -> SessionResult<Self> {
        let (file, path) = open_lock_file(root, id, kind)?;
        let reservation = InProcessReservation::acquire_blocking(path);
        if let Err(source) = file.lock() {
            drop(reservation);
            return Err(SessionError::io(lock_path(root, id, kind), source));
        }
        Ok(Self { file, reservation })
    }

    pub(crate) fn try_acquire(
        root: &Path,
        id: &SessionId,
        kind: SessionLockKind,
        unavailable_path: &Path,
    ) -> SessionResult<Self> {
        let (file, path) = open_lock_file(root, id, kind)?;
        let reservation = InProcessReservation::try_acquire(path, unavailable_path)?;
        match file.try_lock() {
            Ok(()) => Ok(Self { file, reservation }),
            Err(std::fs::TryLockError::WouldBlock) => {
                drop(reservation);
                Err(SessionError::AlreadyOpen {
                    path: unavailable_path.to_path_buf(),
                })
            }
            Err(std::fs::TryLockError::Error(source)) => {
                drop(reservation);
                Err(SessionError::io(lock_path(root, id, kind), source))
            }
        }
    }
}

#[cfg(feature = "agent")]
impl Drop for SessionFileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
        let _ = &self.reservation;
    }
}

#[cfg(feature = "agent")]
pub(crate) fn ensure_store_root(root: &Path) -> SessionResult<PathBuf> {
    match fs::metadata(root) {
        Ok(metadata) if metadata.file_type().is_dir() => {}
        Ok(_) => {
            return Err(SessionError::NotDirectory {
                path: root.to_path_buf(),
            });
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(root).map_err(|source| SessionError::io(root, source))?;
        }
        Err(source) => return Err(SessionError::io(root, source)),
    }
    let metadata = fs::metadata(root).map_err(|source| SessionError::io(root, source))?;
    if !metadata.file_type().is_dir() {
        return Err(SessionError::NotDirectory {
            path: root.to_path_buf(),
        });
    }
    let canonical = fs::canonicalize(root).map_err(|source| SessionError::io(root, source))?;
    tighten_directory(&canonical)?;
    ensure_lock_directory(&canonical)?;
    Ok(canonical)
}

#[cfg(feature = "agent")]
pub(crate) fn create_session_directory(root: &Path, id: &SessionId) -> SessionResult<PathBuf> {
    let root = ensure_store_root(root)?;
    let path = root.join(id.as_str());
    match fs::create_dir(&path) {
        Ok(()) => {
            tighten_directory(&path)?;
            Ok(path)
        }
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            Err(SessionError::AlreadyExists { path })
        }
        Err(source) => Err(SessionError::io(path, source)),
    }
}

#[cfg(feature = "agent")]
pub(crate) fn validate_session_directory(root: &Path, id: &SessionId) -> SessionResult<PathBuf> {
    let root = ensure_store_root(root)?;
    let path = root.join(id.as_str());
    let metadata =
        fs::symlink_metadata(&path).map_err(|source| session_path_error(&path, source))?;
    if !metadata.file_type().is_dir() {
        return Err(SessionError::NotDirectory { path });
    }
    tighten_directory(&path)?;
    let canonical = fs::canonicalize(&path).map_err(|source| SessionError::io(&path, source))?;
    if !canonical.starts_with(&root) {
        return Err(SessionError::PathEscapesFilesRoot { path: canonical });
    }
    Ok(canonical)
}

#[cfg_attr(not(feature = "agent"), allow(dead_code))]
pub(crate) fn tighten_file(path: &Path) -> SessionResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|source| SessionError::io(path, source))?;
    }
    Ok(())
}

#[cfg(feature = "agent")]
fn open_lock_file(
    root: &Path,
    id: &SessionId,
    kind: SessionLockKind,
) -> SessionResult<(File, PathBuf)> {
    let root = ensure_store_root(root)?;
    let path = root.join(LOCKS_DIRECTORY).join(kind.file_name(id));
    loop {
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.file_type().is_file() {
                    return Err(SessionError::InvalidMetadata {
                        path,
                        message: "session lock path must be a regular file".to_string(),
                    });
                }
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(&path)
                    .map_err(|source| SessionError::io(&path, source))?;
                tighten_file(&path)?;
                return Ok((file, path));
            }
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
                match OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&path)
                {
                    Ok(file) => {
                        tighten_file(&path)?;
                        return Ok((file, path));
                    }
                    Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(source) => return Err(SessionError::io(&path, source)),
                }
            }
            Err(source) => return Err(SessionError::io(&path, source)),
        }
    }
}

#[cfg(feature = "agent")]
fn ensure_lock_directory(root: &Path) -> SessionResult<()> {
    let path = root.join(LOCKS_DIRECTORY);
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(source) => return Err(SessionError::io(&path, source)),
    }
    let metadata = fs::symlink_metadata(&path).map_err(|source| SessionError::io(&path, source))?;
    if !metadata.file_type().is_dir() {
        return Err(SessionError::NotDirectory { path });
    }
    tighten_directory(&path)
}

#[cfg(feature = "agent")]
#[cfg(feature = "agent")]
pub(crate) fn tighten_directory(path: &Path) -> SessionResult<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|source| SessionError::io(path, source))?;
    }
    Ok(())
}

#[cfg(feature = "agent")]
fn lock_path(root: &Path, id: &SessionId, kind: SessionLockKind) -> PathBuf {
    root.join(LOCKS_DIRECTORY).join(kind.file_name(id))
}

#[cfg(feature = "agent")]
fn session_path_error(path: &Path, source: std::io::Error) -> SessionError {
    if source.kind() == std::io::ErrorKind::NotFound {
        SessionError::NotFound {
            path: path.to_path_buf(),
        }
    } else {
        SessionError::io(path, source)
    }
}

#[cfg(feature = "agent")]
static IN_PROCESS_LOCKS: OnceLock<(Mutex<HashSet<PathBuf>>, Condvar)> = OnceLock::new();

#[cfg(feature = "agent")]
#[derive(Debug)]
struct InProcessReservation {
    path: PathBuf,
}

#[cfg(feature = "agent")]
impl InProcessReservation {
    fn acquire_blocking(path: PathBuf) -> Self {
        let (locks, wake) = IN_PROCESS_LOCKS.get_or_init(Default::default);
        let mut held = locks.lock().expect("session file lock mutex poisoned");
        while held.contains(&path) {
            held = wake.wait(held).expect("session file lock mutex poisoned");
        }
        held.insert(path.clone());
        Self { path }
    }

    fn try_acquire(path: PathBuf, unavailable_path: &Path) -> SessionResult<Self> {
        let (locks, _) = IN_PROCESS_LOCKS.get_or_init(Default::default);
        let mut held = locks.lock().expect("session file lock mutex poisoned");
        if !held.insert(path.clone()) {
            return Err(SessionError::AlreadyOpen {
                path: unavailable_path.to_path_buf(),
            });
        }
        Ok(Self { path })
    }
}

#[cfg(feature = "agent")]
impl Drop for InProcessReservation {
    fn drop(&mut self) {
        if let Some((locks, wake)) = IN_PROCESS_LOCKS.get() {
            if let Ok(mut held) = locks.lock() {
                held.remove(&self.path);
                wake.notify_all();
            }
        }
    }
}

#[cfg(all(test, feature = "agent"))]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn async_acquisition_does_not_block_a_current_thread_runtime() {
        let temp = tempdir().expect("temp dir should be created");
        let root = ensure_store_root(temp.path()).expect("store root should initialize");
        let id = SessionId::parse("async-lock").expect("session id should parse");
        let first = SessionFileLock::acquire_blocking(&root, &id, SessionLockKind::Metadata)
            .expect("first lock should acquire");
        let (progress_tx, progress_rx) = mpsc::channel();
        let (done_tx, done_rx) = mpsc::channel();

        let waiter_root = root.clone();
        let waiter_id = id.clone();
        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime should build")
                .block_on(async move {
                    let waiter = tokio::spawn(async move {
                        SessionFileLock::acquire(
                            &waiter_root,
                            &waiter_id,
                            SessionLockKind::Metadata,
                        )
                        .await
                    });
                    tokio::task::yield_now().await;
                    progress_tx
                        .send(())
                        .expect("progress signal should be received");
                    let second = waiter
                        .await
                        .expect("lock task should join")
                        .expect("second lock should acquire");
                    done_tx
                        .send(second)
                        .expect("completion signal should be received");
                });
        });

        progress_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("runtime should make progress while the lock is contended");
        drop(first);
        let second = done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second lock should acquire after release");
        drop(second);
    }
}
