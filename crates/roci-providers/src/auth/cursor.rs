//! Cursor browser authorization and token exchange.
//!
//! Protocol reference: CLIProxyAPIPlus internal/auth/cursor/oauth.go. Cursor
//! binds a browser login to a PKCE verifier through a polling endpoint; there
//! is no authorization code to paste and no loopback callback server.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::{DateTime, Utc};
use roci_core::auth::{
    AuthBackend, AuthError, AuthPollResult, AuthStep, CredentialFlow, DeviceCodeSession, Token,
    TokenStore,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const LOGIN_URL: &str = "https://cursor.com/loginDeepControl";
const POLL_URL: &str = "https://api2.cursor.sh/auth/poll";
const REFRESH_URL: &str = "https://api2.cursor.sh/auth/exchange_user_api_key";

/// Cursor authorization operations. Endpoint injection supports isolated tests.
pub struct CursorAuth {
    store: Arc<dyn TokenStore>,
    poll_url: String,
    refresh_url: String,
}

#[derive(Serialize, Deserialize)]
struct LoginSession {
    uuid: String,
    verifier: String,
    expires_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenPair {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

impl CursorAuth {
    pub fn new(store: Arc<dyn TokenStore>) -> Self {
        Self {
            store,
            poll_url: POLL_URL.into(),
            refresh_url: REFRESH_URL.into(),
        }
    }

    pub fn with_endpoints(mut self, poll_url: String, refresh_url: String) -> Self {
        self.poll_url = poll_url;
        self.refresh_url = refresh_url;
        self
    }

    pub fn start_login(&self) -> Result<AuthStep, AuthError> {
        let mut entropy = Vec::with_capacity(96);
        for _ in 0..6 {
            entropy.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
        }
        let verifier = URL_SAFE_NO_PAD.encode(entropy);
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let uuid = uuid::Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::minutes(15);
        let mut authorization_url = reqwest::Url::parse(LOGIN_URL)
            .map_err(|_| AuthError::InvalidResponse("invalid Cursor login endpoint".into()))?;
        authorization_url.query_pairs_mut().extend_pairs([
            ("challenge", challenge.as_str()),
            ("uuid", uuid.as_str()),
            ("mode", "login"),
            ("redirectTarget", "cli"),
        ]);
        Ok(AuthStep::BrowserPoll {
            authorization_url: authorization_url.to_string(),
            interval: Duration::from_secs(2),
            expires_at,
            session_data: serde_json::to_value(LoginSession {
                uuid,
                verifier,
                expires_at,
            })?,
        })
    }

    pub async fn poll(&self, session: &serde_json::Value) -> Result<AuthPollResult, AuthError> {
        let session: LoginSession = serde_json::from_value(session.clone())
            .map_err(|_| AuthError::InvalidResponse("invalid Cursor login session".into()))?;
        if session.expires_at <= Utc::now() {
            return Ok(AuthPollResult::Expired);
        }
        let response = auth_client()?
            .get(&self.poll_url)
            .query(&[("uuid", &session.uuid), ("verifier", &session.verifier)])
            .timeout(Duration::from_secs(10))
            .send()
            .await
            // A reqwest error can contain the verifier-bearing query URL.
            .map_err(|_| AuthError::Network("Cursor authorization polling failed".into()))?;
        match response.status().as_u16() {
            404 => Ok(AuthPollResult::Pending),
            429 => Ok(AuthPollResult::SlowDown {
                new_interval: Duration::from_secs(10),
            }),
            401 | 403 => Ok(AuthPollResult::Denied),
            200..=299 => {
                let pair = read_pair(response).await?;
                let token = token_from_pair(pair, None)?;
                self.store.save("cursor", "default", &token)?;
                Ok(AuthPollResult::Authorized { token })
            }
            500..=599 => Err(AuthError::Network(
                "Cursor authorization service unavailable".into(),
            )),
            status => Err(AuthError::InvalidResponse(format!(
                "Cursor authorization returned HTTP {status}"
            ))),
        }
    }

    /// Return a refreshed token; callers own coordinated persistence/rotation.
    pub async fn refresh_token(&self, token: &Token) -> Result<Token, AuthError> {
        let refresh = token
            .refresh_token
            .as_deref()
            .filter(|s| !s.is_empty())
            .ok_or(AuthError::ExpiredOrInvalidGrant)?;
        let response = auth_client()?
            .post(&self.refresh_url)
            .bearer_auth(refresh)
            .json(&serde_json::json!({}))
            .timeout(Duration::from_secs(10))
            .send()
            .await
            .map_err(|_| AuthError::Network("Cursor token refresh request failed".into()))?;
        match response.status().as_u16() {
            401 | 403 => return Err(AuthError::ExpiredOrInvalidGrant),
            429 => {
                return Err(AuthError::RateLimited {
                    retry_after_ms: None,
                })
            }
            200..=299 => {}
            status => {
                return Err(AuthError::InvalidResponse(format!(
                    "Cursor refresh returned HTTP {status}"
                )))
            }
        }
        let mut refreshed = token_from_pair(read_pair(response).await?, Some(refresh))?;
        refreshed.provider_metadata = token.provider_metadata.clone();
        if refreshed.account_id.is_none() {
            refreshed.account_id.clone_from(&token.account_id);
        }
        Ok(refreshed)
    }
}

fn auth_client() -> Result<reqwest::Client, AuthError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .build()
        .map_err(|_| AuthError::Network("could not create Cursor authentication client".into()))
}

async fn read_pair(mut response: reqwest::Response) -> Result<TokenPair, AuthError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| AuthError::Network("Cursor credential response interrupted".into()))?
    {
        if bytes.len().saturating_add(chunk.len()) > 64 * 1024 {
            return Err(AuthError::InvalidResponse(
                "Cursor credential response too large".into(),
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| AuthError::InvalidResponse("invalid Cursor credential response".into()))
}

fn token_from_pair(pair: TokenPair, previous_refresh: Option<&str>) -> Result<Token, AuthError> {
    if pair.access_token.trim().is_empty() {
        return Err(AuthError::InvalidResponse(
            "Cursor response missing access token".into(),
        ));
    }
    let refresh_token = pair
        .refresh_token
        .filter(|s| !s.trim().is_empty())
        .or_else(|| previous_refresh.map(str::to_owned));
    if refresh_token.is_none() {
        return Err(AuthError::InvalidResponse(
            "Cursor response missing refresh token".into(),
        ));
    }
    // Claims are untrusted scheduling/account hints, never an authorization check.
    let claims = pair
        .access_token
        .split('.')
        .nth(1)
        .and_then(|value| URL_SAFE_NO_PAD.decode(value).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let expires_at = claims
        .as_ref()
        .and_then(|c| c.get("exp"))
        .and_then(serde_json::Value::as_i64)
        .and_then(|exp| DateTime::from_timestamp(exp, 0))
        .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(1));
    let account_id = claims
        .as_ref()
        .and_then(|c| c.get("sub"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    Ok(Token {
        provider_metadata: None,
        access_token: pair.access_token,
        refresh_token,
        id_token: None,
        expires_at: Some(expires_at),
        last_refresh: Some(Utc::now()),
        scopes: None,
        account_id,
    })
}

pub struct CursorBackend;

#[async_trait]
impl AuthBackend for CursorBackend {
    fn aliases(&self) -> &[&str] {
        &["cursor"]
    }
    fn display_name(&self) -> &str {
        "Cursor"
    }
    fn store_key(&self) -> &str {
        "cursor"
    }
    fn canonical_provider_key(&self) -> &str {
        "cursor"
    }
    fn oauth_flow(&self) -> CredentialFlow {
        CredentialFlow::BrowserPoll
    }
    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
        CursorAuth::new(store.clone()).start_login()
    }
    async fn poll_browser(
        &self,
        store: &Arc<dyn TokenStore>,
        session: &serde_json::Value,
    ) -> Result<AuthPollResult, AuthError> {
        CursorAuth::new(store.clone()).poll(session).await
    }
    async fn poll_device_code(
        &self,
        _: &Arc<dyn TokenStore>,
        _: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        Err(AuthError::Unsupported("Cursor uses browser polling".into()))
    }
    async fn complete_pkce(
        &self,
        _: &Arc<dyn TokenStore>,
        _: &str,
        _: &str,
        _session_data: &serde_json::Value,
    ) -> Result<Token, AuthError> {
        Err(AuthError::Unsupported("Cursor uses browser polling".into()))
    }
    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        store.load("cursor", "default")
    }
    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError> {
        store.clear("cursor", "default")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roci_core::auth::{FileTokenStore, TokenStoreConfig};
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn browser_poll_preserves_pkce_binding_and_persists_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        let server = MockServer::start().await;
        let auth = CursorAuth::new(store.clone())
            .with_endpoints(format!("{}/poll", server.uri()), server.uri());
        let AuthStep::BrowserPoll {
            authorization_url,
            session_data,
            ..
        } = auth.start_login().unwrap()
        else {
            panic!()
        };
        let url = reqwest::Url::parse(&authorization_url).unwrap();
        let query: std::collections::HashMap<_, _> = url.query_pairs().collect();
        let verifier = session_data["verifier"].as_str().unwrap();
        assert_eq!(
            query["challenge"],
            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
        );
        assert!(!authorization_url.contains(verifier));
        Mock::given(method("GET"))
            .and(path("/poll"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        assert!(matches!(
            auth.poll(&session_data).await.unwrap(),
            AuthPollResult::Pending
        ));
        server.reset().await;
        Mock::given(method("GET"))
            .and(path("/poll"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"accessToken":"access", "refreshToken":"refresh"}),
            ))
            .mount(&server)
            .await;
        assert!(matches!(
            auth.poll(&session_data).await.unwrap(),
            AuthPollResult::Authorized { .. }
        ));
        assert_eq!(
            store
                .load("cursor", "default")
                .unwrap()
                .unwrap()
                .access_token,
            "access"
        );
    }

    #[tokio::test]
    async fn refresh_retains_refresh_token_and_redacts_error_body() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        let server = MockServer::start().await;
        let auth = CursorAuth::new(store).with_endpoints(server.uri(), server.uri());
        let token = token_from_pair(
            TokenPair {
                access_token: "old".into(),
                refresh_token: Some("secret-refresh".into()),
            },
            None,
        )
        .unwrap();
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"accessToken":"new"})),
            )
            .mount(&server)
            .await;
        let refreshed = auth.refresh_token(&token).await.unwrap();
        assert_eq!(refreshed.access_token, "new");
        assert_eq!(refreshed.refresh_token, token.refresh_token);
        server.reset().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400).set_body_string("secret-refresh"))
            .mount(&server)
            .await;
        assert!(!auth
            .refresh_token(&token)
            .await
            .unwrap_err()
            .to_string()
            .contains("secret-refresh"));
    }

    #[tokio::test]
    async fn polling_does_not_follow_redirects_or_expose_pkce_verifier() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        let server = MockServer::start().await;
        let destination = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(&destination)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", destination.uri()))
            .mount(&server)
            .await;
        let auth = CursorAuth::new(store).with_endpoints(server.uri(), server.uri());
        let AuthStep::BrowserPoll { session_data, .. } = auth.start_login().unwrap() else {
            panic!()
        };
        let error = auth.poll(&session_data).await.unwrap_err().to_string();
        assert!(!error.contains(session_data["verifier"].as_str().unwrap()));
        assert!(error.contains("302"));
    }
}
