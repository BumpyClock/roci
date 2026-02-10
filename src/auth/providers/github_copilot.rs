use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use reqwest::StatusCode;
use serde::Deserialize;

use crate::auth::device_code::{DeviceCodePoll, DeviceCodeSession};
use crate::auth::error::AuthError;
use crate::auth::store::TokenStore;
use crate::auth::token::Token;

const DEFAULT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";
const DEFAULT_DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const DEFAULT_ACCESS_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const DEFAULT_COPILOT_TOKEN_URL: &str = "https://api.github.com/copilot_internal/v2/token";
const DEFAULT_COPILOT_BASE_URL: &str = "https://api.individual.githubcopilot.com";

/// GitHub Copilot OAuth helper with device-code flow.
///
/// # Example
/// ```no_run
/// use std::sync::Arc;
/// use roci::auth::{FileTokenStore, TokenStoreConfig};
/// use roci::auth::providers::github_copilot::GitHubCopilotAuth;
///
/// let store = FileTokenStore::new(TokenStoreConfig::new(std::path::PathBuf::from("/tmp")));
/// let auth = GitHubCopilotAuth::new(Arc::new(store));
/// # Ok::<(), roci::auth::AuthError>(())
/// ```
pub struct GitHubCopilotAuth {
    client: reqwest::Client,
    client_id: String,
    device_code_url: String,
    access_token_url: String,
    copilot_token_url: String,
    token_store: std::sync::Arc<dyn TokenStore>,
    profile: String,
    cached_copilot: Mutex<Option<CachedCopilotToken>>,
}

impl GitHubCopilotAuth {
    pub fn new(token_store: std::sync::Arc<dyn TokenStore>) -> Self {
        Self {
            client: reqwest::Client::new(),
            client_id: DEFAULT_CLIENT_ID.to_string(),
            device_code_url: DEFAULT_DEVICE_CODE_URL.to_string(),
            access_token_url: DEFAULT_ACCESS_TOKEN_URL.to_string(),
            copilot_token_url: DEFAULT_COPILOT_TOKEN_URL.to_string(),
            token_store,
            profile: "default".to_string(),
            cached_copilot: Mutex::new(None),
        }
    }

    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = profile.into();
        self
    }

    pub fn with_device_code_url(mut self, url: impl Into<String>) -> Self {
        self.device_code_url = url.into();
        self
    }

    pub fn with_access_token_url(mut self, url: impl Into<String>) -> Self {
        self.access_token_url = url.into();
        self
    }

    pub fn with_copilot_token_url(mut self, url: impl Into<String>) -> Self {
        self.copilot_token_url = url.into();
        self
    }

    pub async fn logged_in(&self) -> Result<bool, AuthError> {
        Ok(self
            .token_store
            .load("github-copilot", &self.profile)?
            .is_some())
    }

    pub async fn start_device_code(&self) -> Result<DeviceCodeSession, AuthError> {
        let resp = self
            .client
            .post(&self.device_code_url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("scope", "read:user"),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(AuthError::InvalidResponse(format!(
                "Device code request failed with status {}",
                resp.status()
            )));
        }
        let payload: GitHubDeviceCodeResponse = resp.json().await?;
        let expires_at = Utc::now() + Duration::seconds(payload.expires_in as i64);
        Ok(DeviceCodeSession {
            provider: "github-copilot".to_string(),
            verification_url: payload.verification_uri,
            user_code: payload.user_code,
            device_code: payload.device_code,
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
        let resp = self
            .client
            .post(&self.access_token_url)
            .header("Accept", "application/json")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("device_code", session.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(AuthError::InvalidResponse(format!(
                "Device token request failed with status {}",
                resp.status()
            )));
        }
        let payload: GitHubDeviceTokenResponse = resp.json().await?;
        if let Some(access_token) = payload.access_token {
            let token = Token {
                access_token,
                refresh_token: None,
                id_token: None,
                expires_at: None,
                last_refresh: Some(Utc::now()),
                scopes: payload
                    .scope
                    .map(|s| s.split(',').map(|v| v.trim().to_string()).collect()),
                account_id: None,
            };
            self.token_store
                .save("github-copilot", &self.profile, &token)?;
            return Ok(DeviceCodePoll::Authorized { token });
        }
        match payload.error.as_deref() {
            Some("authorization_pending") => Ok(DeviceCodePoll::Pending {
                interval_secs: session.interval_secs,
            }),
            Some("slow_down") => Ok(DeviceCodePoll::SlowDown {
                interval_secs: session.interval_secs + 2,
            }),
            Some("expired_token") => Ok(DeviceCodePoll::Expired),
            Some("access_denied") => Ok(DeviceCodePoll::AccessDenied),
            Some(other) => Err(AuthError::InvalidResponse(format!(
                "Device code error: {other}"
            ))),
            None => Err(AuthError::InvalidResponse(
                "Device code response missing token and error".to_string(),
            )),
        }
    }

    pub async fn exchange_copilot_token(&self) -> Result<CopilotToken, AuthError> {
        if let Some(cached) = self.read_cached_token() {
            return Ok(cached);
        }
        let github_token = self
            .token_store
            .load("github-copilot", &self.profile)?
            .ok_or(AuthError::NotLoggedIn)?;
        let resp = self
            .client
            .get(&self.copilot_token_url)
            .header("Accept", "application/json")
            .header(
                "Authorization",
                format!("Bearer {}", github_token.access_token),
            )
            .send()
            .await?;
        if resp.status() == StatusCode::UNAUTHORIZED {
            return Err(AuthError::ExpiredOrInvalidGrant);
        }
        if !resp.status().is_success() {
            return Err(AuthError::InvalidResponse(format!(
                "Copilot token exchange failed with status {}",
                resp.status()
            )));
        }
        let payload: CopilotTokenResponse = resp.json().await?;
        let expires_at = parse_expires_at(payload.expires_at)?;
        let base_url = derive_copilot_base_url(&payload.token)
            .unwrap_or_else(|| DEFAULT_COPILOT_BASE_URL.to_string());
        let token = CopilotToken {
            token: payload.token,
            expires_at,
            base_url,
        };
        self.write_cached_token(token.clone());
        Ok(token)
    }

    fn read_cached_token(&self) -> Option<CopilotToken> {
        let guard = self.cached_copilot.lock().ok()?;
        let cached = guard.as_ref()?;
        let now = Utc::now();
        if cached.expires_at - now < Duration::minutes(5) {
            return None;
        }
        Some(CopilotToken {
            token: cached.token.clone(),
            expires_at: cached.expires_at,
            base_url: cached.base_url.clone(),
        })
    }

    fn write_cached_token(&self, token: CopilotToken) {
        if let Ok(mut guard) = self.cached_copilot.lock() {
            *guard = Some(CachedCopilotToken {
                token: token.token,
                expires_at: token.expires_at,
                base_url: token.base_url,
            });
        }
    }
}

#[derive(Debug, Clone)]
struct CachedCopilotToken {
    token: String,
    expires_at: DateTime<Utc>,
    base_url: String,
}

#[derive(Debug, Clone)]
pub struct CopilotToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub base_url: String,
}

#[derive(Debug, Deserialize)]
struct GitHubDeviceCodeResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}

#[derive(Debug, Deserialize)]
struct GitHubDeviceTokenResponse {
    access_token: Option<String>,
    scope: Option<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CopilotTokenResponse {
    token: String,
    expires_at: serde_json::Value,
}

fn parse_expires_at(value: serde_json::Value) -> Result<DateTime<Utc>, AuthError> {
    if let Some(num) = value.as_i64() {
        let secs = if num > 10_000_000_000 {
            num / 1000
        } else {
            num
        };
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64);
        return Ok(DateTime::<Utc>::from(time));
    }
    if let Some(text) = value.as_str() {
        let parsed: i64 = text.parse().map_err(|_| {
            AuthError::InvalidResponse("Copilot token expires_at invalid".to_string())
        })?;
        let secs = if parsed > 10_000_000_000 {
            parsed / 1000
        } else {
            parsed
        };
        let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64);
        return Ok(DateTime::<Utc>::from(time));
    }
    Err(AuthError::InvalidResponse(
        "Copilot token expires_at missing".to_string(),
    ))
}

fn derive_copilot_base_url(token: &str) -> Option<String> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return None;
    }
    let proxy = trimmed.split(';').find_map(|part| {
        let part = part.trim();
        if part.to_ascii_lowercase().starts_with("proxy-ep=") {
            Some(part.trim_start_matches("proxy-ep=").trim().to_string())
        } else {
            None
        }
    })?;
    let host = proxy
        .replace("https://", "")
        .replace("http://", "")
        .replacen("proxy.", "api.", 1);
    if host.is_empty() {
        return None;
    }
    Some(format!("https://{host}"))
}
