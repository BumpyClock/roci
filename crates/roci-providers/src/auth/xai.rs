//! xAI Grok CLI OAuth device authorization and refresh.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;

use roci_core::auth::{
    AuthBackend, AuthError, AuthPollResult, AuthStep, CredentialFlow, DeviceCodeSession, Token,
    TokenStore,
};

const DISCOVERY_URL: &str = "https://auth.x.ai/.well-known/openid-configuration";
const CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const SCOPE: &str = "openid profile email offline_access grok-cli:access api:access";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
const STORE_KEY: &str = "xai";

/// Provider protocol helper. Refresh coordination and persistence belong to the caller.
pub struct XaiAuth {
    client: reqwest::Client,
    store: Arc<dyn TokenStore>,
    profile: String,
    discovery_url: String,
    #[cfg(test)]
    allow_test_endpoint: bool,
}

impl XaiAuth {
    pub fn new(store: Arc<dyn TokenStore>) -> Self {
        Self {
            // Redirects must not move a credential-bearing form to another origin.
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("static xAI HTTP client configuration"),
            store,
            profile: "default".into(),
            discovery_url: DISCOVERY_URL.into(),
            #[cfg(test)]
            allow_test_endpoint: false,
        }
    }

    pub fn with_profile(mut self, profile: impl Into<String>) -> Self {
        self.profile = profile.into();
        self
    }

    /// Load the persisted token, including its expiry, for the lifecycle coordinator.
    pub fn get_token(&self) -> Result<Token, AuthError> {
        self.store
            .load(STORE_KEY, &self.profile)?
            .ok_or(AuthError::NotLoggedIn)
    }

    async fn discover(&self) -> Result<Discovery, AuthError> {
        let response = self
            .client
            .get(&self.discovery_url)
            .timeout(Duration::from_secs(30))
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = response.status();
        if !status.is_success() {
            return Err(http_error(status));
        }
        let discovery: Discovery = response.json().await?;
        for endpoint in [
            &discovery.device_authorization_endpoint,
            &discovery.token_endpoint,
        ] {
            #[cfg(test)]
            if self.allow_test_endpoint && endpoint.starts_with("http://127.0.0.1:") {
                continue;
            }
            validate_endpoint(endpoint)?;
        }
        Ok(discovery)
    }

    async fn post_form(
        &self,
        endpoint: &str,
        form: &[(&str, &str)],
    ) -> Result<reqwest::Response, AuthError> {
        Ok(self
            .client
            .post(endpoint)
            .timeout(Duration::from_secs(30))
            .header("Accept", "application/json")
            .form(form)
            .send()
            .await?)
    }

    pub async fn start_device_code(&self) -> Result<DeviceCodeSession, AuthError> {
        let discovery = self.discover().await?;
        let response = self
            .post_form(
                &discovery.device_authorization_endpoint,
                &[("client_id", CLIENT_ID), ("scope", SCOPE)],
            )
            .await?;
        if !response.status().is_success() {
            return Err(http_error(response.status()));
        }
        let payload: DeviceResponse = response.json().await?;
        let verification_url = nonempty(payload.verification_uri_complete)
            .or_else(|| nonempty(payload.verification_uri))
            .ok_or_else(|| invalid("device response missing verification URI"))?;
        let verification = reqwest::Url::parse(&verification_url)
            .map_err(|_| invalid("invalid verification URI"))?;
        if verification.scheme() != "https"
            || verification.host_str().is_none()
            || !verification.username().is_empty()
            || verification.password().is_some()
        {
            return Err(invalid("verification URI must use HTTPS"));
        }
        let device_code = nonempty(Some(payload.device_code))
            .ok_or_else(|| invalid("device response missing device_code"))?;
        let user_code = nonempty(Some(payload.user_code))
            .ok_or_else(|| invalid("device response missing user_code"))?;
        let lifetime = payload.expires_in.unwrap_or(1800).min(1800);
        if lifetime == 0 {
            return Err(AuthError::ExpiredOrInvalidGrant);
        }
        Ok(DeviceCodeSession {
            provider: STORE_KEY.into(),
            verification_url,
            user_code,
            device_code,
            interval_secs: payload.interval.unwrap_or(5).max(5),
            expires_at: Utc::now() + chrono::Duration::seconds(lifetime as i64),
        })
    }

    pub async fn poll_device_code(
        &self,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        if session.provider != STORE_KEY {
            return Err(invalid("device session belongs to another provider"));
        }
        if Utc::now() >= session.expires_at {
            return Ok(AuthPollResult::Expired);
        }
        // DeviceCodeSession is provider-neutral; rediscovery avoids hiding transport
        // metadata in the device code or account ID.
        let discovery = self.discover().await?;
        let response = self
            .post_form(
                &discovery.token_endpoint,
                &[
                    ("client_id", CLIENT_ID),
                    ("grant_type", DEVICE_GRANT),
                    ("device_code", session.device_code.as_str()),
                ],
            )
            .await?;
        let status = response.status();
        if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(http_error(status));
        }
        let payload: TokenResponse = response.json().await?;
        match payload.error.as_deref() {
            Some("authorization_pending") => return Ok(AuthPollResult::Pending),
            Some("slow_down") => {
                return Ok(AuthPollResult::SlowDown {
                    new_interval: Duration::from_secs(
                        session.interval_secs.max(5).saturating_add(5),
                    ),
                });
            }
            Some("expired_token") => return Ok(AuthPollResult::Expired),
            Some("access_denied") => return Ok(AuthPollResult::Denied),
            Some("invalid_grant" | "invalid_token") => {
                return Err(AuthError::ExpiredOrInvalidGrant);
            }
            Some(_) => return Err(invalid("device endpoint returned an OAuth error")),
            None => {}
        }
        if !status.is_success() {
            return Err(http_error(status));
        }
        let token = payload.into_token(None)?;
        self.store.save(STORE_KEY, &self.profile, &token)?;
        Ok(AuthPollResult::Authorized { token })
    }

    /// Exchange a refresh token without modifying storage. The caller must serialize
    /// concurrent refreshes and durably save the rotated token before publishing it.
    pub async fn refresh_token(&self, current: &Token) -> Result<Token, AuthError> {
        let refresh = current
            .refresh_token
            .as_deref()
            .filter(|v| !v.trim().is_empty())
            .ok_or(AuthError::ExpiredOrInvalidGrant)?;
        let discovery = self.discover().await?;
        let response = self
            .post_form(
                &discovery.token_endpoint,
                &[
                    ("client_id", CLIENT_ID),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh),
                ],
            )
            .await?;
        let status = response.status();
        if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(http_error(status));
        }
        let payload: TokenResponse = response.json().await?;
        if matches!(
            payload.error.as_deref(),
            Some("invalid_grant" | "invalid_token" | "access_denied")
        ) {
            return Err(AuthError::ExpiredOrInvalidGrant);
        }
        if !status.is_success() {
            return Err(http_error(status));
        }
        if payload.error.is_some() {
            return Err(invalid("token endpoint returned an OAuth error"));
        }
        payload.into_token(Some(current))
    }
}

fn invalid(message: &str) -> AuthError {
    AuthError::InvalidResponse(format!("xAI: {message}"))
}

fn http_error(status: reqwest::StatusCode) -> AuthError {
    match status {
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => {
            AuthError::ExpiredOrInvalidGrant
        }
        reqwest::StatusCode::TOO_MANY_REQUESTS => AuthError::RateLimited {
            retry_after_ms: None,
        },
        _ if status.is_server_error() => AuthError::Network(format!("xAI auth HTTP {status}")),
        _ => invalid(&format!("auth HTTP {status}")),
    }
}

fn validate_endpoint(endpoint: &str) -> Result<(), AuthError> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| invalid("invalid OAuth endpoint"))?;
    let trusted_host = url
        .host_str()
        .is_some_and(|host| host == "x.ai" || host.ends_with(".x.ai"));
    if url.scheme() != "https"
        || !trusted_host
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid("OAuth endpoint must use HTTPS on x.ai"));
    }
    Ok(())
}

fn nonempty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

#[derive(Deserialize)]
struct Discovery {
    device_authorization_endpoint: String,
    token_endpoint: String,
}

#[derive(Deserialize)]
struct DeviceResponse {
    #[serde(default)]
    device_code: String,
    #[serde(default)]
    user_code: String,
    verification_uri: Option<String>,
    verification_uri_complete: Option<String>,
    expires_in: Option<u64>,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResponse {
    error: Option<String>,
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<u64>,
}

impl TokenResponse {
    fn into_token(self, old: Option<&Token>) -> Result<Token, AuthError> {
        let access_token = nonempty(self.access_token)
            .ok_or_else(|| invalid("token response missing access_token"))?;
        let now = Utc::now();
        let expires_at = match self.expires_in.filter(|seconds| *seconds > 0) {
            Some(seconds) => Some(
                now.checked_add_signed(
                    chrono::Duration::try_seconds(
                        i64::try_from(seconds).map_err(|_| invalid("token expiry out of range"))?,
                    )
                    .ok_or_else(|| invalid("token expiry out of range"))?,
                )
                .ok_or_else(|| invalid("token expiry out of range"))?,
            ),
            None => None,
        };
        Ok(Token {
            provider_metadata: old.and_then(|value| value.provider_metadata.clone()),
            access_token,
            refresh_token: nonempty(self.refresh_token)
                .or_else(|| old.and_then(|v| v.refresh_token.clone())),
            id_token: nonempty(self.id_token).or_else(|| old.and_then(|v| v.id_token.clone())),
            expires_at,
            last_refresh: Some(now),
            scopes: Some(SCOPE.split_whitespace().map(str::to_owned).collect()),
            account_id: old.and_then(|v| v.account_id.clone()),
        })
    }
}

/// Registers xAI device login against the existing Grok launch provider.
pub struct XaiBackend;

#[async_trait]
impl AuthBackend for XaiBackend {
    fn aliases(&self) -> &[&str] {
        &["grok", "xai"]
    }
    fn display_name(&self) -> &str {
        "Grok"
    }
    fn store_key(&self) -> &str {
        STORE_KEY
    }
    fn canonical_provider_key(&self) -> &str {
        "grok"
    }
    fn oauth_flow(&self) -> CredentialFlow {
        CredentialFlow::DeviceCode
    }
    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
        let session = XaiAuth::new(store.clone()).start_device_code().await?;
        Ok(AuthStep::DeviceCode {
            verification_url: session.verification_url.clone(),
            user_code: session.user_code.clone(),
            interval: Duration::from_secs(session.interval_secs),
            expires_at: session.expires_at,
            session,
        })
    }
    async fn poll_device_code(
        &self,
        store: &Arc<dyn TokenStore>,
        session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        XaiAuth::new(store.clone()).poll_device_code(session).await
    }
    async fn complete_pkce(
        &self,
        _: &Arc<dyn TokenStore>,
        _: &str,
        _: &str,
        _session_data: &serde_json::Value,
    ) -> Result<Token, AuthError> {
        Err(AuthError::Unsupported(
            "xAI uses device authorization".into(),
        ))
    }
    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        store.load(STORE_KEY, "default")
    }
    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError> {
        store.clear(STORE_KEY, "default")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use roci_core::auth::{FileTokenStore, TokenStoreConfig};
    use serde_json::json;
    use wiremock::{
        matchers::{body_string_contains, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    async fn fixture() -> (MockServer, XaiAuth, tempfile::TempDir) {
        let server = MockServer::start().await;
        let temp = tempfile::tempdir().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            temp.path().into(),
        )));
        let mut auth = XaiAuth::new(store);
        auth.discovery_url = format!("{}/discovery", server.uri());
        auth.allow_test_endpoint = true;
        Mock::given(method("GET"))
            .and(path("/discovery"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_authorization_endpoint": format!("{}/device", server.uri()),
                "token_endpoint": format!("{}/token", server.uri()),
            })))
            .mount(&server)
            .await;
        (server, auth, temp)
    }

    fn session() -> DeviceCodeSession {
        DeviceCodeSession {
            provider: STORE_KEY.into(),
            verification_url: "https://auth.x.ai/device".into(),
            user_code: "USER".into(),
            device_code: "opaque-device".into(),
            interval_secs: 7,
            expires_at: Utc::now() + chrono::Duration::minutes(5),
        }
    }

    #[test]
    fn validates_discovered_endpoints_without_echoing_credentials() {
        for endpoint in [
            "http://auth.x.ai/token",
            "https://x.ai.attacker.test/token",
            "https://attacker.test/token",
            "https://secret@auth.x.ai/token",
            "https://auth.x.ai/token#secret",
        ] {
            let error = validate_endpoint(endpoint).unwrap_err().to_string();
            assert!(!error.contains("secret"));
        }
        assert!(validate_endpoint("https://auth.x.ai/token").is_ok());
    }

    #[tokio::test]
    async fn discovers_and_starts_device_flow() {
        let (server, auth, _temp) = fixture().await;
        Mock::given(method("POST")).and(path("/device"))
            .and(body_string_contains(format!("client_id={CLIENT_ID}")))
            .and(body_string_contains("scope=openid+profile+email+offline_access+grok-cli%3Aaccess+api%3Aaccess"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "device_code": "device", "user_code": "USER", "verification_uri": "https://auth.x.ai/device",
                "verification_uri_complete": "https://auth.x.ai/device?code=USER", "expires_in": 600, "interval": 2
            }))).expect(1).mount(&server).await;
        let result = auth.start_device_code().await.unwrap();
        assert_eq!(
            result.verification_url,
            "https://auth.x.ai/device?code=USER"
        );
        assert_eq!(result.interval_secs, 5);
        assert!(result.expires_at > Utc::now());
    }

    #[tokio::test]
    async fn device_poll_handles_oauth_errors_on_http_400() {
        for (code, expected) in [
            ("authorization_pending", "pending"),
            ("slow_down", "slow"),
            ("access_denied", "denied"),
            ("expired_token", "expired"),
        ] {
            let (server, auth, _temp) = fixture().await;
            Mock::given(method("POST"))
                .and(path("/token"))
                .and(body_string_contains("device_code=opaque-device"))
                .and(body_string_contains(
                    "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
                ))
                .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": code})))
                .expect(1)
                .mount(&server)
                .await;
            match (expected, auth.poll_device_code(&session()).await.unwrap()) {
                ("pending", AuthPollResult::Pending)
                | ("denied", AuthPollResult::Denied)
                | ("expired", AuthPollResult::Expired) => {}
                ("slow", AuthPollResult::SlowDown { new_interval }) => {
                    assert_eq!(new_interval.as_secs(), 12)
                }
                other => panic!("unexpected poll result: {other:?}"),
            }
            assert!(matches!(auth.get_token(), Err(AuthError::NotLoggedIn)));
        }
    }

    #[tokio::test]
    async fn successful_device_poll_persists_before_authorized() {
        let (server, auth, _temp) = fixture().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "access_token": "access", "refresh_token": "refresh", "expires_in": 3600
            })))
            .mount(&server)
            .await;
        assert!(matches!(
            auth.poll_device_code(&session()).await.unwrap(),
            AuthPollResult::Authorized { .. }
        ));
        let saved = auth.get_token().unwrap();
        assert_eq!(saved.access_token, "access");
        assert_eq!(saved.refresh_token.as_deref(), Some("refresh"));
        assert!(saved.expires_at.unwrap() > Utc::now());
    }

    #[tokio::test]
    async fn refresh_preserves_omitted_fields_and_does_not_persist() {
        let (server, auth, _temp) = fixture().await;
        let old = Token {
            provider_metadata: None,
            access_token: "old-access".into(),
            refresh_token: Some("old-refresh".into()),
            id_token: Some("identity".into()),
            expires_at: None,
            last_refresh: None,
            scopes: None,
            account_id: Some("account".into()),
        };
        auth.store.save(STORE_KEY, "default", &old).unwrap();
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=old-refresh"))
            .and(body_string_contains(format!("client_id={CLIENT_ID}")))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"access_token":"new-access", "expires_in":3600})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let refreshed = auth.refresh_token(&old).await.unwrap();
        assert_eq!(refreshed.access_token, "new-access");
        assert_eq!(refreshed.refresh_token, old.refresh_token);
        assert_eq!(refreshed.id_token, old.id_token);
        assert_eq!(refreshed.account_id, old.account_id);
        assert_eq!(auth.get_token().unwrap().access_token, "old-access");
    }

    #[tokio::test]
    async fn expired_session_does_not_make_network_requests() {
        let (_server, auth, _temp) = fixture().await;
        let mut expired = session();
        expired.expires_at = Utc::now() - chrono::Duration::seconds(1);
        assert!(matches!(
            auth.poll_device_code(&expired).await.unwrap(),
            AuthPollResult::Expired
        ));
    }

    #[test]
    fn invalid_expiry_is_rejected_without_panicking() {
        let payload: TokenResponse =
            serde_json::from_value(json!({"access_token":"a", "expires_in":u64::MAX})).unwrap();
        assert!(payload.into_token(None).is_err());
    }

    #[tokio::test]
    async fn login_does_not_report_success_when_persistence_fails() {
        struct FailingStore;
        impl TokenStore for FailingStore {
            fn save_if_current(
                &self,
                _: &str,
                _: &str,
                _: Option<&Token>,
                _: &Token,
            ) -> Result<bool, AuthError> {
                panic!("login persistence test must not refresh tokens")
            }
            fn try_acquire_refresh_lease(
                &self,
                _: &str,
                _: &str,
            ) -> Result<Option<Box<dyn roci_core::auth::TokenRefreshLease>>, AuthError>
            {
                panic!("login persistence test must not acquire refresh leases")
            }
            fn load(&self, _: &str, _: &str) -> Result<Option<Token>, AuthError> {
                Ok(None)
            }
            fn save(&self, _: &str, _: &str, _: &Token) -> Result<(), AuthError> {
                Err(AuthError::Io("disk full".into()))
            }
            fn clear(&self, _: &str, _: &str) -> Result<(), AuthError> {
                Ok(())
            }
        }
        let (server, mut auth, _temp) = fixture().await;
        auth.store = Arc::new(FailingStore);
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"token"})))
            .mount(&server)
            .await;
        assert!(matches!(
            auth.poll_device_code(&session()).await,
            Err(AuthError::Io(_))
        ));
    }

    #[tokio::test]
    async fn refresh_distinguishes_revoked_grants_from_temporary_failures() {
        for (status, error) in [
            (400, "invalid_grant"),
            (429, "rate_limit"),
            (503, "server_error"),
        ] {
            let (server, auth, _temp) = fixture().await;
            Mock::given(method("POST"))
                .and(path("/token"))
                .respond_with(ResponseTemplate::new(status).set_body_json(
                    json!({"error":error, "error_description":"secret-refresh-token"}),
                ))
                .mount(&server)
                .await;
            let old = Token {
                provider_metadata: None,
                access_token: "old".into(),
                refresh_token: Some("secret-refresh-token".into()),
                id_token: None,
                expires_at: None,
                last_refresh: None,
                scopes: None,
                account_id: None,
            };
            let result = auth.refresh_token(&old).await.unwrap_err();
            assert!(!result.to_string().contains("secret-refresh-token"));
            match status {
                400 => assert!(matches!(result, AuthError::ExpiredOrInvalidGrant)),
                429 => assert!(matches!(result, AuthError::RateLimited { .. })),
                503 => assert!(matches!(result, AuthError::Network(_))),
                _ => unreachable!(),
            }
        }
    }
}
