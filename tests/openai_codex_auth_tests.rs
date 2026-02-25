mod auth_support;

use std::fs;
use std::sync::Arc;

use chrono::{Duration, Utc};
use roci::auth::providers::openai_codex::OpenAiCodexAuth;
use roci::auth::{AuthError, DeviceCodePoll, DeviceCodeSession, Token};
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use auth_support::InMemoryTokenStore;

fn codex_auth(store: Arc<InMemoryTokenStore>, issuer: &str) -> OpenAiCodexAuth {
    OpenAiCodexAuth::new(store).with_issuer(issuer.to_string())
}

fn active_session(interval_secs: u64) -> DeviceCodeSession {
    DeviceCodeSession {
        provider: "openai-codex".to_string(),
        verification_url: "https://auth.openai.com/codex/device".to_string(),
        user_code: "ABCD-1234".to_string(),
        device_code: "device-auth-id".to_string(),
        interval_secs,
        expires_at: Utc::now() + Duration::minutes(10),
    }
}

fn write_auth_json(temp: &TempDir, contents: &str) {
    fs::write(temp.path().join("auth.json"), contents).expect("write auth.json");
}

fn stale_token(refresh_token: Option<&str>) -> Token {
    Token {
        access_token: "old-access".to_string(),
        refresh_token: refresh_token.map(ToString::to_string),
        id_token: Some("old-id".to_string()),
        expires_at: None,
        last_refresh: Some(Utc::now() - Duration::days(9)),
        scopes: None,
        account_id: Some("acct_123".to_string()),
    }
}

#[tokio::test]
async fn codex_start_device_code_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/usercode"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_auth_id": "device-auth-id",
            "user_code": "ABCD-1234",
            "interval": "11"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = codex_auth(store, &server.uri());
    let session = auth.start_device_code().await.expect("start device code");

    assert_eq!(session.provider, "openai-codex");
    assert_eq!(session.device_code, "device-auth-id");
    assert_eq!(session.user_code, "ABCD-1234");
    assert_eq!(session.interval_secs, 11);
    assert_eq!(
        session.verification_url,
        format!("{}/codex/device", server.uri())
    );
}

#[tokio::test]
async fn codex_start_device_code_handles_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/usercode"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = codex_auth(store, &server.uri());
    let result = auth.start_device_code().await;

    assert!(
        matches!(result, Err(AuthError::Unsupported(message)) if message.contains("not enabled"))
    );
}

#[tokio::test]
async fn codex_poll_pending_returns_interval() {
    let server = MockServer::start().await;
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = codex_auth(store, &server.uri());
    let session = active_session(8);

    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/token"))
        .respond_with(ResponseTemplate::new(403))
        .expect(1)
        .mount(&server)
        .await;
    let pending = auth.poll_device_code(&session).await.expect("pending");
    assert!(matches!(
        pending,
        DeviceCodePoll::Pending { interval_secs: 8 }
    ));
}

#[tokio::test]
async fn codex_poll_not_found_returns_pending() {
    let server = MockServer::start().await;
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = codex_auth(store, &server.uri());
    let session = active_session(8);

    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/token"))
        .respond_with(ResponseTemplate::new(404))
        .expect(1)
        .mount(&server)
        .await;
    let pending = auth.poll_device_code(&session).await.expect("pending");
    assert!(matches!(
        pending,
        DeviceCodePoll::Pending { interval_secs: 8 }
    ));
}

#[tokio::test]
async fn codex_poll_authorized_exchanges_and_persists_token() {
    let server = MockServer::start().await;
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = codex_auth(store.clone(), &server.uri());
    let session = active_session(8);

    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authorization_code": "auth-code",
            "code_verifier": "code-verifier"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id_token": "new-id-token",
            "access_token": "new-access-token",
            "refresh_token": "new-refresh-token"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let authorized = auth.poll_device_code(&session).await.expect("authorized");
    let token = match authorized {
        DeviceCodePoll::Authorized { token } => token,
        other => panic!("expected authorized, got {other:?}"),
    };

    assert_eq!(token.access_token, "new-access-token");
    assert_eq!(token.refresh_token.as_deref(), Some("new-refresh-token"));
    assert_eq!(token.id_token.as_deref(), Some("new-id-token"));
    assert_eq!(
        store
            .get("openai-codex", "default")
            .expect("token persisted")
            .access_token,
        "new-access-token"
    );
}

#[tokio::test]
async fn codex_poll_exchange_failure_is_returned() {
    let server = MockServer::start().await;
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = codex_auth(store, &server.uri());
    let session = active_session(5);

    Mock::given(method("POST"))
        .and(path("/api/accounts/deviceauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "authorization_code": "auth-code",
            "code_verifier": "code-verifier"
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let result = auth.poll_device_code(&session).await;
    assert!(
        matches!(result, Err(AuthError::InvalidResponse(message)) if message.contains("Token exchange failed with status 500"))
    );
}

#[tokio::test]
async fn codex_poll_expired_session_short_circuits() {
    let server = MockServer::start().await;
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = codex_auth(store, &server.uri());
    let session = DeviceCodeSession {
        provider: "openai-codex".to_string(),
        verification_url: "https://auth.openai.com/codex/device".to_string(),
        user_code: "ABCD-1234".to_string(),
        device_code: "device-auth-id".to_string(),
        interval_secs: 5,
        expires_at: Utc::now() - Duration::seconds(1),
    };

    let result = auth.poll_device_code(&session).await.expect("expired");
    assert!(matches!(result, DeviceCodePoll::Expired));
}

#[tokio::test]
async fn codex_import_auth_json_handles_missing_empty_and_invalid() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = OpenAiCodexAuth::new(store.clone());

    let missing = TempDir::new().expect("tempdir");
    let missing_result = auth
        .import_codex_auth_json(Some(missing.path().to_path_buf()))
        .expect("missing file should be ignored");
    assert!(missing_result.is_none());

    let no_tokens = TempDir::new().expect("tempdir");
    write_auth_json(
        &no_tokens,
        r#"{
          "tokens": null,
          "last_refresh": "2024-01-01T00:00:00Z"
        }"#,
    );
    let no_tokens_result = auth
        .import_codex_auth_json(Some(no_tokens.path().to_path_buf()))
        .expect("null tokens should be ignored");
    assert!(no_tokens_result.is_none());

    let malformed = TempDir::new().expect("tempdir");
    write_auth_json(&malformed, "{not-json");
    let malformed_result = auth.import_codex_auth_json(Some(malformed.path().to_path_buf()));
    assert!(matches!(malformed_result, Err(AuthError::Serialization(_))));
}

#[tokio::test]
async fn codex_import_auth_json_success_persists_token() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = OpenAiCodexAuth::new(store.clone());
    let temp = TempDir::new().expect("tempdir");

    write_auth_json(
        &temp,
        r#"{
          "tokens": {
            "access_token": "import-access",
            "refresh_token": "import-refresh",
            "id_token": "import-id",
            "account_id": "acct_42"
          },
          "last_refresh": "2024-01-01T00:00:00Z"
        }"#,
    );
    let imported = auth
        .import_codex_auth_json(Some(temp.path().to_path_buf()))
        .expect("import should succeed")
        .expect("token should exist");

    assert_eq!(imported.access_token, "import-access");
    assert_eq!(imported.refresh_token.as_deref(), Some("import-refresh"));
    assert_eq!(imported.id_token.as_deref(), Some("import-id"));
    assert_eq!(imported.account_id.as_deref(), Some("acct_42"));
    assert_eq!(
        store
            .get("openai-codex", "default")
            .expect("stored token")
            .access_token,
        "import-access"
    );
}

#[tokio::test]
async fn codex_get_token_refresh_success_updates_store() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "refreshed-access",
            "refresh_token": "refreshed-refresh",
            "id_token": "refreshed-id"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    store.seed("openai-codex", "default", stale_token(Some("old-refresh")));
    let auth = OpenAiCodexAuth::new(store.clone())
        .with_refresh_token_url_override(format!("{}/oauth/token", server.uri()));

    let token = auth.get_token().await.expect("refresh should succeed");

    assert_eq!(token.access_token, "refreshed-access");
    assert_eq!(token.refresh_token.as_deref(), Some("refreshed-refresh"));
    assert_eq!(token.id_token.as_deref(), Some("refreshed-id"));
    assert_eq!(token.account_id.as_deref(), Some("acct_123"));
    assert!(token.last_refresh.is_some());

    let stored = store
        .get("openai-codex", "default")
        .expect("token persisted");
    assert_eq!(stored.access_token, "refreshed-access");
    assert_eq!(stored.account_id.as_deref(), Some("acct_123"));
}

#[tokio::test]
async fn codex_get_token_refresh_invalid_grant_maps_to_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/oauth/token"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(r#"{"error":"refresh_token_expired"}"#),
        )
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    store.seed("openai-codex", "default", stale_token(Some("old-refresh")));
    let auth = OpenAiCodexAuth::new(store)
        .with_refresh_token_url_override(format!("{}/oauth/token", server.uri()));

    let result = auth.get_token().await;
    assert!(matches!(result, Err(AuthError::ExpiredOrInvalidGrant)));
}

#[tokio::test]
async fn codex_get_token_refresh_without_refresh_token_is_invalid_grant() {
    let store = Arc::new(InMemoryTokenStore::new());
    store.seed("openai-codex", "default", stale_token(None));
    let auth = OpenAiCodexAuth::new(store);

    let result = auth.get_token().await;
    assert!(matches!(result, Err(AuthError::ExpiredOrInvalidGrant)));
}
