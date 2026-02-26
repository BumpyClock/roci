use std::path::PathBuf;
use std::sync::Arc;

use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use roci_core::auth::{DeviceCodePoll, DeviceCodeSession};
use roci_core::auth::AuthError;
use roci_core::auth::TokenStore;
use roci_core::auth::Token;

const DEFAULT_ISSUER: &str = "https://auth.openai.com";
const DEFAULT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const DEFAULT_REFRESH_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const TOKEN_REFRESH_INTERVAL_SECS: i64 = 8 * 24 * 60 * 60;

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

    pub async fn get_token(&self) -> Result<Token, AuthError> {
        let mut token = self
            .token_store
            .load("openai-codex", &self.profile)?
            .ok_or(AuthError::NotLoggedIn)?;
        if needs_refresh(&token) {
            let refreshed = self.refresh_token(&token).await?;
            self.token_store
                .save("openai-codex", &self.profile, &refreshed)?;
            token = refreshed;
        }
        Ok(token)
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

    pub async fn poll_device_code(
        &self,
        session: &DeviceCodeSession,
    ) -> Result<DeviceCodePoll, AuthError> {
        if Utc::now() >= session.expires_at {
            return Ok(DeviceCodePoll::Expired);
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
            return Ok(DeviceCodePoll::Authorized { token });
        }
        if status == StatusCode::FORBIDDEN || status == StatusCode::NOT_FOUND {
            return Ok(DeviceCodePoll::Pending {
                interval_secs: session.interval_secs,
            });
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
            access_token: tokens.access_token,
            refresh_token: Some(tokens.refresh_token),
            id_token: tokens.id_token,
            expires_at: None,
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
        let url = format!("{}/oauth/token", self.issuer.trim_end_matches('/'));
        let resp = self
            .client
            .post(url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("grant_type", "authorization_code"),
                ("code", authorization_code),
                ("redirect_uri", &redirect_uri),
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
        Ok(Token {
            access_token: payload.access_token,
            refresh_token: Some(payload.refresh_token),
            id_token: Some(payload.id_token),
            expires_at: None,
            last_refresh: Some(Utc::now()),
            scopes: None,
            account_id: None,
        })
    }

    async fn refresh_token(&self, token: &Token) -> Result<Token, AuthError> {
        let refresh_token = token
            .refresh_token
            .as_ref()
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
            return Ok(Token {
                access_token: payload.access_token,
                refresh_token: Some(payload.refresh_token),
                id_token: payload.id_token,
                expires_at: None,
                last_refresh: Some(Utc::now()),
                scopes: None,
                account_id: token.account_id.clone(),
            });
        }
        let body = resp.text().await.unwrap_or_default();
        if status == StatusCode::UNAUTHORIZED {
            let code = extract_refresh_error_code(&body);
            if matches!(
                code.as_deref(),
                Some("refresh_token_expired")
                    | Some("refresh_token_reused")
                    | Some("refresh_token_invalidated")
            ) {
                return Err(AuthError::ExpiredOrInvalidGrant);
            }
            return Err(AuthError::InvalidResponse(format!(
                "Refresh token rejected: {}",
                code.unwrap_or_else(|| "unknown".to_string())
            )));
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
    refresh_token: String,
    id_token: Option<String>,
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
                .and_then(|e| e.as_str())
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

fn needs_refresh(token: &Token) -> bool {
    let now = Utc::now();
    if let Some(expires_at) = token.expires_at {
        if now >= expires_at {
            return true;
        }
    }
    let last = token
        .last_refresh
        .unwrap_or_else(|| DateTime::<Utc>::from(std::time::UNIX_EPOCH));
    now - last >= Duration::seconds(TOKEN_REFRESH_INTERVAL_SECS)
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
