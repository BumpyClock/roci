use std::path::PathBuf;
use std::sync::Arc;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};

use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use roci_core::auth::AuthError;
use roci_core::auth::AuthPollResult;
use roci_core::auth::DeviceCodeSession;
use roci_core::auth::Token;
use roci_core::auth::TokenStore;

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const DEFAULT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_REFRESH_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const BROWSER_REDIRECT_URI: &str = "http://localhost:1455/auth/callback";

/// Secret browser-login session material; retain inside the host auth manager.
#[derive(Clone, Serialize, Deserialize)]
pub struct CodexPkceSession {
    pub authorize_url: String,
    pub state: String,
    pub code_verifier: String,
    pub redirect_uri: String,
}

/// OpenAI Codex OAuth device-code auth helper.
///
/// # Example
/// ```no_run
/// use std::sync::Arc;
/// use roci_core::auth::{FileTokenStore, TokenStoreConfig};
/// use roci_providers::auth::openai_codex::OpenAiCodexAuth;
///
/// let store = FileTokenStore::new(TokenStoreConfig::new(std::path::PathBuf::from("/tmp")));
/// let auth = OpenAiCodexAuth::new(Arc::new(store));
/// # Ok::<(), roci_core::auth::AuthError>(())
/// ```
#[derive(Clone)]
pub struct OpenAiCodexAuth {
    client: reqwest::Client,
    issuer: String,
    client_id: String,
    refresh_token_url_override: Option<String>,
    token_store: Arc<dyn TokenStore>,
    profile: String,
}

impl OpenAiCodexAuth {
    pub fn new(token_store: Arc<dyn TokenStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            issuer: DEFAULT_ISSUER.to_string(),
            client_id: DEFAULT_CLIENT_ID.to_string(),
            refresh_token_url_override: None,
            token_store,
            profile: "default".to_string(),
        }
    }

    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = profile.into();
        self
    }

    pub fn with_issuer(mut self, issuer: impl Into<String>) -> Self {
        self.issuer = issuer.into();
        self
    }

    pub fn with_refresh_token_url_override(mut self, url: impl Into<String>) -> Self {
        self.refresh_token_url_override = Some(url.into());
        self
    }

    pub async fn logged_in(&self) -> Result<bool, AuthError> {
        Ok(self
            .token_store
            .load("openai-codex", &self.profile)?
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
        super::runtime::OAuthSession::new(self.token_store.clone(), "openai-codex", refresh, false)
            .with_profile(&self.profile)
            .token(None)
            .await
    }

    pub async fn start_device_code(&self) -> Result<DeviceCodeSession, AuthError> {
        let url = format!(
            "{}/api/accounts/deviceauth/usercode",
            self.issuer.trim_end_matches('/')
        );
        let resp = self
            .client
            .post(url)
            .json(&UserCodeRequest {
                client_id: self.client_id.clone(),
            })
            .send()
            .await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Err(AuthError::Unsupported(
                "Device code login not enabled for issuer".to_string(),
            ));
        }
        if !resp.status().is_success() {
            return Err(AuthError::InvalidResponse(format!(
                "Device code request failed with status {}",
                resp.status()
            )));
        }
        let payload: UserCodeResponse = resp.json().await?;
        let expires_at = Utc::now() + Duration::minutes(15);
        Ok(DeviceCodeSession {
            provider: "openai-codex".to_string(),
            verification_url: format!("{}/codex/device", self.issuer.trim_end_matches('/')),
            user_code: payload.user_code,
            device_code: payload.device_auth_id,
            interval_secs: payload.interval,
            expires_at,
        })
    }

    /// Create a separate browser authorization session without importing CLI credentials.
    pub fn start_browser_login(&self) -> Result<CodexPkceSession, AuthError> {
        let mut randomness = Vec::with_capacity(32);
        randomness.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        randomness.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        let code_verifier = URL_SAFE_NO_PAD.encode(randomness);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()));
        let state = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let mut url = reqwest::Url::parse(&format!(
            "{}/oauth/authorize",
            self.issuer.trim_end_matches('/')
        ))
        .map_err(|_| AuthError::InvalidResponse("invalid Codex issuer URL".into()))?;
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.client_id.as_str()),
            ("response_type", "code"),
            ("redirect_uri", BROWSER_REDIRECT_URI),
            ("scope", "openid email profile offline_access"),
            ("state", state.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("prompt", "login"),
            ("id_token_add_organizations", "true"),
            ("codex_cli_simplified_flow", "true"),
        ]);
        Ok(CodexPkceSession {
            authorize_url: url.into(),
            state,
            code_verifier,
            redirect_uri: BROWSER_REDIRECT_URI.into(),
        })
    }

    /// Complete a loopback or manually pasted full callback URL, validating state first.
    pub async fn complete_browser_login(
        &self,
        session: &CodexPkceSession,
        callback: &str,
    ) -> Result<Token, AuthError> {
        let url = reqwest::Url::parse(callback.trim()).map_err(|_| {
            AuthError::InvalidResponse(
                "paste the complete Codex callback URL, including code and state".into(),
            )
        })?;
        let expected = reqwest::Url::parse(&session.redirect_uri)
            .map_err(|_| AuthError::InvalidResponse("invalid Codex redirect URI".into()))?;
        if url.scheme() != expected.scheme()
            || url.host_str() != expected.host_str()
            || url.port_or_known_default() != expected.port_or_known_default()
            || url.path() != expected.path()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(AuthError::InvalidResponse(
                "Codex callback URI does not match this login".into(),
            ));
        }
        let pairs: Vec<_> = url.query_pairs().collect();
        let states: Vec<_> = pairs.iter().filter(|(key, _)| key == "state").collect();
        if states.len() != 1 || states[0].1 != session.state {
            return Err(AuthError::InvalidResponse(
                "Codex callback state mismatch".into(),
            ));
        }
        if pairs.iter().any(|(key, _)| key == "error") {
            return Err(AuthError::ExpiredOrInvalidGrant);
        }
        let codes: Vec<_> = pairs.iter().filter(|(key, _)| key == "code").collect();
        if codes.len() != 1 || codes[0].1.trim().is_empty() || session.code_verifier.is_empty() {
            return Err(AuthError::InvalidResponse(
                "Codex callback is missing a valid authorization code".into(),
            ));
        }
        let token = self
            .exchange_code_for_tokens_with_redirect(
                &codes[0].1,
                &session.code_verifier,
                &session.redirect_uri,
            )
            .await?;
        self.token_store
            .save("openai-codex", &self.profile, &token)?;
        Ok(token)
    }

    pub async fn poll_device_code(
        &self,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        if Utc::now() >= session.expires_at {
            return Ok(AuthPollResult::Expired);
        }
        let url = format!(
            "{}/api/accounts/deviceauth/token",
            self.issuer.trim_end_matches('/')
        );
        let resp = self
            .client
            .post(url)
            .json(&DeviceTokenRequest {
                device_auth_id: session.device_code.clone(),
                user_code: session.user_code.clone(),
            })
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            let payload: DeviceTokenResponse = resp.json().await?;
            let token = self
                .exchange_code_for_tokens(&payload.authorization_code, &payload.code_verifier)
                .await?;
            self.token_store
                .save("openai-codex", &self.profile, &token)?;
            return Ok(AuthPollResult::Authorized { token });
        }
        if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
            return Ok(AuthPollResult::Pending);
        }
        Err(AuthError::InvalidResponse(format!(
            "Device code poll failed with status {}",
            status
        )))
    }

    pub fn import_codex_auth_json(
        &self,
        codex_home: Option<PathBuf>,
    ) -> Result<Option<Token>, AuthError> {
        let home = codex_home.unwrap_or_else(default_codex_home);
        let path = home.join("auth.json");
        let raw = match std::fs::read_to_string(&path) {
            Ok(data) => data,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(AuthError::Io(err.to_string())),
        };
        let auth: CodexAuthJson = serde_json::from_str(&raw)?;
        let tokens = match auth.tokens {
            Some(tokens) => tokens,
            None => return Ok(None),
        };
        let token = Token {
            provider_metadata: None,
            expires_at: jwt_expiry(&tokens.access_token),
            access_token: tokens.access_token,
            refresh_token: Some(tokens.refresh_token),
            id_token: tokens.id_token,
            last_refresh: auth.last_refresh,
            scopes: None,
            account_id: tokens.account_id,
        };
        self.token_store
            .save("openai-codex", &self.profile, &token)?;
        Ok(Some(token))
    }

    async fn exchange_code_for_tokens(
        &self,
        authorization_code: &str,
        code_verifier: &str,
    ) -> Result<Token, AuthError> {
        let redirect_uri = format!("{}/deviceauth/callback", self.issuer.trim_end_matches('/'));
        self.exchange_code_for_tokens_with_redirect(
            authorization_code,
            code_verifier,
            &redirect_uri,
        )
        .await
    }

    async fn exchange_code_for_tokens_with_redirect(
        &self,
        authorization_code: &str,
        code_verifier: &str,
        redirect_uri: &str,
    ) -> Result<Token, AuthError> {
        let url = format!("{}/oauth/token", self.issuer.trim_end_matches('/'));
        let resp = self
            .client
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", authorization_code),
                ("redirect_uri", redirect_uri),
                ("client_id", &self.client_id),
                ("code_verifier", code_verifier),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(AuthError::InvalidResponse(format!(
                "Token exchange failed with status {}",
                resp.status()
            )));
        }
        let payload: TokenResponse = resp.json().await?;
        if payload.access_token.trim().is_empty() {
            return Err(AuthError::InvalidResponse(
                "Codex token exchange returned an empty access token".into(),
            ));
        }
        Ok(Token {
            provider_metadata: None,
            expires_at: token_expiry(payload.expires_in, &payload.access_token),
            access_token: payload.access_token,
            refresh_token: Some(payload.refresh_token),
            id_token: Some(payload.id_token),
            last_refresh: Some(Utc::now()),
            scopes: None,
            account_id: None,
        })
    }

    pub async fn refresh_token(&self, token: &Token) -> Result<Token, AuthError> {
        let refresh_token = token
            .refresh_token
            .as_ref()
            .filter(|value| !value.trim().is_empty())
            .ok_or(AuthError::ExpiredOrInvalidGrant)?;
        let endpoint = self
            .refresh_token_url_override
            .clone()
            .unwrap_or_else(|| DEFAULT_REFRESH_ENDPOINT.to_string());
        let resp = self
            .client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .json(&RefreshRequest {
                client_id: self.client_id.clone(),
                grant_type: "refresh_token".to_string(),
                refresh_token: refresh_token.to_string(),
                scope: "openid profile email".to_string(),
            })
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            let payload: RefreshResponse = resp.json().await?;
            if payload.access_token.trim().is_empty() {
                return Err(AuthError::InvalidResponse(
                    "empty refreshed access token".into(),
                ));
            }
            return Ok(Token {
                provider_metadata: token.provider_metadata.clone(),
                expires_at: token_expiry(payload.expires_in, &payload.access_token),
                access_token: payload.access_token,
                refresh_token: payload
                    .refresh_token
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| token.refresh_token.clone()),
                id_token: payload
                    .id_token
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| token.id_token.clone()),
                last_refresh: Some(Utc::now()),
                scopes: token.scopes.clone(),
                account_id: token.account_id.clone(),
            });
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after_ms = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| seconds.saturating_mul(1000));
            return Err(AuthError::RateLimited { retry_after_ms });
        }
        if status.is_server_error() {
            return Err(AuthError::Network(format!(
                "Codex refresh temporarily unavailable ({status})"
            )));
        }
        let body = resp.text().await?;
        let code = extract_refresh_error_code(&body);
        if status == StatusCode::UNAUTHORIZED
            || matches!(
                code.as_deref(),
                Some(
                    "invalid_grant"
                        | "refresh_token_expired"
                        | "refresh_token_reused"
                        | "refresh_token_invalidated"
                )
            )
        {
            return Err(AuthError::ExpiredOrInvalidGrant);
        }
        Err(AuthError::InvalidResponse(format!(
            "Refresh token failed with status {status}"
        )))
    }
}

#[derive(Debug, Deserialize)]
struct UserCodeResponse {
    device_auth_id: String,
    #[serde(alias = "user_code", alias = "usercode")]
    user_code: String,
    #[serde(default, deserialize_with = "deserialize_interval")]
    interval: u64,
}

#[derive(Debug, Serialize)]
struct UserCodeRequest {
    client_id: String,
}

#[derive(Debug, Serialize)]
struct DeviceTokenRequest {
    device_auth_id: String,
    user_code: String,
}

#[derive(Debug, Deserialize)]
struct DeviceTokenResponse {
    authorization_code: String,
    code_verifier: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    id_token: String,
    access_token: String,
    refresh_token: String,
    expires_in: Option<i64>,
}

#[derive(Debug, Serialize)]
struct RefreshRequest {
    client_id: String,
    grant_type: String,
    refresh_token: String,
    scope: String,
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
}

fn token_expiry(expires_in: Option<i64>, access_token: &str) -> Option<DateTime<Utc>> {
    expires_in
        .and_then(|seconds| Utc::now().checked_add_signed(Duration::seconds(seconds)))
        .or_else(|| jwt_expiry(access_token))
}

// Unverified JWT expiry is a scheduling hint, never evidence of identity or authorization.
fn jwt_expiry(token: &str) -> Option<DateTime<Utc>> {
    use base64::Engine;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    DateTime::from_timestamp(claims.get("exp")?.as_i64()?, 0)
}

#[derive(Debug, Deserialize)]
struct CodexAuthJson {
    tokens: Option<CodexTokens>,
    last_refresh: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
struct CodexTokens {
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    account_id: Option<String>,
}

fn extract_refresh_error_code(body: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            v.get("error")
                .and_then(|e| {
                    e.as_str()
                        .or_else(|| e.get("code").and_then(|c| c.as_str()))
                        .or_else(|| e.get("type").and_then(|c| c.as_str()))
                })
                .map(|s| s.to_string())
        })
}

fn deserialize_interval<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    s.trim()
        .parse::<u64>()
        .map_err(|e| serde::de::Error::custom(format!("invalid u64 string: {e}")))
}

fn default_codex_home() -> PathBuf {
    if let Some(value) = std::env::var_os("CODEX_HOME") {
        let path = PathBuf::from(value);
        if !path.as_os_str().is_empty() {
            return path;
        }
    }
    let base = directories::UserDirs::new()
        .map(|dirs| dirs.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(".codex")
}

#[cfg(test)]
mod browser_tests {
    use super::*;
    use roci_core::auth::{FileTokenStore, TokenStoreConfig};
    use serde_json::json;
    use wiremock::{
        matchers::{body_string_contains, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn fixture() -> (tempfile::TempDir, Arc<dyn TokenStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        (dir, store)
    }

    #[test]
    fn browser_login_uses_random_pkce_state_and_loopback_redirect() {
        let (_dir, store) = fixture();
        let auth = OpenAiCodexAuth::new(store);
        let first = auth.start_browser_login().unwrap();
        let second = auth.start_browser_login().unwrap();
        assert_ne!(first.state, second.state);
        assert_ne!(first.code_verifier, second.code_verifier);
        let url = reqwest::Url::parse(&first.authorize_url).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(query["redirect_uri"], BROWSER_REDIRECT_URI);
        assert_eq!(query["state"], first.state);
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(
            query["code_challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(first.code_verifier.as_bytes()))
        );
        assert!(!first.authorize_url.contains(&first.code_verifier));
    }

    #[tokio::test]
    async fn browser_exchange_validates_callback_then_persists_new_session() {
        let (_dir, store) = fixture();
        let server = MockServer::start().await;
        let auth = OpenAiCodexAuth::new(store.clone()).with_issuer(server.uri());
        let session = auth.start_browser_login().unwrap();
        Mock::given(method("POST")).and(path("/oauth/token"))
            .and(body_string_contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"))
            .and(body_string_contains(format!("code_verifier={}", session.code_verifier)))
            .and(body_string_contains("code=one-time-code"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token":"new-access", "refresh_token":"new-refresh", "id_token":"identity", "expires_in":3600
            }))).expect(1).mount(&server).await;
        for callback in [
            format!("{BROWSER_REDIRECT_URI}?code=one-time-code&state=wrong"),
            format!(
                "http://evil.test/auth/callback?code=one-time-code&state={}",
                session.state
            ),
            format!(
                "{BROWSER_REDIRECT_URI}?code=one-time-code&state={}&state=wrong",
                session.state
            ),
        ] {
            let error = auth
                .complete_browser_login(&session, &callback)
                .await
                .unwrap_err();
            assert!(!error.to_string().contains("one-time-code"));
        }
        assert!(store.load("openai-codex", "default").unwrap().is_none());
        let callback = format!(
            "{BROWSER_REDIRECT_URI}?code=one-time-code&state={}",
            session.state
        );
        let token = auth
            .complete_browser_login(&session, &callback)
            .await
            .unwrap();
        assert_eq!(token.access_token, "new-access");
        assert_eq!(
            store
                .load("openai-codex", "default")
                .unwrap()
                .unwrap()
                .refresh_token
                .as_deref(),
            Some("new-refresh")
        );
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    use roci_core::auth::{FileTokenStore, TokenStoreConfig};
    use serde_json::json;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn old_token() -> Token {
        Token {
            access_token: "old-access".into(),
            refresh_token: Some("old-refresh".into()),
            id_token: Some("old-identity".into()),
            expires_at: Some(Utc::now() - Duration::hours(1)),
            last_refresh: None,
            scopes: Some(vec!["openid".into()]),
            account_id: Some("account".into()),
            provider_metadata: None,
        }
    }

    fn fixture() -> (tempfile::TempDir, Arc<dyn TokenStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        (dir, store)
    }

    #[tokio::test]
    async fn refresh_preserves_identity_and_rotation_omissions_without_persisting() {
        let (_dir, store) = fixture();
        let server = MockServer::start().await;
        let old = old_token();
        store.save("openai-codex", "default", &old).unwrap();
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token":"new-access", "refresh_token":" ", "id_token":"", "expires_in":3600
            })))
            .expect(1)
            .mount(&server)
            .await;
        let auth = OpenAiCodexAuth::new(store.clone())
            .with_refresh_token_url_override(format!("{}/token", server.uri()));
        let refreshed = auth.refresh_token(&old).await.unwrap();
        assert_eq!(refreshed.access_token, "new-access");
        assert_eq!(refreshed.refresh_token, old.refresh_token);
        assert_eq!(refreshed.id_token, old.id_token);
        assert_eq!(refreshed.account_id, old.account_id);
        assert_eq!(refreshed.scopes, old.scopes);
        assert_eq!(
            store
                .load("openai-codex", "default")
                .unwrap()
                .unwrap()
                .access_token,
            "old-access"
        );
    }

    #[tokio::test]
    async fn refresh_errors_are_normalized_without_provider_secrets() {
        let (_dir, store) = fixture();
        for (status, body, kind) in [
            (400, json!({"error":"invalid_grant"}), "grant"),
            (400, json!({"error":{"code":"invalid_grant"}}), "grant"),
            (
                400,
                json!({"error":{"type":"refresh_token_reused"}}),
                "grant",
            ),
            (
                401,
                json!({"error":{"code":"sensitive-provider-detail"}}),
                "grant",
            ),
            (429, json!({"error":"sensitive-provider-detail"}), "rate"),
            (503, json!({"error":"sensitive-provider-detail"}), "network"),
            (400, json!({"error":"sensitive-provider-detail"}), "invalid"),
        ] {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("Retry-After", "2")
                        .set_body_json(body),
                )
                .expect(1)
                .mount(&server)
                .await;
            let auth =
                OpenAiCodexAuth::new(store.clone()).with_refresh_token_url_override(server.uri());
            let error = auth.refresh_token(&old_token()).await.unwrap_err();
            assert!(!error.to_string().contains("sensitive-provider-detail"));
            match kind {
                "grant" => assert!(matches!(error, AuthError::ExpiredOrInvalidGrant)),
                "rate" => assert!(matches!(
                    error,
                    AuthError::RateLimited {
                        retry_after_ms: Some(2000)
                    }
                )),
                "network" => assert!(matches!(error, AuthError::Network(_))),
                _ => assert!(matches!(error, AuthError::InvalidResponse(_))),
            }
        }
    }

    #[tokio::test]
    async fn blank_refresh_token_is_rejected_before_network_io() {
        let (_dir, store) = fixture();
        let server = MockServer::start().await;
        let auth = OpenAiCodexAuth::new(store).with_refresh_token_url_override(server.uri());
        let mut token = old_token();
        token.refresh_token = Some(" ".into());
        assert!(matches!(
            auth.refresh_token(&token).await,
            Err(AuthError::ExpiredOrInvalidGrant)
        ));
        assert!(server.received_requests().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn public_token_helpers_coordinate_refresh_and_honor_profile() {
        for key in ["openai-codex", "claude-code"] {
            let (_dir, store) = fixture();
            let server = MockServer::start().await;
            let old = old_token();
            store.save(key, "secondary", &old).unwrap();
            store.save(key, "default", &old).unwrap();
            Mock::given(method("POST")).respond_with(ResponseTemplate::new(200)
                .set_delay(std::time::Duration::from_millis(50))
                .set_body_json(json!({"access_token":"new-access", "refresh_token":"rotated-refresh", "expires_in":3600})))
                .expect(1).mount(&server).await;
            let mut requests = Vec::new();
            for _ in 0..8 {
                let store = store.clone();
                let endpoint = server.uri();
                requests.push(tokio::spawn(async move {
                    if key == "openai-codex" {
                        OpenAiCodexAuth::new(store)
                            .with_profile("secondary")
                            .with_refresh_token_url_override(endpoint)
                            .get_token()
                            .await
                    } else {
                        crate::auth::claude_code::ClaudeCodeAuth::new(store)
                            .with_profile("secondary")
                            .with_token_url(endpoint)
                            .get_token()
                            .await
                    }
                }));
            }
            for request in requests {
                assert_eq!(request.await.unwrap().unwrap().access_token, "new-access");
            }
            assert_eq!(
                store
                    .load(key, "secondary")
                    .unwrap()
                    .unwrap()
                    .refresh_token
                    .as_deref(),
                Some("rotated-refresh")
            );
            assert_eq!(
                store.load(key, "default").unwrap().unwrap().access_token,
                "old-access"
            );
        }
    }
}
