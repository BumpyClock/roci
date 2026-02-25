mod auth_support;

use std::fs;
use std::sync::Arc;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{Duration, Utc};
use roci::auth::providers::claude_code::ClaudeCodeAuth;
use roci::auth::{AuthError, Token};
use reqwest::Url;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use auth_support::InMemoryTokenStore;

fn write_claude_credentials(temp: &TempDir, contents: &str) {
    let path = temp.path().join(".claude");
    fs::create_dir_all(&path).expect("create .claude dir");
    fs::write(path.join(".credentials.json"), contents).expect("write credentials");
}

#[tokio::test]
async fn claude_import_missing_file_returns_none() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store.clone());
    let temp = TempDir::new().expect("tempdir");

    let imported = auth
        .import_cli_credentials(Some(temp.path().to_path_buf()))
        .expect("import should not fail");

    assert!(imported.is_none());
    assert!(!auth.logged_in().await.expect("logged_in"));
    assert!(matches!(
        auth.get_token().await,
        Err(AuthError::NotLoggedIn)
    ));
}

#[tokio::test]
async fn claude_import_without_oauth_payload_returns_none() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store.clone());
    let temp = TempDir::new().expect("tempdir");
    write_claude_credentials(&temp, r#"{"claudeAiOauth":null}"#);

    let imported = auth
        .import_cli_credentials(Some(temp.path().to_path_buf()))
        .expect("import should not fail");

    assert!(imported.is_none());
    assert!(!auth.logged_in().await.expect("logged_in"));
    assert!(store.get("claude-code", "default").is_none());
}

#[tokio::test]
async fn claude_import_success_persists_token_and_get_token_works() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store.clone());
    let temp = TempDir::new().expect("tempdir");

    write_claude_credentials(
        &temp,
        r#"{
          "claudeAiOauth": {
            "accessToken": "claude-access",
            "refreshToken": "claude-refresh",
            "expiresAt": 4102444800000
          }
        }"#,
    );

    let imported = auth
        .import_cli_credentials(Some(temp.path().to_path_buf()))
        .expect("import should succeed")
        .expect("token should be imported");

    assert_eq!(imported.access_token, "claude-access");
    assert_eq!(imported.refresh_token.as_deref(), Some("claude-refresh"));
    assert_eq!(
        imported.expires_at.expect("expires_at").timestamp(),
        4_102_444_800
    );
    assert!(imported.last_refresh.is_some());

    assert!(auth.logged_in().await.expect("logged_in"));
    let loaded = auth.get_token().await.expect("token should exist");
    assert_eq!(loaded.access_token, "claude-access");
    assert_eq!(loaded.refresh_token.as_deref(), Some("claude-refresh"));
}

#[tokio::test]
async fn claude_import_honors_profile_isolation() {
    let store = Arc::new(InMemoryTokenStore::new());
    let default_auth = ClaudeCodeAuth::new(store.clone());
    let work_auth = ClaudeCodeAuth::new(store.clone()).with_profile("work");
    let temp = TempDir::new().expect("tempdir");

    write_claude_credentials(
        &temp,
        r#"{
          "claudeAiOauth": {
            "accessToken": "work-access",
            "refreshToken": "work-refresh",
            "expiresAt": 4102444800000
          }
        }"#,
    );

    let imported = work_auth
        .import_cli_credentials(Some(temp.path().to_path_buf()))
        .expect("import should succeed");

    assert!(imported.is_some());
    assert!(!default_auth.logged_in().await.expect("default logged_in"));
    assert!(work_auth.logged_in().await.expect("work logged_in"));
    assert_eq!(
        store
            .get("claude-code", "work")
            .expect("work token should exist")
            .access_token,
        "work-access"
    );
}

#[tokio::test]
async fn claude_import_invalid_json_returns_serialization_error() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store);
    let temp = TempDir::new().expect("tempdir");
    write_claude_credentials(&temp, "{not-json");

    let result = auth.import_cli_credentials(Some(temp.path().to_path_buf()));

    assert!(matches!(result, Err(AuthError::Serialization(_))));
}

// ---------------------------------------------------------------------------
// PKCE OAuth flow tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn start_auth_produces_valid_authorize_url_with_required_params() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store);

    let session = auth.start_auth().expect("start_auth should succeed");

    assert!(!session.state.is_empty());
    assert!(session.state.len() == 64, "state should be 32-byte hex (64 chars)");
    assert!(session.code_verifier.len() >= 43);
    assert!(session.code_verifier.len() <= 128);

    let url = Url::parse(&session.authorize_url).expect("valid URL");
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    assert_eq!(params.get("response_type").map(|v| v.as_ref()), Some("code"));
    assert_eq!(params.get("code_challenge_method").map(|v| v.as_ref()), Some("S256"));
    assert!(params.contains_key("client_id"));
    assert!(params.contains_key("redirect_uri"));
    assert!(params.contains_key("scope"));
    assert!(params.contains_key("state"));
    assert!(params.contains_key("code_challenge"));
    assert_eq!(params.get("state").map(|v| v.as_ref()), Some(session.state.as_str()));
}

#[tokio::test]
async fn start_auth_code_challenge_is_sha256_of_verifier() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store);

    let session = auth.start_auth().expect("start_auth should succeed");

    let expected_challenge = {
        let digest = Sha256::digest(session.code_verifier.as_bytes());
        URL_SAFE_NO_PAD.encode(digest)
    };

    let url = Url::parse(&session.authorize_url).expect("valid URL");
    let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
    let actual_challenge = params.get("code_challenge").expect("code_challenge param");

    assert_eq!(actual_challenge.as_ref(), expected_challenge.as_str());
}

#[tokio::test]
async fn start_auth_generates_unique_sessions() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store);

    let session_a = auth.start_auth().expect("start_auth a");
    let session_b = auth.start_auth().expect("start_auth b");

    assert_ne!(session_a.state, session_b.state);
    assert_ne!(session_a.code_verifier, session_b.code_verifier);
}

#[tokio::test]
async fn exchange_code_rejects_mismatched_state() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store);
    let session = auth.start_auth().expect("start_auth");

    let result = auth
        .exchange_code(&session, &format!("some-code#wrong-state"))
        .await;

    assert!(
        matches!(result, Err(AuthError::InvalidResponse(ref msg)) if msg.contains("state")),
        "expected state mismatch error, got: {result:?}"
    );
}

#[tokio::test]
async fn exchange_code_posts_correct_params_and_returns_token() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::body_string_contains("grant_type=authorization_code"))
        .and(wiremock::matchers::body_string_contains("client_id="))
        .and(wiremock::matchers::body_string_contains("code=my-auth-code"))
        .and(wiremock::matchers::body_string_contains("code_verifier="))
        .and(wiremock::matchers::body_string_contains("redirect_uri="))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "claude-new-access",
            "refresh_token": "claude-new-refresh",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store.clone())
        .with_token_url(mock_server.uri());

    let session = auth.start_auth().expect("start_auth");
    let auth_response = format!("my-auth-code#{}", session.state);

    let token = auth
        .exchange_code(&session, &auth_response)
        .await
        .expect("exchange_code should succeed");

    assert_eq!(token.access_token, "claude-new-access");
    assert_eq!(token.refresh_token.as_deref(), Some("claude-new-refresh"));
    assert!(token.expires_at.is_some());

    let stored = store.get("claude-code", "default").expect("token stored");
    assert_eq!(stored.access_token, "claude-new-access");
}

#[tokio::test]
async fn exchange_code_accepts_code_without_state_fragment() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "access-no-state",
            "refresh_token": "refresh-no-state",
            "expires_in": 7200
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store)
        .with_token_url(mock_server.uri());
    let session = auth.start_auth().expect("start_auth");

    let token = auth
        .exchange_code(&session, "just-a-code")
        .await
        .expect("exchange_code should accept code-only format");

    assert_eq!(token.access_token, "access-no-state");
}

#[tokio::test]
async fn exchange_code_propagates_server_error() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "invalid_grant"
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store)
        .with_token_url(mock_server.uri());
    let session = auth.start_auth().expect("start_auth");

    let result = auth.exchange_code(&session, "bad-code").await;

    assert!(matches!(result, Err(AuthError::InvalidResponse(_))));
}

#[tokio::test]
async fn refresh_token_returns_new_token_on_success() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::body_string_contains("grant_type=refresh_token"))
        .and(wiremock::matchers::body_string_contains("refresh_token=old-refresh"))
        .and(wiremock::matchers::body_string_contains("client_id="))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "refreshed-access",
            "refresh_token": "refreshed-refresh",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store.clone())
        .with_token_url(mock_server.uri());

    let old_token = Token {
        access_token: "old-access".to_string(),
        refresh_token: Some("old-refresh".to_string()),
        id_token: None,
        expires_at: Some(Utc::now() - Duration::hours(1)),
        last_refresh: None,
        scopes: None,
        account_id: None,
    };

    let refreshed = auth
        .refresh_token(&old_token)
        .await
        .expect("refresh should succeed");

    assert_eq!(refreshed.access_token, "refreshed-access");
    assert_eq!(refreshed.refresh_token.as_deref(), Some("refreshed-refresh"));
    assert!(refreshed.expires_at.is_some());

    let stored = store.get("claude-code", "default").expect("token stored");
    assert_eq!(stored.access_token, "refreshed-access");
}

#[tokio::test]
async fn refresh_token_fails_when_no_refresh_token_present() {
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = ClaudeCodeAuth::new(store);

    let token_without_refresh = Token {
        access_token: "access".to_string(),
        refresh_token: None,
        id_token: None,
        expires_at: None,
        last_refresh: None,
        scopes: None,
        account_id: None,
    };

    let result = auth.refresh_token(&token_without_refresh).await;

    assert!(matches!(result, Err(AuthError::ExpiredOrInvalidGrant)));
}

#[tokio::test]
async fn get_token_auto_refreshes_expired_token() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::body_string_contains("grant_type=refresh_token"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "auto-refreshed-access",
            "refresh_token": "auto-refreshed-refresh",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let expired_token = Token {
        access_token: "expired-access".to_string(),
        refresh_token: Some("my-refresh".to_string()),
        id_token: None,
        expires_at: Some(Utc::now() - Duration::minutes(10)),
        last_refresh: None,
        scopes: None,
        account_id: None,
    };
    store.seed("claude-code", "default", expired_token);

    let auth = ClaudeCodeAuth::new(store.clone())
        .with_token_url(mock_server.uri());

    let token = auth.get_token().await.expect("get_token should auto-refresh");

    assert_eq!(token.access_token, "auto-refreshed-access");
    let stored = store.get("claude-code", "default").expect("token persisted");
    assert_eq!(stored.access_token, "auto-refreshed-access");
}

#[tokio::test]
async fn get_token_returns_valid_token_without_http_call() {
    let store = Arc::new(InMemoryTokenStore::new());
    let valid_token = Token {
        access_token: "still-valid".to_string(),
        refresh_token: Some("refresh".to_string()),
        id_token: None,
        expires_at: Some(Utc::now() + Duration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: None,
        account_id: None,
    };
    store.seed("claude-code", "default", valid_token);

    let auth = ClaudeCodeAuth::new(store);

    let token = auth.get_token().await.expect("get_token should succeed");

    assert_eq!(token.access_token, "still-valid");
}

#[tokio::test]
async fn get_token_within_grace_period_triggers_refresh() {
    let mock_server = wiremock::MockServer::start().await;

    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "access_token": "grace-refreshed",
            "refresh_token": "grace-refresh-tok",
            "expires_in": 3600
        })))
        .expect(1)
        .mount(&mock_server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let almost_expired_token = Token {
        access_token: "about-to-expire".to_string(),
        refresh_token: Some("my-refresh".to_string()),
        id_token: None,
        expires_at: Some(Utc::now() + Duration::minutes(3)),
        last_refresh: None,
        scopes: None,
        account_id: None,
    };
    store.seed("claude-code", "default", almost_expired_token);

    let auth = ClaudeCodeAuth::new(store.clone())
        .with_token_url(mock_server.uri());

    let token = auth.get_token().await.expect("get_token should refresh within grace period");

    assert_eq!(token.access_token, "grace-refreshed");
}
