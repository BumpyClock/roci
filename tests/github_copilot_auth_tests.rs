mod auth_support;

use std::sync::Arc;

use chrono::{Duration, Utc};
use roci::auth::providers::github_copilot::GitHubCopilotAuth;
use roci::auth::{AuthError, DeviceCodePoll, DeviceCodeSession};
use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use auth_support::{token, InMemoryTokenStore};

fn active_session(interval_secs: u64) -> DeviceCodeSession {
    DeviceCodeSession {
        provider: "github-copilot".to_string(),
        verification_url: "https://github.com/login/device".to_string(),
        user_code: "ABCD-EFGH".to_string(),
        device_code: "device-code-1".to_string(),
        interval_secs,
        expires_at: Utc::now() + Duration::minutes(10),
    }
}

fn copilot_auth(store: Arc<InMemoryTokenStore>, server: &MockServer) -> GitHubCopilotAuth {
    GitHubCopilotAuth::new(store)
        .with_device_code_url(format!("{}/login/device/code", server.uri()))
        .with_access_token_url(format!("{}/login/oauth/access_token", server.uri()))
        .with_copilot_token_url(format!("{}/copilot_internal/v2/token", server.uri()))
}

#[tokio::test]
async fn copilot_start_device_code_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/device/code"))
        .and(header("accept", "application/json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code": "device-123",
            "user_code": "ABCD-EFGH",
            "verification_uri": "https://github.com/login/device",
            "expires_in": 900,
            "interval": 5
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);
    let session = auth.start_device_code().await.expect("start device code");

    assert_eq!(session.provider, "github-copilot");
    assert_eq!(session.device_code, "device-123");
    assert_eq!(session.user_code, "ABCD-EFGH");
    assert_eq!(session.interval_secs, 5);
    assert!(session.expires_at > Utc::now());
}

#[tokio::test]
async fn copilot_poll_pending_returns_pending() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "authorization_pending"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);
    let result = auth
        .poll_device_code(&active_session(7))
        .await
        .expect("pending");

    assert!(matches!(
        result,
        DeviceCodePoll::Pending { interval_secs: 7 }
    ));
}

#[tokio::test]
async fn copilot_poll_expired_token_response_returns_expired() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "expired_token"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);
    let result = auth
        .poll_device_code(&active_session(7))
        .await
        .expect("expired token");

    assert!(matches!(result, DeviceCodePoll::Expired));
}

#[tokio::test]
async fn copilot_poll_slow_down_adds_two_seconds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "slow_down"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);
    let result = auth
        .poll_device_code(&active_session(7))
        .await
        .expect("slow_down");

    assert!(matches!(
        result,
        DeviceCodePoll::SlowDown { interval_secs: 9 }
    ));
}

#[tokio::test]
async fn copilot_poll_denied_returns_access_denied() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "access_denied"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);
    let result = auth
        .poll_device_code(&active_session(7))
        .await
        .expect("access denied");

    assert!(matches!(result, DeviceCodePoll::AccessDenied));
}

#[tokio::test]
async fn copilot_poll_authorized_saves_token_with_scopes() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "ghu_123",
            "scope": "read:user,repo"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store.clone(), &server);
    let result = auth
        .poll_device_code(&active_session(7))
        .await
        .expect("authorized");

    let token = match result {
        DeviceCodePoll::Authorized { token } => token,
        other => panic!("expected authorized, got {other:?}"),
    };
    assert_eq!(token.access_token, "ghu_123");
    assert_eq!(
        token.scopes.expect("scope list"),
        vec!["read:user".to_string(), "repo".to_string()]
    );
    assert_eq!(
        store
            .get("github-copilot", "default")
            .expect("stored token")
            .access_token,
        "ghu_123"
    );
}

#[tokio::test]
async fn copilot_poll_expired_session_short_circuits() {
    let server = MockServer::start().await;
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);
    let expired_session = DeviceCodeSession {
        provider: "github-copilot".to_string(),
        verification_url: "https://github.com/login/device".to_string(),
        user_code: "ABCD-EFGH".to_string(),
        device_code: "device-code-1".to_string(),
        interval_secs: 5,
        expires_at: Utc::now() - Duration::seconds(1),
    };

    let result = auth
        .poll_device_code(&expired_session)
        .await
        .expect("expired poll");
    assert!(matches!(result, DeviceCodePoll::Expired));
}

#[tokio::test]
async fn copilot_poll_unknown_error_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "error": "unknown_error"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);
    let result = auth.poll_device_code(&active_session(5)).await;

    assert!(
        matches!(result, Err(AuthError::InvalidResponse(message)) if message.contains("unknown_error"))
    );
}

#[tokio::test]
async fn copilot_poll_missing_error_and_token_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);
    let result = auth.poll_device_code(&active_session(5)).await;

    assert!(
        matches!(result, Err(AuthError::InvalidResponse(message)) if message.contains("missing token"))
    );
}

#[tokio::test]
async fn copilot_poll_non_success_status_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);
    let result = auth.poll_device_code(&active_session(5)).await;

    assert!(
        matches!(result, Err(AuthError::InvalidResponse(message)) if message.contains("status 500"))
    );
}

#[tokio::test]
async fn copilot_exchange_requires_login() {
    let server = MockServer::start().await;
    let store = Arc::new(InMemoryTokenStore::new());
    let auth = copilot_auth(store, &server);

    let result = auth.exchange_copilot_token().await;
    assert!(matches!(result, Err(AuthError::NotLoggedIn)));
}

#[tokio::test]
async fn copilot_exchange_unauthorized_maps_to_invalid_grant() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/copilot_internal/v2/token"))
        .respond_with(ResponseTemplate::new(401))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    store.seed("github-copilot", "default", token("ghu_123"));
    let auth = copilot_auth(store, &server);

    let result = auth.exchange_copilot_token().await;
    assert!(matches!(result, Err(AuthError::ExpiredOrInvalidGrant)));
}

#[tokio::test]
async fn copilot_exchange_non_success_status_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/copilot_internal/v2/token"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    store.seed("github-copilot", "default", token("ghu_123"));
    let auth = copilot_auth(store, &server);

    let result = auth.exchange_copilot_token().await;
    assert!(
        matches!(result, Err(AuthError::InvalidResponse(message)) if message.contains("status 500"))
    );
}

#[tokio::test]
async fn copilot_exchange_success_derives_base_url_and_uses_cache() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/copilot_internal/v2/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "copilot-token;proxy-ep=https://proxy.individual.githubcopilot.com",
            "expires_at": "4102444800"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    store.seed("github-copilot", "default", token("ghu_123"));
    let auth = copilot_auth(store, &server);

    let first = auth.exchange_copilot_token().await.expect("first exchange");
    let second = auth
        .exchange_copilot_token()
        .await
        .expect("cached exchange");

    assert_eq!(
        first.token,
        "copilot-token;proxy-ep=https://proxy.individual.githubcopilot.com"
    );
    assert_eq!(first.base_url, "https://api.individual.githubcopilot.com");
    assert_eq!(second.token, first.token);
    assert_eq!(second.base_url, first.base_url);
    server.verify().await;
}

#[tokio::test]
async fn copilot_exchange_refreshes_when_cached_token_near_expiry() {
    let server = MockServer::start().await;
    let expires_at = Utc::now().timestamp() + 240;
    Mock::given(method("GET"))
        .and(path("/copilot_internal/v2/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "copilot-short-lived",
            "expires_at": expires_at
        })))
        .expect(2)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    store.seed("github-copilot", "default", token("ghu_123"));
    let auth = copilot_auth(store, &server);

    let first = auth.exchange_copilot_token().await.expect("first exchange");
    let second = auth
        .exchange_copilot_token()
        .await
        .expect("second exchange");

    assert_eq!(first.token, "copilot-short-lived");
    assert_eq!(second.token, "copilot-short-lived");
    server.verify().await;
}

#[tokio::test]
async fn copilot_exchange_invalid_expires_at_is_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/copilot_internal/v2/token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "token": "copilot-token",
            "expires_at": {"bad": "shape"}
        })))
        .expect(1)
        .mount(&server)
        .await;

    let store = Arc::new(InMemoryTokenStore::new());
    store.seed("github-copilot", "default", token("ghu_123"));
    let auth = copilot_auth(store, &server);

    let result = auth.exchange_copilot_token().await;
    assert!(
        matches!(result, Err(AuthError::InvalidResponse(message)) if message.contains("expires_at missing"))
    );
}
