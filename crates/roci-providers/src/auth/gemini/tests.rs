use super::*;
use roci_core::auth::{FileTokenStore, TokenStoreConfig};
use tempfile::TempDir;
use wiremock::{
    matchers::{body_string_contains, header, method, path},
    Mock, MockServer, ResponseTemplate,
};

fn auth(server: &MockServer) -> (TempDir, GeminiAuth) {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
        dir.path().into(),
    )));
    let mut auth = GeminiAuth::new(store)
        .with_oauth_client("public-client".into(), "public-client-secret".into())
        .with_endpoints(format!("{}/token", server.uri()), server.uri());
    auth.project_id = None;
    (dir, auth)
}

fn session(auth: &GeminiAuth) -> (String, Value) {
    let AuthStep::Pkce {
        state,
        session_data,
        ..
    } = auth.start_auth().unwrap()
    else {
        panic!("expected PKCE")
    };
    (state, session_data)
}

#[tokio::test]
async fn authorization_uses_random_state_and_pkce_s256() {
    let server = MockServer::start().await;
    let (_dir, auth) = auth(&server);
    let AuthStep::Pkce {
        authorize_url,
        state,
        session_data,
    } = auth.start_auth().unwrap()
    else {
        panic!("PKCE")
    };
    let url = reqwest::Url::parse(&authorize_url).unwrap();
    let query: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    assert_eq!(query["client_id"], "public-client");
    assert!(!authorize_url.contains("public-client-secret"));
    assert_eq!(query["state"], state);
    assert_eq!(query["redirect_uri"], REDIRECT_URI);
    assert_eq!(query["access_type"], "offline");
    assert_eq!(query["code_challenge_method"], "S256");
    assert_eq!(
        query["code_challenge"],
        URL_SAFE_NO_PAD.encode(Sha256::digest(
            session_data["verifier"].as_str().unwrap().as_bytes()
        ))
    );
    assert!(!authorize_url.contains(session_data["verifier"].as_str().unwrap()));
    let (other, _) = session(&auth);
    assert_ne!(state, other);
}

#[tokio::test]
async fn missing_client_configuration_fails_before_login_exchange_or_refresh_network() {
    let server = MockServer::start().await;
    let (_dir, configured) = auth(&server);
    let (state, data) = session(&configured);
    let token = parse_token(
        json!({"access_token":"access", "refresh_token":"refresh", "expires_in":3600}),
        None,
    )
    .unwrap();
    for (client_id, client_secret) in [
        ("", "synthetic-secret"),
        ("  ", "synthetic-secret"),
        ("synthetic-client", ""),
        ("synthetic-client", " \t"),
    ] {
        let (_dir, auth) = auth(&server);
        let auth = auth.with_oauth_client(client_id.into(), client_secret.into());
        let errors = [
            auth.start_auth().unwrap_err(),
            auth.exchange_code("code", &state, &data).await.unwrap_err(),
            auth.refresh_token(&token).await.unwrap_err(),
        ];
        for error in errors {
            let message = error.to_string();
            assert!(message.contains("ROCI_GEMINI_OAUTH_CLIENT_ID"));
            assert!(message.contains("ROCI_GEMINI_OAUTH_CLIENT_SECRET"));
            assert!(!message.contains("synthetic-"));
        }
    }
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn exchange_validates_session_before_network_and_rejects_callback_state() {
    let server = MockServer::start().await;
    let (_dir, auth) = auth(&server);
    let (state, data) = session(&auth);
    for input in [
        format!("{REDIRECT_URI}?code=secret&state=wrong"),
        format!("{REDIRECT_URI}?code=secret&state={state}&state={state}"),
        format!("https://wrong.example/authcode?code=secret&state={state}"),
    ] {
        let error = auth.exchange_code(&input, &state, &data).await.unwrap_err();
        assert!(!error.to_string().contains("secret"));
    }
    assert!(auth
        .exchange_code("code", "wrong-session", &data)
        .await
        .is_err());
    assert!(server.received_requests().await.unwrap().is_empty());
}

#[tokio::test]
async fn exchange_discovers_project_then_persists_typed_metadata() {
    let server = MockServer::start().await;
    let (_dir, auth) = auth(&server);
    let (state, data) = session(&auth);
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code_verifier="))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"access_token":"access", "refresh_token":"refresh", "expires_in":3600}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/v1internal:loadCodeAssist"))
        .and(header("authorization", "Bearer access"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"currentTier":{"id":"free-tier"}, "cloudaicompanionProject":"project-one"}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    let result = auth
        .exchange_code("displayed-code", &state, &data)
        .await
        .unwrap();
    assert_eq!(project_id(&result), Some("project-one"));
    assert_eq!(auth.store.load("gemini", "default").unwrap(), Some(result));
}

#[tokio::test]
async fn onboarding_uses_default_tier_and_accepts_project_object() {
    let server = MockServer::start().await;
    let (_dir, auth) = auth(&server);
    Mock::given(path("/v1internal:loadCodeAssist"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"allowedTiers":[{"id":"free-tier", "isDefault":true}]})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/v1internal:onboardUser")).and(body_string_contains("free-tier"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"done":true,"response":{"cloudaicompanionProject":{"id":"provisioned-project"}}}))).expect(1).mount(&server).await;
    assert_eq!(
        auth.discover_project("access").await.unwrap(),
        "provisioned-project"
    );
}

#[tokio::test]
async fn refresh_preserves_rotation_metadata_and_does_not_persist() {
    let server = MockServer::start().await;
    let (_dir, auth) = auth(&server);
    let mut old = parse_token(
        json!({"access_token":"old", "refresh_token":"old-refresh", "expires_in":3600}),
        None,
    )
    .unwrap();
    set_project(&mut old, "project-one".into());
    auth.store.save("gemini", "default", &old).unwrap();
    Mock::given(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .and(body_string_contains("refresh_token=old-refresh"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token":"new", "expires_in":3600})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let result = auth.refresh_token(&old).await.unwrap();
    assert_eq!(result.access_token, "new");
    assert_eq!(result.refresh_token.as_deref(), Some("old-refresh"));
    assert_eq!(project_id(&result), Some("project-one"));
    assert_eq!(auth.store.load("gemini", "default").unwrap(), Some(old));
}

#[tokio::test]
async fn failed_onboarding_does_not_persist_partially_ready_login() {
    let server = MockServer::start().await;
    let (_dir, auth) = auth(&server);
    let (state, data) = session(&auth);
    Mock::given(path("/token")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token":"secret-access", "refresh_token":"secret-refresh", "expires_in":3600}))).mount(&server).await;
    Mock::given(path("/v1internal:loadCodeAssist"))
        .respond_with(ResponseTemplate::new(403).set_body_string("secret-access secret-refresh"))
        .mount(&server)
        .await;
    let error = auth.exchange_code("code", &state, &data).await.unwrap_err();
    assert!(!error.to_string().contains("secret-"));
    assert!(auth.store.load("gemini", "default").unwrap().is_none());
}

#[test]
fn invalid_expiry_and_empty_tokens_are_rejected() {
    for value in [
        json!({"access_token":"", "expires_in":3600}),
        json!({"access_token":"a", "expires_in":-1}),
        json!({"access_token":"a", "expires_in":i64::MAX}),
    ] {
        assert!(parse_token(value, None).is_err());
    }
}
