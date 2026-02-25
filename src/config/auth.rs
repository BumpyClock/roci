//! Authentication value types and credential management.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::RociError;
use serde::{Deserialize, Serialize};

const CREDENTIAL_FILE_VERSION: u32 = 1;

/// An authentication value (API key, token, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum AuthValue {
    /// Plain API key.
    ApiKey(String),
    /// Bearer token.
    BearerToken(String),
    /// Environment variable name to read at runtime.
    EnvVar(String),
}

impl AuthValue {
    /// Resolve to the actual secret string.
    pub fn resolve(&self) -> Result<String, RociError> {
        match self {
            Self::ApiKey(k) => Ok(k.clone()),
            Self::BearerToken(t) => Ok(t.clone()),
            Self::EnvVar(var) => std::env::var(var).map_err(|_| {
                RociError::Authentication(format!("Environment variable {var} not set"))
            }),
        }
    }
}

/// Manages credential storage and retrieval.
#[derive(Debug, Default)]
pub struct AuthManager {
    credentials: HashMap<String, AuthValue>,
}

impl AuthManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a credential for a provider.
    pub fn set(&mut self, provider: impl Into<String>, value: AuthValue) {
        self.credentials.insert(provider.into(), value);
    }

    /// Get a credential for a provider.
    pub fn get(&self, provider: &str) -> Option<&AuthValue> {
        self.credentials.get(provider)
    }

    /// Resolve a credential to its string value.
    pub fn resolve(&self, provider: &str) -> Result<String, RociError> {
        self.get(provider)
            .ok_or_else(|| {
                RociError::Authentication(format!("No credentials for provider: {provider}"))
            })?
            .resolve()
    }

    /// Default credential file path (~/.roci/credentials.json).
    pub fn default_credential_path() -> PathBuf {
        dirs_path().join("credentials.json")
    }

    /// Load credentials from the default credential file path.
    pub fn load_default() -> Result<Self, RociError> {
        Self::load_from_path(Self::default_credential_path())
    }

    /// Load credentials from a specific path.
    ///
    /// Returns an empty manager if the file does not exist.
    pub fn load_from_path(path: impl AsRef<Path>) -> Result<Self, RociError> {
        let path = path.as_ref();
        let raw = match fs::read_to_string(path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(err) => return Err(RociError::Io(err)),
        };

        let credential_file: CredentialFile = serde_json::from_str(&raw)?;
        if credential_file.version != CREDENTIAL_FILE_VERSION {
            return Err(RociError::Configuration(format!(
                "Unsupported credentials file version {} at {}",
                credential_file.version,
                path.display()
            )));
        }

        Ok(Self {
            credentials: credential_file.credentials.into_iter().collect(),
        })
    }

    /// Save credentials to the default credential file path.
    pub fn save_default(&self) -> Result<(), RociError> {
        self.save_to_path(Self::default_credential_path())
    }

    /// Save credentials to a specific path.
    pub fn save_to_path(&self, path: impl AsRef<Path>) -> Result<(), RociError> {
        let credentials = self
            .credentials
            .iter()
            .map(|(provider, value)| (provider.clone(), value.clone()))
            .collect::<BTreeMap<_, _>>();
        let credential_file = CredentialFile {
            version: CREDENTIAL_FILE_VERSION,
            credentials,
        };
        let serialized = serde_json::to_vec_pretty(&credential_file)?;
        atomic_write(path.as_ref(), &serialized)
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialFile {
    version: u32,
    credentials: BTreeMap<String, AuthValue>,
}

fn dirs_path() -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".roci")
    } else {
        PathBuf::from(".roci")
    }
}

fn atomic_write(path: &Path, data: &[u8]) -> Result<(), RociError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }

    let file_name = path.file_name().ok_or_else(|| {
        RociError::Configuration(format!(
            "Credential path {} has no file name",
            path.display()
        ))
    })?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temp_name = format!(
        ".{}.tmp-{}-{nonce}",
        file_name.to_string_lossy(),
        std::process::id()
    );
    let temp_path = path.with_file_name(temp_name);

    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);

    let write_result = (|| -> std::io::Result<()> {
        let mut temp_file = options.open(&temp_path)?;
        temp_file.write_all(data)?;
        temp_file.sync_all()?;
        Ok(())
    })();

    if let Err(err) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(RociError::Io(err));
    }

    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(RociError::Io(err));
    }

    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;

    Ok(())
}
