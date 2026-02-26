use std::path::PathBuf;
use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use roci_core::auth::AuthError;
use roci_core::auth::Token;
use roci_core::auth::TokenStore;

const CLAUDE_CLI_REL_PATH: &str = ".claude/.credentials.json";
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_AUTHORIZE_URL: &str = "https://claude.ai/oauth/authorize";
const CLAUDE_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const CLAUDE_REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const CLAUDE_SCOPES: &str = "org:create_api_key user:profile user:inference";
const REFRESH_GRACE_PERIOD_MINUTES: i64 = 5;

/// PKCE authorization session returned by [`ClaudeCodeAuth::start_auth`].
///
/// The caller opens `authorize_url` in a browser and pastes the callback
/// response back into [`ClaudeCodeAuth::exchange_code`].
///
/// # Example
/// ```no_run
/// use roci_providers::auth::claude_code::PkceSession;
///
/// let session = PkceSession {
///     authorize_url: "https://claude.ai/oauth/authorize?...".to_string(),
///     state: "abc123".to_string(),
///     code_verifier: "verifier-value".to_string(),
/// };
/// ```
#[derive(Debug, Clone)]
pub struct PkceSession {
    pub authorize_url: String,
    pub state: String,
    pub code_verifier: String,
}

/// Claude Code credential importer and OAuth PKCE authenticator.
///
/// Supports two authentication strategies:
/// 1. **File import** — reads existing tokens from `~/.claude/.credentials.json`
/// 2. **Interactive PKCE** — authorization-code flow with browser redirect
///
/// # Example
/// ```no_run
/// use std::sync::Arc;
/// use roci_core::auth::{FileTokenStore, TokenStoreConfig};
/// use roci_providers::auth::claude_code::ClaudeCodeAuth;
///
/// let store = FileTokenStore::new(TokenStoreConfig::new(std::path::PathBuf::from("/tmp")));
/// let auth = ClaudeCodeAuth::new(Arc::new(store));
/// # Ok::<(), roci_core::auth::AuthError>(())
/// ```
pub struct ClaudeCodeAuth {
    client: reqwest::Client,
    token_store: Arc<dyn TokenStore>,
    profile: String,
    token_url: String,
}

impl ClaudeCodeAuth {
    pub fn new(token_store: Arc<dyn TokenStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            token_store,
            profile: "default".to_string(),
            token_url: CLAUDE_TOKEN_URL.to_string(),
        }
    }

    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = profile.into();
        self
    }

    pub fn with_token_url(mut self, url: impl Into<String>) -> Self {
        self.token_url = url.into();
        self
    }

    pub async fn logged_in(&self) -> Result<bool, AuthError> {
        Ok(self
            .token_store
            .load("claude-code", &self.profile)?
            .is_some())
    }

    pub async fn get_token(&self) -> Result<Token, AuthError> {
        let mut token = self
            .token_store
            .load("claude-code", &self.profile)?
            .ok_or(AuthError::NotLoggedIn)?;
        if needs_refresh(&token) && token.refresh_token.is_some() {
            let refreshed = self.refresh_token(&token).await?;
            self.token_store
                .save("claude-code", &self.profile, &refreshed)?;
            token = refreshed;
        }
        Ok(token)
    }

    /// Begin an interactive PKCE authorization flow.
    ///
    /// Generates a cryptographic `state` and PKCE `code_verifier`, then
    /// builds the authorize URL. The caller should open `authorize_url` in
    /// a browser and later call [`exchange_code`](Self::exchange_code) with
    /// the response.
    pub fn start_auth(&self) -> Result<PkceSession, AuthError> {
        let state = random_hex(32);
        let code_verifier = generate_code_verifier();
        let code_challenge = compute_code_challenge(&code_verifier);

        let params = [
            ("client_id", CLAUDE_CLIENT_ID),
            ("redirect_uri", CLAUDE_REDIRECT_URI),
            ("response_type", "code"),
            ("scope", CLAUDE_SCOPES),
            ("state", &state),
            ("code_challenge", &code_challenge),
            ("code_challenge_method", "S256"),
        ];

        let authorize_url = build_url_with_params(CLAUDE_AUTHORIZE_URL, &params);

        Ok(PkceSession {
            authorize_url,
            state,
            code_verifier,
        })
    }

    /// Exchange an authorization code for tokens.
    ///
    /// `auth_response` may be `"code#state"` (from the redirect) or just
    /// the bare `"code"`. When state is present it is verified against the
    /// session.
    pub async fn exchange_code(
        &self,
        session: &PkceSession,
        auth_response: &str,
    ) -> Result<Token, AuthError> {
        let (code, state) = parse_auth_response(auth_response);
        if let Some(returned_state) = state {
            if returned_state != session.state {
                return Err(AuthError::InvalidResponse(format!(
                    "OAuth state mismatch: expected {}, got {returned_state}",
                    session.state
                )));
            }
        }

        let resp = self
            .client
            .post(&self.token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", CLAUDE_CLIENT_ID),
                ("code", code),
                ("redirect_uri", CLAUDE_REDIRECT_URI),
                ("code_verifier", session.code_verifier.as_str()),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(AuthError::InvalidResponse(format!(
                "Token exchange failed with status {}",
                resp.status()
            )));
        }

        let payload: TokenExchangeResponse = resp.json().await?;
        let token = token_from_exchange_response(payload);
        self.token_store
            .save("claude-code", &self.profile, &token)?;
        Ok(token)
    }

    /// Refresh an expired token using its refresh_token.
    pub async fn refresh_token(&self, token: &Token) -> Result<Token, AuthError> {
        let refresh_token = token
            .refresh_token
            .as_ref()
            .ok_or(AuthError::ExpiredOrInvalidGrant)?;

        let resp = self
            .client
            .post(&self.token_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", CLAUDE_CLIENT_ID),
                ("refresh_token", refresh_token.as_str()),
            ])
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(AuthError::InvalidResponse(format!(
                "Token refresh failed with status {}",
                resp.status()
            )));
        }

        let payload: TokenExchangeResponse = resp.json().await?;
        let refreshed = token_from_exchange_response(payload);
        self.token_store
            .save("claude-code", &self.profile, &refreshed)?;
        Ok(refreshed)
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

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct TokenExchangeResponse {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: Option<i64>,
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

fn needs_refresh(token: &Token) -> bool {
    let Some(expires_at) = token.expires_at else {
        return false;
    };
    let grace = Duration::minutes(REFRESH_GRACE_PERIOD_MINUTES);
    Utc::now() >= expires_at - grace
}

fn token_from_exchange_response(payload: TokenExchangeResponse) -> Token {
    let expires_at = payload
        .expires_in
        .map(|secs| Utc::now() + Duration::seconds(secs));
    Token {
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        id_token: None,
        expires_at,
        last_refresh: Some(Utc::now()),
        scopes: None,
        account_id: None,
    }
}

fn parse_auth_response(input: &str) -> (&str, Option<&str>) {
    match input.split_once('#') {
        Some((code, state)) => (code, Some(state)),
        None => (input, None),
    }
}

fn random_hex(byte_count: usize) -> String {
    let mut buf = vec![0u8; byte_count];
    for chunk in buf.chunks_mut(16) {
        let id = uuid::Uuid::new_v4();
        let bytes = id.as_bytes();
        let len = chunk.len().min(16);
        chunk[..len].copy_from_slice(&bytes[..len]);
    }
    hex_encode(&buf)
}

fn generate_code_verifier() -> String {
    let mut buf = [0u8; 32];
    for chunk in buf.chunks_mut(16) {
        let id = uuid::Uuid::new_v4();
        let bytes = id.as_bytes();
        let len = chunk.len().min(16);
        chunk[..len].copy_from_slice(&bytes[..len]);
    }
    URL_SAFE_NO_PAD.encode(buf)
}

fn compute_code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(digest)
}

fn build_url_with_params(base: &str, params: &[(&str, &str)]) -> String {
    let mut url = base.to_string();
    url.push('?');
    for (i, (key, value)) in params.iter().enumerate() {
        if i > 0 {
            url.push('&');
        }
        url.push_str(&urlencoded(key));
        url.push('=');
        url.push_str(&urlencoded(value));
    }
    url
}

fn urlencoded(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

fn hex_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() * 2);
    for byte in data {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn user_home_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}
