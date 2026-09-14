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
const CLAUDE_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_REDIRECT_URI: &str = "https://console.anthropic.com/oauth/code/callback";
const CLAUDE_SCOPES: &str = "org:create_api_key user:profile user:inference";

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
#[derive(Clone)]
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

    /// Load the current credential, coordinating renewal with provider calls and logout.
    pub async fn get_token(&self) -> Result<Token, AuthError> {
        let auth = self.clone();
        let refresh: super::runtime::Refresh = Arc::new(move |token| {
            let auth = auth.clone();
            Box::pin(async move {
                auth.refresh_token(&token.ok_or(AuthError::NotLoggedIn)?)
                    .await
            })
        });
        super::runtime::OAuthSession::new(self.token_store.clone(), "claude-code", refresh, false)
            .with_profile(&self.profile)
            .token(None)
            .await
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
    /// `auth_response` may be the full hosted callback URL, `"code#state"`,
    /// or a bare code. Returned state is checked against the pending session;
    /// the session state is always included in the token exchange.
    pub async fn exchange_code(
        &self,
        session: &PkceSession,
        auth_response: &str,
    ) -> Result<Token, AuthError> {
        let code = parse_auth_response(auth_response, &session.state)?;
        if session.state.is_empty() || session.code_verifier.is_empty() {
            return Err(AuthError::InvalidResponse("Incomplete PKCE session".into()));
        }

        let resp = self
            .client
            .post(&self.token_url)
            .header("Accept", "application/json")
            .json(&serde_json::json!({
                "grant_type": "authorization_code",
                "client_id": CLAUDE_CLIENT_ID,
                "code": code,
                "redirect_uri": CLAUDE_REDIRECT_URI,
                "code_verifier": session.code_verifier,
                "state": session.state,
            }))
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
            .header("Accept", "application/json")
            .json(&serde_json::json!({
                "grant_type": "refresh_token",
                "client_id": CLAUDE_CLIENT_ID,
                "refresh_token": refresh_token,
            }))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Err(AuthError::InvalidResponse(format!(
                "Token refresh failed with status {}",
                resp.status()
            )));
        }

        let payload: TokenExchangeResponse = resp.json().await?;
        let mut refreshed = token_from_exchange_response(payload);
        if refreshed.access_token.trim().is_empty() {
            return Err(AuthError::InvalidResponse(
                "empty refreshed access token".into(),
            ));
        }
        refreshed.refresh_token = refreshed
            .refresh_token
            .filter(|value| !value.is_empty())
            .or_else(|| token.refresh_token.clone());
        refreshed.account_id = token.account_id.clone();
        refreshed.provider_metadata = token.provider_metadata.clone();
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
            provider_metadata: None,
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

fn token_from_exchange_response(payload: TokenExchangeResponse) -> Token {
    let expires_at = payload
        .expires_in
        .map(|secs| Utc::now() + Duration::seconds(secs));
    Token {
        provider_metadata: None,
        access_token: payload.access_token,
        refresh_token: payload.refresh_token,
        id_token: None,
        expires_at,
        last_refresh: Some(Utc::now()),
        scopes: None,
        account_id: None,
    }
}

fn parse_auth_response(input: &str, expected_state: &str) -> Result<String, AuthError> {
    let invalid = || AuthError::InvalidResponse("Invalid OAuth callback response".into());
    let input = input.trim();
    let (code, state) = if input.contains("://") {
        let url = reqwest::Url::parse(input).map_err(|_| invalid())?;
        let redirect = reqwest::Url::parse(CLAUDE_REDIRECT_URI).expect("valid hosted callback");
        if url.origin() != redirect.origin()
            || url.path() != redirect.path()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid());
        }
        let params: Vec<_> = url.query_pairs().collect();
        let values = |key: &str| {
            params
                .iter()
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value.to_string())
                .collect::<Vec<_>>()
        };
        let codes = values("code");
        let states = values("state");
        if codes.len() != 1 || states.len() != 1 || !values("error").is_empty() {
            return Err(invalid());
        }
        (codes[0].clone(), Some(states[0].clone()))
    } else {
        match input.split_once('#') {
            Some((code, state)) => (code.to_owned(), Some(state.to_owned())),
            None => (input.to_owned(), None),
        }
    };
    if code.trim().is_empty() {
        return Err(invalid());
    }
    if state
        .as_deref()
        .is_some_and(|state| state != expected_state)
    {
        return Err(AuthError::InvalidResponse("OAuth state mismatch".into()));
    }
    Ok(code)
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

#[cfg(test)]
mod tests {
    use super::*;
    use roci_core::auth::{FileTokenStore, TokenStoreConfig};
    use serde_json::json;
    use wiremock::{
        matchers::{body_json, header, method},
        Mock, MockServer, ResponseTemplate,
    };

    fn fixture() -> (tempfile::TempDir, Arc<dyn TokenStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        (dir, store)
    }

    #[tokio::test]
    async fn exchange_sends_json_with_session_state_and_parsed_callback() {
        let (_dir, store) = fixture();
        let server = MockServer::start().await;
        let auth = ClaudeCodeAuth::new(store.clone()).with_token_url(server.uri());
        let session = auth.start_auth().unwrap();
        Mock::given(method("POST"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({
                "grant_type": "authorization_code",
                "client_id": CLAUDE_CLIENT_ID,
                "code": "test-code",
                "redirect_uri": CLAUDE_REDIRECT_URI,
                "code_verifier": session.code_verifier,
                "state": session.state,
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "test-access", "refresh_token": "test-refresh", "expires_in": 3600
            })))
            .expect(3)
            .mount(&server)
            .await;
        for response in [
            "test-code".to_string(),
            format!("test-code#{}", session.state),
            format!(
                "{CLAUDE_REDIRECT_URI}?code=test-code&state={}",
                session.state
            ),
        ] {
            auth.exchange_code(&session, &response).await.unwrap();
        }
        assert_eq!(
            store
                .load("claude-code", "default")
                .unwrap()
                .unwrap()
                .access_token,
            "test-access"
        );
    }

    #[tokio::test]
    async fn invalid_callback_is_rejected_without_network_or_secret_echo() {
        let (_dir, store) = fixture();
        let server = MockServer::start().await;
        let auth = ClaudeCodeAuth::new(store).with_token_url(server.uri());
        let session = auth.start_auth().unwrap();
        for response in [
            "test-code#wrong-state".to_string(),
            format!("{CLAUDE_REDIRECT_URI}?code=test-code&state=wrong-state"),
            format!(
                "{CLAUDE_REDIRECT_URI}?code=test-code&state={}&state=wrong",
                session.state
            ),
            format!("{CLAUDE_REDIRECT_URI}?code=&state={}", session.state),
            format!(
                "https://evil.test/callback?code=test-code&state={}",
                session.state
            ),
            format!("{CLAUDE_REDIRECT_URI}?code=test-code"),
            String::new(),
        ] {
            let error = auth.exchange_code(&session, &response).await.unwrap_err();
            assert!(!error.to_string().contains("test-code"));
            assert!(!error.to_string().contains(&session.state));
        }
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn refresh_uses_json_and_retains_omitted_rotation_fields() {
        let (_dir, store) = fixture();
        let server = MockServer::start().await;
        let auth = ClaudeCodeAuth::new(store).with_token_url(server.uri());
        Mock::given(method("POST"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({
                "grant_type": "refresh_token", "client_id": CLAUDE_CLIENT_ID,
                "refresh_token": "test-refresh"
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "renewed-access", "expires_in": 3600
            })))
            .expect(1)
            .mount(&server)
            .await;
        let token = token_from_exchange_response(TokenExchangeResponse {
            access_token: "old-access".into(),
            refresh_token: Some("test-refresh".into()),
            expires_in: Some(3600),
        });
        let refreshed = auth.refresh_token(&token).await.unwrap();
        assert_eq!(refreshed.access_token, "renewed-access");
        assert_eq!(refreshed.refresh_token, token.refresh_token);
    }
}
