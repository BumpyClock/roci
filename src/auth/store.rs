use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::error::AuthError;
use super::token::Token;

/// Storage abstraction for persisted OAuth tokens.
pub trait TokenStore: Send + Sync {
    fn load(&self, provider: &str, profile: &str) -> Result<Option<Token>, AuthError>;
    fn save(&self, provider: &str, profile: &str, token: &Token) -> Result<(), AuthError>;
    fn clear(&self, provider: &str, profile: &str) -> Result<(), AuthError>;
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

/// File-backed token store using TOML files.
///
/// # Example
/// ```no_run
/// use roci::auth::{FileTokenStore, Token, TokenStore};
/// use chrono::{DateTime, Utc};
///
/// let store = FileTokenStore::new_default();
/// let token = Token {
///     access_token: "access".to_string(),
///     refresh_token: Some("refresh".to_string()),
///     id_token: None,
///     expires_at: None,
///     last_refresh: Some(DateTime::<Utc>::from(std::time::SystemTime::now())),
///     scopes: None,
///     account_id: None,
/// };
/// store.save("openai-codex", "default", &token)?;
/// # Ok::<(), roci::auth::AuthError>(())
/// ```
#[derive(Debug, Clone)]
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

    fn ensure_parent(path: &Path) -> Result<(), AuthError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self, provider: &str, profile: &str) -> Result<Option<Token>, AuthError> {
        let path = self.token_path(provider, profile);
        let raw = match fs::read_to_string(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(AuthError::Io(err.to_string())),
        };
        let file: TokenFile = toml::from_str(&raw)?;
        Ok(Some(file.token))
    }

    fn save(&self, provider: &str, profile: &str, token: &Token) -> Result<(), AuthError> {
        let path = self.token_path(provider, profile);
        Self::ensure_parent(&path)?;
        let file = TokenFile {
            version: 1,
            provider: provider.to_string(),
            profile: profile.to_string(),
            token: token.clone(),
            saved_at: DateTime::<Utc>::from(std::time::SystemTime::now()),
        };
        let serialized = toml::to_string(&file)?;
        fs::write(&path, serialized)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn clear(&self, provider: &str, profile: &str) -> Result<(), AuthError> {
        let path = self.token_path(provider, profile);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AuthError::Io(err.to_string())),
        }
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
}
