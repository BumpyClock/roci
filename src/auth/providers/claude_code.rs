use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::auth::error::AuthError;
use crate::auth::store::TokenStore;
use crate::auth::token::Token;

const CLAUDE_CLI_REL_PATH: &str = ".claude/.credentials.json";

/// Claude Code credential importer (file-based).
///
/// # Example
/// ```no_run
/// use std::sync::Arc;
/// use roci::auth::{FileTokenStore, TokenStoreConfig};
/// use roci::auth::providers::claude_code::ClaudeCodeAuth;
///
/// let store = FileTokenStore::new(TokenStoreConfig::new(std::path::PathBuf::from("/tmp")));
/// let auth = ClaudeCodeAuth::new(Arc::new(store));
/// # Ok::<(), roci::auth::AuthError>(())
/// ```
pub struct ClaudeCodeAuth {
    token_store: Arc<dyn TokenStore>,
    profile: String,
}

impl ClaudeCodeAuth {
    pub fn new(token_store: Arc<dyn TokenStore>) -> Self {
        Self {
            token_store,
            profile: "default".to_string(),
        }
    }

    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = profile.into();
        self
    }

    pub async fn logged_in(&self) -> Result<bool, AuthError> {
        Ok(self
            .token_store
            .load("claude-code", &self.profile)?
            .is_some())
    }

    pub async fn get_token(&self) -> Result<Token, AuthError> {
        self.token_store
            .load("claude-code", &self.profile)?
            .ok_or(AuthError::NotLoggedIn)
    }

    pub fn import_cli_credentials(
        &self,
        home_dir: Option<PathBuf>,
    ) -> Result<Option<Token>, AuthError> {
        let base = home_dir.unwrap_or_else(user_home_dir);
        let path = base.join(CLAUDE_CLI_REL_PATH);
        let raw = match std::fs::read_to_string(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(AuthError::Io(err.to_string())),
        };
        let payload: ClaudeCredentialsFile = serde_json::from_str(&raw)?;
        let oauth = match payload.claude_ai_oauth {
            Some(value) => value,
            None => return Ok(None),
        };
        let expires_at = DateTime::<Utc>::from(std::time::UNIX_EPOCH)
            + chrono::Duration::seconds(oauth.expires_at / 1000);
        let token = Token {
            access_token: oauth.access_token,
            refresh_token: oauth.refresh_token,
            id_token: None,
            expires_at: Some(expires_at),
            last_refresh: Some(Utc::now()),
            scopes: None,
            account_id: None,
        };
        self.token_store
            .save("claude-code", &self.profile, &token)?;
        Ok(Some(token))
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeCredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeOauthPayload>,
}

#[derive(Debug, Deserialize)]
struct ClaudeOauthPayload {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: i64,
}

fn user_home_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}
