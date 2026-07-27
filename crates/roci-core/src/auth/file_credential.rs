//! Locked atomic Unix file storage for provider API-key credentials.

use std::collections::BTreeMap;
use std::ffi::{CString, OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::credential::{
    ProviderCredentialRecord, ProviderCredentialStore, ProviderCredentialStoreError,
    StoredProviderCredentialRecord,
};
#[cfg(test)]
use super::file_credential_tests::{pause_after_read_for_test, CommitFailure};

const AUTH_FILE: &str = "auth.json";
const LOCK_FILE: &str = "auth.json.lock";
const DIRECTORY_MODE: u32 = 0o700;
const FILE_MODE: u32 = 0o600;
const MAX_AUTH_FILE_BYTES: u64 = 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);
const TEMP_CREATE_LIMIT: usize = 20;
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

type StoredRecords = BTreeMap<String, StoredProviderCredentialRecord>;

/// Locked atomic Unix credential map rooted at one dedicated directory.
///
/// `root_dir` is a trusted, dedicated directory containing only `auth.json` and
/// its lock/temporary files. This store creates and chmods only that final
/// directory; its existing intermediate directories are caller-controlled and
/// are not descriptor-walked. The default trusts the user's home directory.
/// Processes able to replace paths as the same user are outside the threat
/// model. Errors and `Debug` redact paths and values.
pub struct FileProviderCredentialStore {
    root_dir: PathBuf,
    #[cfg(test)]
    failure: Option<CommitFailure>,
}

impl FileProviderCredentialStore {
    /// Use a trusted, dedicated credential root directory.
    pub fn new(root_dir: impl Into<PathBuf>) -> Self {
        Self {
            root_dir: root_dir.into(),
            #[cfg(test)]
            failure: None,
        }
    }

    /// Use the canonical `~/.roci` credential root directory.
    pub fn new_default() -> Result<Self, ProviderCredentialStoreError> {
        directories::UserDirs::new()
            .map(|dirs| Self::new(dirs.home_dir().join(".roci")))
            .ok_or(ProviderCredentialStoreError::Unavailable)
    }

    #[cfg(test)]
    pub(super) fn with_failure(root_dir: impl Into<PathBuf>, failure: CommitFailure) -> Self {
        Self {
            root_dir: root_dir.into(),
            failure: Some(failure),
        }
    }

    fn prepare_and_lock(&self) -> Result<(File, File), ProviderCredentialStoreError> {
        let root = open_root(&self.root_dir)?;
        let lock = open_at(
            &root,
            OsStr::new(LOCK_FILE),
            libc::O_RDWR | libc::O_CREAT | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            FILE_MODE,
        )
        .map_err(|_| ProviderCredentialStoreError::Unavailable)?;
        secure_regular_file(&lock)?;
        acquire_lock(&lock)?;
        Ok((lock, root))
    }

    fn read_records(&self, root: &File) -> Result<StoredRecords, ProviderCredentialStoreError> {
        let mut file = match open_at(
            root,
            OsStr::new(AUTH_FILE),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            /*mode*/ 0,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(StoredRecords::new());
            }
            Err(_) => return Err(ProviderCredentialStoreError::Unavailable),
        };
        if secure_regular_file(&file)?.len() > MAX_AUTH_FILE_BYTES {
            return Err(ProviderCredentialStoreError::InvalidRecord);
        }
        let mut raw = Vec::new();
        Read::by_ref(&mut file)
            .take(MAX_AUTH_FILE_BYTES + 1)
            .read_to_end(&mut raw)
            .map_err(|_| ProviderCredentialStoreError::Unavailable)?;
        if raw.len() as u64 > MAX_AUTH_FILE_BYTES {
            return Err(ProviderCredentialStoreError::InvalidRecord);
        }
        let records: StoredRecords = serde_json::from_slice(&raw)
            .map_err(|_| ProviderCredentialStoreError::InvalidRecord)?;
        for record in records.values() {
            record.validate_version()?;
        }
        Ok(records)
    }

    fn commit(
        &self,
        root: &File,
        records: &StoredRecords,
    ) -> Result<(), ProviderCredentialStoreError> {
        let serialized =
            serde_json::to_vec(records).map_err(|_| ProviderCredentialStoreError::InvalidRecord)?;
        if serialized.len() as u64 > MAX_AUTH_FILE_BYTES {
            return Err(ProviderCredentialStoreError::InvalidRecord);
        }
        let (name, file) = create_unique_temp(root)?;
        let mut temp = PendingTempFile {
            root,
            name,
            file,
            committed: false,
        };

        #[cfg(test)]
        if self.failure == Some(CommitFailure::WriteAfterPrefix) {
            temp.file
                .write_all(&serialized[..serialized.len().min(8)])
                .map_err(|_| ProviderCredentialStoreError::Unavailable)?;
            return Err(ProviderCredentialStoreError::Unavailable);
        }

        temp.file
            .write_all(&serialized)
            .and_then(|()| temp.file.sync_all())
            .map_err(|_| ProviderCredentialStoreError::Unavailable)?;
        #[cfg(test)]
        if self.failure == Some(CommitFailure::Rename) {
            return Err(ProviderCredentialStoreError::Unavailable);
        }
        rename_at(root, &temp.name, OsStr::new(AUTH_FILE))?;
        temp.committed = true;
        sync_root(root)
    }
}

impl fmt::Debug for FileProviderCredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileProviderCredentialStore([REDACTED])")
    }
}

impl ProviderCredentialStore for FileProviderCredentialStore {
    fn load(
        &self,
        provider: &str,
    ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
        let (_lock, root) = self.prepare_and_lock()?;
        self.read_records(&root)?
            .remove(provider)
            .map(StoredProviderCredentialRecord::into_record)
            .transpose()
    }

    fn save(
        &self,
        provider: &str,
        record: &ProviderCredentialRecord,
    ) -> Result<(), ProviderCredentialStoreError> {
        let input_bytes = provider
            .len()
            .checked_add(record.api_key.expose_secret().len())
            .and_then(|bytes| {
                record.endpoint.as_ref().map_or(Some(bytes), |endpoint| {
                    bytes.checked_add(endpoint.as_str().len())
                })
            });
        if input_bytes.is_none_or(|bytes| bytes > MAX_AUTH_FILE_BYTES as usize) {
            return Err(ProviderCredentialStoreError::InvalidRecord);
        }
        let (_lock, root) = self.prepare_and_lock()?;
        let mut records = self.read_records(&root)?;
        pause_after_read_for_test();
        records.insert(
            provider.to_string(),
            StoredProviderCredentialRecord::from_record(record),
        );
        self.commit(&root, &records)
    }

    fn clear(&self, provider: &str) -> Result<(), ProviderCredentialStoreError> {
        let (_lock, root) = self.prepare_and_lock()?;
        let mut records = self.read_records(&root)?;
        if records.remove(provider).is_none() {
            return Ok(());
        }
        self.commit(&root, &records)
    }
}

fn open_root(path: &Path) -> Result<File, ProviderCredentialStoreError> {
    match fs::DirBuilder::new().mode(DIRECTORY_MODE).create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(ProviderCredentialStoreError::Unavailable),
    }
    let root = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| ProviderCredentialStoreError::Unavailable)?;
    let metadata = root
        .metadata()
        .map_err(|_| ProviderCredentialStoreError::Unavailable)?;
    if !metadata.file_type().is_dir() {
        return Err(ProviderCredentialStoreError::Unavailable);
    }
    fchmod(&root, DIRECTORY_MODE)?;
    Ok(root)
}

fn open_at(root: &File, name: &OsStr, flags: i32, mode: u32) -> std::io::Result<File> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: `name` is NUL-terminated. Success returns one owned descriptor.
    let descriptor = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            flags,
            mode as libc::mode_t as libc::c_uint,
        )
    };
    if descriptor == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        // SAFETY: `openat` returned a new owned descriptor above.
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

fn secure_regular_file(file: &File) -> Result<fs::Metadata, ProviderCredentialStoreError> {
    let metadata = file
        .metadata()
        .map_err(|_| ProviderCredentialStoreError::Unavailable)?;
    if !metadata.file_type().is_file() {
        return Err(ProviderCredentialStoreError::Unavailable);
    }
    fchmod(file, FILE_MODE)?;
    Ok(metadata)
}

fn fchmod(file: &File, mode: u32) -> Result<(), ProviderCredentialStoreError> {
    // SAFETY: descriptor remains live for this call.
    if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } == 0 {
        Ok(())
    } else {
        Err(ProviderCredentialStoreError::Unavailable)
    }
}

fn acquire_lock(lock: &File) -> Result<(), ProviderCredentialStoreError> {
    let deadline = Instant::now() + LOCK_TIMEOUT;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(()),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                thread::sleep(LOCK_RETRY_DELAY);
            }
            Err(TryLockError::WouldBlock | TryLockError::Error(_)) => {
                return Err(ProviderCredentialStoreError::Unavailable);
            }
        }
    }
}

fn create_unique_temp(root: &File) -> Result<(OsString, File), ProviderCredentialStoreError> {
    for _ in 0..TEMP_CREATE_LIMIT {
        let name = OsString::from(format!(
            "{AUTH_FILE}.tmp.{}.{}.{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        match open_at(
            root,
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            FILE_MODE,
        ) {
            Ok(file) => {
                if let Err(error) = secure_regular_file(&file) {
                    drop(file);
                    unlink_at(root, &name);
                    return Err(error);
                }
                return Ok((name, file));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(ProviderCredentialStoreError::Unavailable),
        }
    }
    Err(ProviderCredentialStoreError::Unavailable)
}

fn rename_at(
    root: &File,
    source: &OsStr,
    target: &OsStr,
) -> Result<(), ProviderCredentialStoreError> {
    let source =
        CString::new(source.as_bytes()).map_err(|_| ProviderCredentialStoreError::Unavailable)?;
    let target =
        CString::new(target.as_bytes()).map_err(|_| ProviderCredentialStoreError::Unavailable)?;
    // SAFETY: names are NUL-terminated; `renameat` does not follow target entry.
    if unsafe {
        libc::renameat(
            root.as_raw_fd(),
            source.as_ptr(),
            root.as_raw_fd(),
            target.as_ptr(),
        )
    } == 0
    {
        Ok(())
    } else {
        Err(ProviderCredentialStoreError::Unavailable)
    }
}

fn unlink_at(root: &File, name: &OsStr) {
    let Ok(name) = CString::new(name.as_bytes()) else {
        return;
    };
    // SAFETY: `name` is NUL-terminated and descriptor remains live.
    unsafe {
        libc::unlinkat(root.as_raw_fd(), name.as_ptr(), /*flags*/ 0)
    };
}

fn sync_root(root: &File) -> Result<(), ProviderCredentialStoreError> {
    match root.sync_all() {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
            ) =>
        {
            Ok(())
        }
        Err(_) => Err(ProviderCredentialStoreError::Unavailable),
    }
}

struct PendingTempFile<'a> {
    root: &'a File,
    name: OsString,
    file: File,
    committed: bool,
}

impl Drop for PendingTempFile<'_> {
    fn drop(&mut self) {
        if !self.committed {
            unlink_at(self.root, &self.name);
        }
    }
}

#[cfg(not(test))]
fn pause_after_read_for_test() {}
