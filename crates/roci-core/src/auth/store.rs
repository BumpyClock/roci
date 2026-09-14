//! Token storage abstraction and file-backed implementation.

use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::AuthError;
use super::token::Token;

/// Storage abstraction for persisted OAuth tokens.
pub trait TokenStore: Send + Sync {
    /// Return the backing store when this object is an account-binding facade.
    /// Implementations that own storage directly use the default `None`.
    fn unscoped_store(&self) -> Option<std::sync::Arc<dyn TokenStore>> {
        None
    }

    /// Stable in-process identity for coordinating one account across facades.
    /// The pointer component is only a lock-map key and must not be persisted.
    fn refresh_coordination_identity(&self, profile: &str) -> (usize, String) {
        (
            self as *const Self as *const () as usize,
            profile.to_owned(),
        )
    }

    fn load(&self, provider: &str, profile: &str) -> Result<Option<Token>, AuthError>;
    fn save(&self, provider: &str, profile: &str, token: &Token) -> Result<(), AuthError>;
    fn clear(&self, provider: &str, profile: &str) -> Result<(), AuthError>;

    /// Publish a refresh only while the stored credential still matches.
    ///
    /// A concurrent logout or login must not be overwritten by an old refresh.
    /// Comparison and replacement must be one atomic operation with respect to
    /// every `save`, `clear`, and conditional write to this credential, including
    /// other store instances sharing its backing storage. Return `false` without
    /// writing when the complete stored token differs from `expected`.
    ///
    /// Implement this with a storage transaction or compare-and-swap, not a
    /// separate `load` followed by `save`. The refresh lease does not replace
    /// this requirement: login and direct credential changes can race a refresh.
    fn save_if_current(
        &self,
        provider: &str,
        profile: &str,
        expected: Option<&Token>,
        replacement: &Token,
    ) -> Result<bool, AuthError>;

    /// Try to coordinate a read/refresh/save transaction for one credential.
    ///
    /// `None` means another refresher holds the lease. Async callers should
    /// retry with an async timer and a deadline, then re-read after acquiring.
    /// Keep the returned lease alive until the refreshed token is persisted.
    /// `Some` must own exclusive refresh access for this credential across all
    /// clients sharing the backing storage, including other processes for a
    /// persistent shared store. Return an error if coordination is unavailable;
    /// a no-op lease does not satisfy this contract.
    ///
    /// The lease must permit `load`, `save_if_current`, and `clear` while held.
    /// Its lock must therefore be separate from individual storage-operation
    /// locks. Dropping the lease releases refresh access, including on cancellation.
    fn try_acquire_refresh_lease(
        &self,
        provider: &str,
        profile: &str,
    ) -> Result<Option<Box<dyn TokenRefreshLease>>, AuthError>;
}

/// Owned refresh transaction lease, released when dropped.
pub trait TokenRefreshLease: Send + Sync {}

struct FileRefreshLease {
    _lock: File,
}

impl TokenRefreshLease for FileRefreshLease {}

impl Drop for FileRefreshLease {
    fn drop(&mut self) {
        // Explicitly release even if a concurrently spawned child inherited the fd.
        let _ = self._lock.unlock();
    }
}

const MAX_TOKEN_FILE_BYTES: u64 = 1024 * 1024;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);

fn unavailable() -> AuthError {
    AuthError::Io("OAuth token storage unavailable".into())
}

fn invalid_record() -> AuthError {
    AuthError::Serialization("Invalid OAuth token record".into())
}

/// Configuration for file-backed token storage.
#[derive(Debug, Clone)]
pub struct TokenStoreConfig {
    pub base_dir: PathBuf,
}

impl TokenStoreConfig {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn default_dir() -> PathBuf {
        default_roci_dir()
    }
}

/// Locked, atomically replaced TOML token files in a trusted dedicated directory.
///
/// On Unix the final directory is mode 0700 and files are mode 0600. Existing
/// intermediate directories are trusted; processes able to replace paths as the
/// same user are outside the threat model. Windows inherits directory ACLs.
/// Refresh transactions use a separate lock from individual storage operations.
#[derive(Clone)]
pub struct FileTokenStore {
    base_dir: PathBuf,
}

impl FileTokenStore {
    pub fn new(config: TokenStoreConfig) -> Self {
        Self {
            base_dir: config.base_dir,
        }
    }

    pub fn new_default() -> Self {
        Self {
            base_dir: default_roci_dir(),
        }
    }

    fn token_path(&self, provider: &str, profile: &str) -> PathBuf {
        let provider = normalize_label(provider);
        let profile = normalize_label(profile);
        let name = if profile == "default" {
            format!("{provider}.toml")
        } else {
            format!("{provider}.{profile}.toml")
        };
        self.base_dir.join(name)
    }

    fn load_unlocked(&self, provider: &str, profile: &str) -> Result<Option<Token>, AuthError> {
        let path = self.token_path(provider, profile);
        let mut file = match private_options().read(true).open(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(unavailable()),
        };
        if secure_regular_file(&file)?.len() > MAX_TOKEN_FILE_BYTES {
            return Err(invalid_record());
        }
        let mut raw = String::new();
        Read::by_ref(&mut file)
            .take(MAX_TOKEN_FILE_BYTES + 1)
            .read_to_string(&mut raw)
            .map_err(|_| invalid_record())?;
        if raw.len() as u64 > MAX_TOKEN_FILE_BYTES {
            return Err(invalid_record());
        }
        // TOML diagnostics can include source snippets containing secrets.
        let file: TokenFile = toml::from_str(&raw).map_err(|_| invalid_record())?;
        if file.version != 1 {
            return Err(invalid_record());
        }
        Ok(Some(file.token))
    }

    fn save_unlocked(&self, provider: &str, profile: &str, token: &Token) -> Result<(), AuthError> {
        let path = self.token_path(provider, profile);
        let file = TokenFile {
            version: 1,
            provider: provider.to_string(),
            profile: profile.to_string(),
            token: token.clone(),
            saved_at: DateTime::<Utc>::from(std::time::SystemTime::now()),
        };
        let serialized = toml::to_string(&file).map_err(|_| invalid_record())?;
        if serialized.len() as u64 > MAX_TOKEN_FILE_BYTES {
            return Err(invalid_record());
        }
        let temp_path = self
            .base_dir
            .join(format!(".oauth-{}.tmp", uuid::Uuid::new_v4()));
        let temp_file = private_options()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|_| unavailable())?;
        let mut temp = PendingTokenFile {
            path: temp_path,
            file: temp_file,
        };
        secure_regular_file(&temp.file)?;
        temp.file
            .write_all(serialized.as_bytes())
            .map_err(|_| unavailable())?;
        temp.file.sync_all().map_err(|_| unavailable())?;
        replace_file(&temp.path, &path)?;
        sync_directory(&self.base_dir)
    }

    fn prepare_lock(
        &self,
        provider: &str,
        profile: &str,
        refresh: bool,
    ) -> Result<File, AuthError> {
        prepare_directory(&self.base_dir)?;
        let path = self.token_path(provider, profile);
        let suffix = if refresh {
            "toml.refresh.lock"
        } else {
            "toml.lock"
        };
        let mut options = private_options();
        options.read(true).write(true).create(true);
        let file = options
            .open(path.with_extension(suffix))
            .map_err(|_| unavailable())?;
        secure_regular_file(&file)?;
        Ok(file)
    }

    fn lock(&self, provider: &str, profile: &str) -> Result<File, AuthError> {
        let lock = self.prepare_lock(provider, profile, false)?;
        let deadline = Instant::now() + LOCK_TIMEOUT;
        loop {
            match lock.try_lock() {
                Ok(()) => return Ok(lock),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(LOCK_RETRY_DELAY);
                }
                Err(_) => return Err(unavailable()),
            }
        }
    }
}

impl fmt::Debug for FileTokenStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("FileTokenStore([REDACTED])")
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self, provider: &str, profile: &str) -> Result<Option<Token>, AuthError> {
        let _lock = self.lock(provider, profile)?;
        self.load_unlocked(provider, profile)
    }

    fn save(&self, provider: &str, profile: &str, token: &Token) -> Result<(), AuthError> {
        let _lock = self.lock(provider, profile)?;
        self.save_unlocked(provider, profile, token)
    }

    fn save_if_current(
        &self,
        provider: &str,
        profile: &str,
        expected: Option<&Token>,
        replacement: &Token,
    ) -> Result<bool, AuthError> {
        let _lock = self.lock(provider, profile)?;
        if self.load_unlocked(provider, profile)?.as_ref() != expected {
            return Ok(false);
        }
        self.save_unlocked(provider, profile, replacement)?;
        Ok(true)
    }

    fn clear(&self, provider: &str, profile: &str) -> Result<(), AuthError> {
        let _lock = self.lock(provider, profile)?;
        let path = self.token_path(provider, profile);
        match fs::remove_file(&path) {
            Ok(()) => sync_directory(&self.base_dir),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(unavailable()),
        }
    }

    fn try_acquire_refresh_lease(
        &self,
        provider: &str,
        profile: &str,
    ) -> Result<Option<Box<dyn TokenRefreshLease>>, AuthError> {
        let lock = self.prepare_lock(provider, profile, true)?;
        match lock.try_lock() {
            Ok(()) => Ok(Some(Box::new(FileRefreshLease { _lock: lock }))),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(_)) => Err(unavailable()),
        }
    }
}

fn prepare_directory(path: &Path) -> Result<(), AuthError> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| unavailable())?;
    if !fs::symlink_metadata(path)
        .map_err(|_| unavailable())?
        .is_dir()
    {
        return Err(unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let root = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|_| unavailable())?;
        root.set_permissions(fs::Permissions::from_mode(0o700))
            .map_err(|_| unavailable())?;
    }
    Ok(())
}

fn private_options() -> OpenOptions {
    let options = OpenOptions::new();
    #[cfg(unix)]
    let options = {
        use std::os::unix::fs::OpenOptionsExt;
        let mut options = options;
        options
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
        options
    };
    options
}

fn secure_regular_file(file: &File) -> Result<fs::Metadata, AuthError> {
    let metadata = file.metadata().map_err(|_| unavailable())?;
    if !metadata.is_file() {
        return Err(unavailable());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| unavailable())?;
    }
    Ok(metadata)
}

fn sync_directory(path: &Path) -> Result<(), AuthError> {
    #[cfg(unix)]
    {
        let root = File::open(path).map_err(|_| unavailable())?;
        match root.sync_all() {
            Ok(()) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
                ) => {}
            Err(_) => return Err(unavailable()),
        }
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn replace_file(temporary: &Path, target: &Path) -> Result<(), AuthError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let temporary: Vec<_> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let target: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: NUL-terminated UTF-16 buffers stay alive for the call.
        if unsafe {
            MoveFileExW(
                temporary.as_ptr(),
                target.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            return Err(unavailable());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    fs::rename(temporary, target).map_err(|_| unavailable())
}

struct PendingTokenFile {
    path: PathBuf,
    file: File,
}

impl Drop for PendingTokenFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TokenFile {
    version: u32,
    provider: String,
    profile: String,
    token: Token,
    saved_at: DateTime<Utc>,
}

fn default_roci_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|dirs| dirs.home_dir().join(".roci"))
        .unwrap_or_else(|| PathBuf::from(".roci"))
}

fn normalize_label(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "default".to_string();
    }
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || lower == '-' {
            out.push(lower);
        } else {
            out.push('-');
        }
    }
    if out.trim_matches('-').is_empty() {
        "default".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_store() -> (TempDir, FileTokenStore) {
        let dir = TempDir::new().unwrap();
        let store = FileTokenStore::new(TokenStoreConfig::new(dir.path().to_path_buf()));
        (dir, store)
    }

    #[test]
    fn token_round_trip_works() {
        let (_dir, store) = temp_store();
        let token = Token {
            provider_metadata: None,
            access_token: "access".to_string(),
            refresh_token: Some("refresh".to_string()),
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: None,
            account_id: None,
        };
        store.save("openai-codex", "default", &token).unwrap();
        let loaded = store.load("openai-codex", "default").unwrap().unwrap();
        assert_eq!(loaded.access_token, "access");
        assert_eq!(loaded.refresh_token.as_deref(), Some("refresh"));
    }

    #[test]
    fn clear_removes_token() {
        let (_dir, store) = temp_store();
        let token = Token {
            provider_metadata: None,
            access_token: "access".to_string(),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: None,
            account_id: None,
        };
        store.save("openai-codex", "default", &token).unwrap();
        store.clear("openai-codex", "default").unwrap();
        let loaded = store.load("openai-codex", "default").unwrap();
        assert!(loaded.is_none());
    }

    fn token_with_access(access_token: &str) -> Token {
        Token {
            provider_metadata: None,
            access_token: access_token.into(),
            refresh_token: Some("refresh".into()),
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: None,
            account_id: None,
        }
    }

    #[test]
    fn refresh_lease_is_scoped_and_allows_storage_operations() {
        let (_dir, store) = temp_store();
        let other = store.clone();
        let lease = store
            .try_acquire_refresh_lease("openai-codex", "default")
            .unwrap()
            .unwrap();
        assert!(other
            .try_acquire_refresh_lease("openai-codex", "default")
            .unwrap()
            .is_none());
        assert!(other
            .try_acquire_refresh_lease("openai-codex", "other")
            .unwrap()
            .is_some());
        store
            .save("openai-codex", "default", &token_with_access("rotated"))
            .unwrap();
        assert_eq!(
            store
                .load("openai-codex", "default")
                .unwrap()
                .unwrap()
                .access_token,
            "rotated"
        );
        store.clear("openai-codex", "default").unwrap();
        drop(lease);
        assert!(other
            .try_acquire_refresh_lease("openai-codex", "default")
            .unwrap()
            .is_some());
    }

    #[test]
    fn invalid_records_are_bounded_and_redacted() {
        let (_dir, store) = temp_store();
        let path = store.token_path("openai-codex", "default");
        fs::write(
            &path,
            "access_token = 'secret-marker'\n invalid secret-marker",
        )
        .unwrap();
        let error = store.load("openai-codex", "default").unwrap_err();
        assert!(!format!("{error:?} {error}").contains("secret-marker"));
        fs::write(&path, vec![b'x'; MAX_TOKEN_FILE_BYTES as usize + 1]).unwrap();
        assert!(store.load("openai-codex", "default").is_err());
        assert!(!format!("{store:?}").contains(&store.base_dir.display().to_string()));
    }

    #[test]
    fn unsupported_version_is_rejected_without_changing_record() {
        let (_dir, store) = temp_store();
        store
            .save("openai-codex", "default", &token_with_access("old"))
            .unwrap();
        let path = store.token_path("openai-codex", "default");
        let unsupported =
            fs::read_to_string(&path)
                .unwrap()
                .replacen("version = 1", "version = 2", 1);
        fs::write(&path, &unsupported).unwrap();
        assert!(store.load("openai-codex", "default").is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), unsupported);
    }

    #[test]
    fn rejected_write_preserves_previous_token() {
        let (_dir, store) = temp_store();
        store
            .save("openai-codex", "default", &token_with_access("old"))
            .unwrap();
        let large = token_with_access(&"x".repeat(MAX_TOKEN_FILE_BYTES as usize));
        assert!(store.save("openai-codex", "default", &large).is_err());
        assert_eq!(
            store
                .load("openai-codex", "default")
                .unwrap()
                .unwrap()
                .access_token,
            "old"
        );
        assert!(fs::read_dir(&store.base_dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[test]
    fn failed_replace_removes_temporary_file() {
        let (_dir, store) = temp_store();
        fs::create_dir(store.token_path("openai-codex", "default")).unwrap();
        assert!(store
            .save("openai-codex", "default", &token_with_access("new"))
            .is_err());
        assert!(fs::read_dir(&store.base_dir).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn files_and_directory_are_private_including_existing_records() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, store) = temp_store();
        fs::set_permissions(&store.base_dir, fs::Permissions::from_mode(0o755)).unwrap();
        store
            .save("openai-codex", "default", &token_with_access("old"))
            .unwrap();
        let path = store.token_path("openai-codex", "default");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        store.load("openai-codex", "default").unwrap();
        let _lease = store
            .try_acquire_refresh_lease("openai-codex", "default")
            .unwrap()
            .unwrap();
        assert_eq!(
            fs::metadata(&store.base_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for entry in fs::read_dir(&store.base_dir).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_are_not_followed() {
        use std::os::unix::fs::symlink;
        let (dir, store) = temp_store();
        let outside = TempDir::new().unwrap();
        let victim = outside.path().join("victim");
        fs::write(&victim, "unchanged").unwrap();
        let path = store.token_path("openai-codex", "default");
        symlink(&victim, &path).unwrap();
        assert!(store.load("openai-codex", "default").is_err());
        store
            .save("openai-codex", "default", &token_with_access("new"))
            .unwrap();
        assert_eq!(fs::read_to_string(&victim).unwrap(), "unchanged");
        fs::remove_file(path.with_extension("toml.lock")).unwrap();
        symlink(&victim, path.with_extension("toml.lock")).unwrap();
        assert!(store
            .save("openai-codex", "default", &token_with_access("bad"))
            .is_err());
        let linked_root = dir.path().join("linked");
        symlink(outside.path(), &linked_root).unwrap();
        let linked_store = FileTokenStore::new(TokenStoreConfig::new(linked_root));
        assert!(linked_store.load("openai-codex", "default").is_err());
    }

    #[test]
    fn concurrent_replacements_never_expose_partial_records() {
        let (_dir, store) = temp_store();
        store
            .save("openai-codex", "default", &token_with_access("initial"))
            .unwrap();
        let path = store.token_path("openai-codex", "default");
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for index in 0..50 {
                    store
                        .save(
                            "openai-codex",
                            "default",
                            &token_with_access(&format!("token-{index}")),
                        )
                        .unwrap();
                }
            });
            for _ in 0..100 {
                let raw = fs::read_to_string(&path).unwrap();
                let parsed: TokenFile =
                    toml::from_str(&raw).expect("readers see a complete committed record");
                assert!(!parsed.token.access_token.is_empty());
            }
        });
    }

    #[test]
    fn refresh_lease_coordinates_separate_processes() {
        let (dir, store) = temp_store();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "auth::store::tests::refresh_lease_child",
                "--ignored",
            ])
            .env("ROCI_TEST_REFRESH_LEASE_DIR", dir.path())
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let ready = dir.path().join("ready");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready.exists() && Instant::now() < deadline {
            assert!(
                child.try_wait().unwrap().is_none(),
                "lease child exited before acquiring lock"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        if !ready.exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("lease child did not acquire lock");
        }
        let contended = store
            .try_acquire_refresh_lease("openai-codex", "default")
            .unwrap()
            .is_none();
        fs::write(dir.path().join("release"), "").unwrap();
        assert!(child.wait().unwrap().success());
        assert!(
            contended,
            "refresh lease must coordinate distinct processes"
        );
        assert!(store
            .try_acquire_refresh_lease("openai-codex", "default")
            .unwrap()
            .is_some());
    }

    #[test]
    #[ignore = "subprocess helper for refresh_lease_coordinates_separate_processes"]
    fn refresh_lease_child() {
        let Some(root) = std::env::var_os("ROCI_TEST_REFRESH_LEASE_DIR") else {
            return;
        };
        let root = PathBuf::from(root);
        let store = FileTokenStore::new(TokenStoreConfig::new(root.clone()));
        let _lease = store
            .try_acquire_refresh_lease("openai-codex", "default")
            .unwrap()
            .unwrap();
        fs::write(root.join("ready"), "").unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !root.join("release").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(root.join("release").exists());
    }

    #[test]
    fn logout_and_relogin_prevent_obsolete_refresh_publication() {
        let (_dir, store) = temp_store();
        let expected = token_with_access("old");
        let replacement = token_with_access("rotated");
        store.save("provider", "default", &expected).unwrap();
        let _lease = store
            .try_acquire_refresh_lease("provider", "default")
            .unwrap()
            .unwrap();
        let other = store.clone();
        other.clear("provider", "default").unwrap();
        assert!(!store
            .save_if_current("provider", "default", Some(&expected), &replacement)
            .unwrap());
        assert!(store.load("provider", "default").unwrap().is_none());
        let login = token_with_access("new-login");
        other.save("provider", "default", &login).unwrap();
        assert!(!store
            .save_if_current("provider", "default", Some(&expected), &replacement)
            .unwrap());
        assert_eq!(store.load("provider", "default").unwrap(), Some(login));
    }

    #[test]
    fn refresh_comparison_includes_metadata_and_optional_fields() {
        let (_dir, store) = temp_store();
        let expected = token_with_access("unchanged-access");
        let replacement = token_with_access("rotated");
        let mutations: [fn(&mut Token); 7] = [
            |token| {
                token.provider_metadata = Some(crate::auth::ProviderTokenMetadata::Gemini {
                    project_id: "new-project".into(),
                })
            },
            |token| token.refresh_token = Some("new-refresh".into()),
            |token| token.id_token = Some("new-id".into()),
            |token| token.expires_at = Some(Utc::now()),
            |token| token.last_refresh = Some(Utc::now()),
            |token| token.scopes = Some(vec!["new-scope".into()]),
            |token| token.account_id = Some("new-account".into()),
        ];
        for mutate in mutations {
            let mut current = expected.clone();
            mutate(&mut current);
            store.save("provider", "default", &current).unwrap();
            assert!(!store
                .save_if_current("provider", "default", Some(&expected), &replacement)
                .unwrap());
            assert_eq!(store.load("provider", "default").unwrap(), Some(current));
        }
        assert!(!store
            .save_if_current("provider", "default", None, &replacement)
            .unwrap());
        store.clear("provider", "default").unwrap();
        assert!(store
            .save_if_current("provider", "default", None, &expected)
            .unwrap());
        assert!(store
            .save_if_current("provider", "default", Some(&expected), &replacement)
            .unwrap());
        assert_eq!(
            store.load("provider", "default").unwrap(),
            Some(replacement)
        );
    }

    #[test]
    fn concurrent_conditional_writes_have_one_winner() {
        let (_dir, store) = temp_store();
        let expected = token_with_access("old");
        store.save("provider", "default", &expected).unwrap();
        let start = std::sync::Barrier::new(3);
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                start.wait();
                store
                    .save_if_current(
                        "provider",
                        "default",
                        Some(&expected),
                        &token_with_access("first"),
                    )
                    .unwrap()
            });
            let second = scope.spawn(|| {
                start.wait();
                store
                    .clone()
                    .save_if_current(
                        "provider",
                        "default",
                        Some(&expected),
                        &token_with_access("second"),
                    )
                    .unwrap()
            });
            start.wait();
            assert_ne!(first.join().unwrap(), second.join().unwrap());
        });
    }
}
