//! Private, account/session-scoped Cursor conversation checkpoints.

use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};

use roci_core::error::RociError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::wire::error;

const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;

/// Stored protocol state contains conversation data but no credentials.
#[derive(Clone, Serialize, Deserialize)]
pub struct CursorSessionSnapshot {
    pub version: u32,
    pub conversation_id: String,
    pub checkpoint: Value,
    /// Base64 blob IDs mapped to base64 blob contents.
    pub blobs: HashMap<String, String>,
    pub input_messages: usize,
    pub input_digest: String,
    pub assistant_text: String,
    /// Only completed text turns are compatible with a new user action.
    /// Tool execution IDs belong to the old HTTP/2 stream and cannot be replayed.
    pub completed_text_turn: bool,
}

/// A claim must exclude concurrent writers for the entire request lifetime.
/// Implementations may reject contention, as the default file store does.
pub trait CursorSessionStore: Send + Sync {
    fn acquire(&self, key: &str) -> Result<Box<dyn CursorSessionLease>, RociError>;
}

/// Exclusive state access; dropping the lease releases its claim.
pub trait CursorSessionLease: Send {
    fn load(&self) -> Result<Option<CursorSessionSnapshot>, RociError>;
    fn save(&mut self, snapshot: &CursorSessionSnapshot) -> Result<(), RociError>;
}

/// File store rooted at a trusted, dedicated directory. Unix files are created
/// mode 0600 and the final directory is 0700. No provider/account/session IDs
/// appear in filenames. Same-user path replacement is outside this boundary.
pub struct FileCursorSessionStore {
    root: PathBuf,
}

impl FileCursorSessionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn new_default() -> Result<Self, RociError> {
        let home = directories::UserDirs::new()
            .ok_or_else(|| error("cannot resolve Cursor session directory"))?;
        Ok(Self::new(home.home_dir().join(".roci/cursor/sessions")))
    }
}

struct FileLease {
    root: PathBuf,
    key: String,
    _lock: File,
}

impl Drop for FileLease {
    fn drop(&mut self) {
        // Explicit unlock also releases a lock whose open file description was
        // transiently inherited by a concurrently spawning child before exec.
        let _ = self._lock.unlock();
    }
}

impl CursorSessionStore for FileCursorSessionStore {
    fn acquire(&self, key: &str) -> Result<Box<dyn CursorSessionLease>, RociError> {
        if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(error("invalid Cursor session key"));
        }
        fs::create_dir_all(&self.root)
            .map_err(|_| error("cannot create Cursor session directory"))?;
        let metadata = fs::symlink_metadata(&self.root)
            .map_err(|_| error("cannot inspect Cursor session directory"))?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(error("unsafe Cursor session directory"));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700))
                .map_err(|_| error("cannot secure Cursor session directory"))?;
        }
        #[cfg(not(unix))]
        return Err(error("default Cursor file sessions require Unix; inject a protected CursorSessionStore on this platform"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let path = self.root.join(format!("{key}.lock"));
            reject_symlink(&path)?;
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .mode(0o600)
                .open(path)
                .map_err(|_| error("cannot open Cursor session lock"))?;
            lock.try_lock()
                .map_err(|_| error("Cursor session is already in use or cannot be locked"))?;
            Ok(Box::new(FileLease {
                root: self.root.clone(),
                key: key.into(),
                _lock: lock,
            }))
        }
    }
}

fn reject_symlink(path: &std::path::Path) -> Result<(), RociError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(error("unsafe Cursor session file"))
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(error("cannot inspect Cursor session file")),
    }
}

impl CursorSessionLease for FileLease {
    fn load(&self) -> Result<Option<CursorSessionSnapshot>, RociError> {
        let path = self.root.join(format!("{}.json", self.key));
        reject_symlink(&path)?;
        let file = match File::open(path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(error("cannot read Cursor session state")),
        };
        let mut data = Vec::new();
        file.take(MAX_STATE_BYTES + 1)
            .read_to_end(&mut data)
            .map_err(|_| error("cannot read Cursor session state"))?;
        if data.len() as u64 > MAX_STATE_BYTES {
            return Err(error("Cursor session state exceeds size limit"));
        }
        let snapshot: CursorSessionSnapshot =
            serde_json::from_slice(&data).map_err(|_| error("invalid Cursor session state"))?;
        if snapshot.version != 1 {
            return Ok(None);
        }
        Ok(Some(snapshot))
    }

    fn save(&mut self, snapshot: &CursorSessionSnapshot) -> Result<(), RociError> {
        let data = serde_json::to_vec(snapshot)
            .map_err(|_| error("cannot encode Cursor session state"))?;
        if data.len() as u64 > MAX_STATE_BYTES {
            return Err(error("Cursor session state exceeds size limit"));
        }
        let temporary = self
            .root
            .join(format!("{}.{}.tmp", self.key, uuid::Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let result = (|| {
            let mut file = options
                .open(&temporary)
                .map_err(|_| error("cannot create Cursor state transaction"))?;
            file.write_all(&data)
                .and_then(|()| file.sync_all())
                .map_err(|_| error("cannot write Cursor session state"))?;
            fs::rename(&temporary, self.root.join(format!("{}.json", self.key)))
                .map_err(|_| error("cannot commit Cursor session state"))?;
            File::open(&self.root)
                .and_then(|f| f.sync_all())
                .map_err(|_| error("cannot sync Cursor session directory"))
        })();
        if result.is_err() {
            let _ = fs::remove_file(temporary);
        }
        result
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn state_survives_reopen_with_private_modes_and_excludes_other_instances() {
        let root = tempfile::tempdir().unwrap();
        let store = FileCursorSessionStore::new(root.path());
        let key = "a".repeat(64);
        let mut lease = store.acquire(&key).unwrap();
        assert!(FileCursorSessionStore::new(root.path())
            .acquire(&key)
            .is_err());
        lease
            .save(&CursorSessionSnapshot {
                version: 1,
                conversation_id: "conversation".into(),
                checkpoint: serde_json::json!({"turns":[]}),
                blobs: HashMap::from([("id".into(), "blob".into())]),
                input_messages: 1,
                input_digest: "digest".into(),
                assistant_text: "answer".into(),
                completed_text_turn: true,
            })
            .unwrap();
        drop(lease);
        let snapshot = FileCursorSessionStore::new(root.path())
            .acquire(&key)
            .unwrap()
            .load()
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.assistant_text, "answer");
        assert_eq!(snapshot.blobs["id"], "blob");
        assert_eq!(
            fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(root.path().join(format!("{key}.json")))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn file_lock_is_cross_process() {
        let key = "b".repeat(64);
        if let Some(root) = std::env::var_os("ROCI_CURSOR_LOCK_PROBE") {
            assert!(FileCursorSessionStore::new(PathBuf::from(root))
                .acquire(&key)
                .is_err());
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let store = FileCursorSessionStore::new(root.path());
        let _lease = store.acquire(&key).unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "provider::cursor::state::tests::file_lock_is_cross_process",
            ])
            .env("ROCI_CURSOR_LOCK_PROBE", root.path())
            .status()
            .unwrap();
        assert!(status.success());
    }
}
