//! Gemini CLI installed-application OAuth and Cloud Code Assist onboarding.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use futures::StreamExt;
use roci_core::auth::{
    AuthBackend, AuthError, AuthPollResult, AuthStep, CredentialFlow, DeviceCodeSession,
    ProviderTokenMetadata, Token, TokenStore,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const AUTHORIZE_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const REDIRECT_URI: &str = "https://codeassist.google.com/authcode";
pub(crate) const CLOUD_CODE_ENDPOINT: &str = "https://cloudcode-pa.googleapis.com";
const SCOPES: &str = "https://www.googleapis.com/auth/cloud-platform https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/userinfo.profile";

pub struct GeminiAuth {
    store: Arc<dyn TokenStore>,
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    token_url: String,
    cloud_endpoint: String,
    project_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct Session {
    state: String,
    verifier: String,
    redirect_uri: String,
}

impl GeminiAuth {
    /// Read the host's OAuth client configuration from `ROCI_GEMINI_OAUTH_CLIENT_ID`
    /// and `ROCI_GEMINI_OAUTH_CLIENT_SECRET`. Both are required for browser OAuth.
    pub fn new(store: Arc<dyn TokenStore>) -> Self {
        Self {
            store,
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .build()
                .expect("valid Gemini auth client"),
            client_id: std::env::var("ROCI_GEMINI_OAUTH_CLIENT_ID").unwrap_or_default(),
            client_secret: std::env::var("ROCI_GEMINI_OAUTH_CLIENT_SECRET").unwrap_or_default(),
            token_url: TOKEN_URL.into(),
            cloud_endpoint: CLOUD_CODE_ENDPOINT.into(),
            project_id: std::env::var("GOOGLE_CLOUD_PROJECT")
                .ok()
                .filter(|value| !value.trim().is_empty()),
        }
    }

    pub fn with_oauth_client(mut self, client_id: String, client_secret: String) -> Self {
        self.client_id = client_id;
        self.client_secret = client_secret;
        self
    }

    pub fn with_project(mut self, project_id: impl Into<String>) -> Self {
        self.project_id = Some(project_id.into());
        self
    }

    /// Override protocol endpoints, for a host-owned gateway or hermetic tests.
    pub fn with_endpoints(mut self, token_url: String, cloud_endpoint: String) -> Self {
        self.token_url = token_url;
        self.cloud_endpoint = cloud_endpoint.trim_end_matches('/').into();
        self
    }

    fn validate_oauth_client(&self) -> Result<(), AuthError> {
        if self.client_id.trim().is_empty() || self.client_secret.trim().is_empty() {
            return Err(invalid(
                "configure ROCI_GEMINI_OAUTH_CLIENT_ID and ROCI_GEMINI_OAUTH_CLIENT_SECRET or use GeminiAuth::with_oauth_client",
            ));
        }
        Ok(())
    }

    pub fn start_auth(&self) -> Result<AuthStep, AuthError> {
        self.validate_oauth_client()?;
        let verifier = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let state = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        let mut url = reqwest::Url::parse(AUTHORIZE_URL).expect("constant authorization URL");
        url.query_pairs_mut().extend_pairs([
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("response_type", "code"),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("scope", SCOPES),
            ("state", state.as_str()),
            ("code_challenge_method", "S256"),
            ("code_challenge", challenge.as_str()),
        ]);
        let session = Session {
            state: state.clone(),
            verifier,
            redirect_uri: REDIRECT_URI.into(),
        };
        Ok(AuthStep::Pkce {
            authorize_url: url.into(),
            state,
            session_data: serde_json::to_value(session)
                .map_err(|_| invalid("invalid login session"))?,
        })
    }

    pub async fn exchange_code(
        &self,
        input: &str,
        state: &str,
        session_data: &Value,
    ) -> Result<Token, AuthError> {
        self.validate_oauth_client()?;
        let session: Session = serde_json::from_value(session_data.clone())
            .map_err(|_| invalid("invalid login session"))?;
        if session.state != state
            || session.redirect_uri != REDIRECT_URI
            || session.verifier.len() < 43
        {
            return Err(invalid("login session mismatch"));
        }
        let code = authorization_code(input, state)?;
        let response = self
            .client
            .post(&self.token_url)
            .form(&[
                ("grant_type", "authorization_code"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("redirect_uri", session.redirect_uri.as_str()),
                ("code_verifier", session.verifier.as_str()),
                ("code", code.as_str()),
            ])
            .send()
            .await
            .map_err(|_| network())?;
        let mut token = parse_token(read_json(response).await?, None)?;
        let project = self.discover_project(&token.access_token).await?;
        set_project(&mut token, project);
        self.store.save("gemini", "default", &token)?;
        Ok(token)
    }

    /// Refresh without persistence; the runtime publishes rotating tokens atomically.
    pub async fn refresh_token(&self, current: &Token) -> Result<Token, AuthError> {
        self.validate_oauth_client()?;
        let refresh = current
            .refresh_token
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or(AuthError::ExpiredOrInvalidGrant)?;
        let response = self
            .client
            .post(&self.token_url)
            .form(&[
                ("grant_type", "refresh_token"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", refresh),
            ])
            .send()
            .await
            .map_err(|_| network())?;
        parse_token(read_json(response).await?, Some(current))
    }

    async fn cloud_call(
        &self,
        access: &str,
        action: &str,
        body: &Value,
    ) -> Result<Value, AuthError> {
        let response = self
            .client
            .post(format!("{}/v1internal:{action}", self.cloud_endpoint))
            .bearer_auth(access)
            .header("User-Agent", "GeminiCLI/0.34.0 roci")
            .json(body)
            .send()
            .await
            .map_err(|_| network())?;
        read_json(response).await
    }

    async fn discover_project(&self, access: &str) -> Result<String, AuthError> {
        let metadata = json!({"ideType":"IDE_UNSPECIFIED", "platform":"PLATFORM_UNSPECIFIED", "pluginType":"GEMINI"});
        let mut load = json!({"metadata":metadata});
        if let Some(project) = &self.project_id {
            load["cloudaicompanionProject"] = json!(project);
        }
        let loaded = self.cloud_call(access, "loadCodeAssist", &load).await?;
        let project = self
            .project_id
            .clone()
            .or_else(|| response_project(&loaded));
        if loaded
            .get("currentTier")
            .is_some_and(|tier| !tier.is_null())
        {
            if let Some(project) = project {
                return Ok(project);
            }
        }
        let tier = loaded
            .get("allowedTiers")
            .and_then(Value::as_array)
            .and_then(|tiers| {
                tiers
                    .iter()
                    .find(|tier| tier["isDefault"].as_bool() == Some(true))
                    .and_then(|tier| tier["id"].as_str())
            })
            .unwrap_or("legacy-tier");
        let mut onboard = json!({"tierId":tier, "metadata":metadata});
        if let Some(project) = &project {
            onboard["cloudaicompanionProject"] = json!(project);
        }
        let task = async {
            loop {
                let result = self.cloud_call(access, "onboardUser", &onboard).await?;
                if result["done"].as_bool() == Some(true) {
                    return response_project(&result["response"])
                        .or(project)
                        .ok_or_else(|| {
                            invalid(
                                "onboarding returned no project; configure GOOGLE_CLOUD_PROJECT",
                            )
                        });
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        };
        tokio::time::timeout(Duration::from_secs(30), task)
            .await
            .map_err(|_| invalid("project onboarding timed out; retry login"))?
    }
}

fn authorization_code(input: &str, expected_state: &str) -> Result<String, AuthError> {
    let input = input.trim();
    if input.is_empty() || input.len() > 8192 {
        return Err(invalid("authorization code is missing or invalid"));
    }
    if input.contains("://") {
        let url = reqwest::Url::parse(input).map_err(|_| invalid("invalid callback URL"))?;
        let expected = reqwest::Url::parse(REDIRECT_URI).expect("constant redirect URL");
        if url.origin() != expected.origin()
            || url.path() != expected.path()
            || url.fragment().is_some()
        {
            return Err(invalid("unexpected callback URL"));
        }
        let query: Vec<_> = url.query_pairs().collect();
        let unique = |key| {
            let values: Vec<_> = query
                .iter()
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value.as_ref())
                .collect();
            if values.len() == 1 {
                Some(values[0])
            } else {
                None
            }
        };
        if unique("state") != Some(expected_state) || query.iter().any(|(key, _)| key == "error") {
            return Err(invalid("callback state mismatch or authorization denied"));
        }
        return unique("code")
            .filter(|code| !code.is_empty())
            .map(str::to_string)
            .ok_or_else(|| invalid("callback code is missing"));
    }
    if input.chars().any(char::is_whitespace) {
        return Err(invalid("invalid authorization code"));
    }
    // Manual codes from Google's authcode page are bound to this session by PKCE.
    Ok(input.into())
}

fn response_project(value: &Value) -> Option<String> {
    let project = value.get("cloudaicompanionProject")?;
    project
        .as_str()
        .or_else(|| project.get("id")?.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn invalid(message: &str) -> AuthError {
    AuthError::InvalidResponse(format!("Gemini {message}"))
}
fn network() -> AuthError {
    AuthError::Network("Gemini authorization request failed".into())
}

async fn read_json(response: reqwest::Response) -> Result<Value, AuthError> {
    let status = response.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            400 | 401 => AuthError::ExpiredOrInvalidGrant,
            429 => AuthError::RateLimited {
                retry_after_ms: None,
            },
            _ => invalid(&format!(
                "authorization request failed (HTTP {})",
                status.as_u16()
            )),
        });
    }
    let mut body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|_| network())?;
        if body.len().saturating_add(chunk.len()) > 1024 * 1024 {
            return Err(invalid("authorization response exceeds size limit"));
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| invalid("invalid authorization response"))
}

fn parse_token(value: Value, current: Option<&Token>) -> Result<Token, AuthError> {
    let access = value["access_token"]
        .as_str()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid("response has no access token"))?;
    let seconds = value["expires_in"]
        .as_i64()
        .filter(|seconds| *seconds > 0)
        .ok_or_else(|| invalid("response has no valid expiry"))?;
    let expiry = chrono::Duration::try_seconds(seconds)
        .and_then(|duration| Utc::now().checked_add_signed(duration))
        .ok_or_else(|| invalid("invalid token expiry"))?;
    let mut token = current.cloned().unwrap_or(Token {
        provider_metadata: None,
        access_token: String::new(),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: None,
        scopes: None,
        account_id: None,
    });
    token.access_token = access.into();
    if let Some(refresh) = value["refresh_token"]
        .as_str()
        .filter(|value| !value.is_empty())
    {
        token.refresh_token = Some(refresh.into());
    }
    if let Some(id) = value["id_token"].as_str() {
        token.id_token = Some(id.into());
    }
    if let Some(scope) = value["scope"].as_str() {
        token.scopes = Some(scope.split_whitespace().map(str::to_string).collect());
    }
    if token
        .refresh_token
        .as_ref()
        .is_none_or(|refresh| refresh.is_empty())
    {
        return Err(invalid(
            "response has no refresh token; authorize offline access again",
        ));
    }
    token.expires_at = Some(expiry);
    token.last_refresh = Some(Utc::now());
    Ok(token)
}

fn set_project(token: &mut Token, project: String) {
    token.provider_metadata = Some(ProviderTokenMetadata::Gemini {
        project_id: project,
    });
}
pub(crate) fn project_id(token: &Token) -> Option<&str> {
    match token.provider_metadata.as_ref()? {
        ProviderTokenMetadata::Gemini { project_id } => Some(project_id),
    }
}

pub struct GeminiBackend;

#[async_trait]
impl AuthBackend for GeminiBackend {
    fn aliases(&self) -> &[&str] {
        &["google", "gemini"]
    }
    fn display_name(&self) -> &str {
        "Gemini"
    }
    fn store_key(&self) -> &str {
        "gemini"
    }
    fn canonical_provider_key(&self) -> &str {
        "google"
    }
    fn oauth_flow(&self) -> CredentialFlow {
        CredentialFlow::Pkce
    }
    async fn start_login(&self, store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
        GeminiAuth::new(store.clone()).start_auth()
    }
    async fn poll_device_code(
        &self,
        _store: &Arc<dyn TokenStore>,
        _session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        Err(AuthError::Unsupported("Gemini uses browser PKCE".into()))
    }
    async fn complete_pkce(
        &self,
        store: &Arc<dyn TokenStore>,
        code: &str,
        state: &str,
        session_data: &Value,
    ) -> Result<Token, AuthError> {
        GeminiAuth::new(store.clone())
            .exchange_code(code, state, session_data)
            .await
    }
    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        store.load("gemini", "default")
    }
    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError> {
        store.clear("gemini", "default")
    }
}

#[cfg(test)]
mod tests;
