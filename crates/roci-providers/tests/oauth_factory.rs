//! Local HTTP checks for credential selection through the public provider registry.
#![cfg(any(feature = "openai", feature = "anthropic", feature = "github-copilot"))]

use chrono::{Duration, Utc};
use roci_core::auth::{FileTokenStore, Token, TokenStore, TokenStoreConfig};
use roci_core::config::RociConfig;
use roci_core::provider::ProviderRegistry;
#[cfg(any(feature = "anthropic", feature = "github-copilot"))]
use roci_core::provider::ProviderRequest;
#[cfg(any(feature = "anthropic", feature = "github-copilot"))]
use roci_core::types::{GenerationSettings, ModelMessage};
use std::sync::Arc;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn token(access: &str) -> Token {
    Token {
        access_token: access.into(),
        refresh_token: None,
        id_token: None,
        expires_at: Some(Utc::now() + Duration::hours(1)),
        last_refresh: Some(Utc::now()),
        scopes: None,
        account_id: None,
        provider_metadata: None,
    }
}

fn configuration() -> (TempDir, Arc<dyn TokenStore>, RociConfig, ProviderRegistry) {
    let dir = TempDir::new().unwrap();
    let store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
        dir.path().into(),
    )));
    let config = RociConfig::new()
        .with_token_store(Some(store.clone()))
        .with_provider_credential_store(None);
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);
    (dir, store, config, registry)
}

#[cfg(any(feature = "anthropic", feature = "github-copilot"))]
fn request() -> ProviderRequest {
    ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: GenerationSettings::default(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: Default::default(),
        metadata: Default::default(),
        payload_callback: None,
        session_id: None,
        transport: None,
    }
}

#[cfg(feature = "anthropic")]
#[tokio::test]
async fn anthropic_factory_uses_oauth_bearer_and_override_uses_api_key() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST"))
        .and(path("/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":"message", "type":"message", "role":"assistant", "model":"claude-sonnet-4",
            "content":[{"type":"text","text":"ok"}], "stop_reason":"end_turn",
            "usage":{"input_tokens":1,"output_tokens":1}
        })))
        .expect(2)
        .mount(&server)
        .await;
    let (_dir, store, config, registry) = configuration();
    store
        .save("claude-code", "default", &token("oauth-secret"))
        .unwrap();
    config.set_base_url("anthropic", server.uri());
    let provider = registry
        .create_provider("anthropic", "claude-sonnet-4", &config)
        .unwrap();
    assert_eq!(provider.generate_text(&request()).await.unwrap().text, "ok");
    // Removing the saved OAuth credential also proves an explicit key bypasses storage.
    store.clear("claude-code", "default").unwrap();
    let mut overridden = request();
    overridden.api_key_override = Some("explicit-secret".into());
    assert_eq!(
        provider.generate_text(&overridden).await.unwrap().text,
        "ok"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["authorization"], "Bearer oauth-secret");
    assert!(!requests[0].headers.contains_key("x-api-key"));
    assert!(requests[0].headers["anthropic-beta"]
        .to_str()
        .unwrap()
        .contains("oauth-2025-04-20"));
    assert_eq!(requests[1].headers["x-api-key"], "explicit-secret");
    assert!(!requests[1].headers.contains_key("authorization"));
    assert!(!requests[1].headers["anthropic-beta"]
        .to_str()
        .unwrap()
        .contains("oauth-2025-04-20"));
}

#[cfg(feature = "github-copilot")]
#[tokio::test]
async fn copilot_factory_uses_named_derived_token_and_its_endpoint() {
    let server = MockServer::builder().start().await;
    Mock::given(method("POST")).and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id":"chat", "object":"chat.completion", "created":0, "model":"gpt-4o",
            "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}
        }))).expect(1).mount(&server).await;
    let (_dir, store, config, registry) = configuration();
    store
        .save("github-copilot", "default", &token("default-primary"))
        .unwrap();
    store
        .save("github-copilot-api", "default", &token("default-derived"))
        .unwrap();
    let config = config.with_account("work").unwrap();
    let scoped = config.token_store().unwrap();
    scoped
        .save("github-copilot", "default", &token("work-primary"))
        .unwrap();
    let mut derived = token("work-derived");
    derived.account_id = Some(server.uri());
    scoped
        .save("github-copilot-api", "default", &derived)
        .unwrap();
    let provider = registry
        .create_provider("github-copilot", "gpt-4o", &config)
        .unwrap();
    assert_eq!(provider.generate_text(&request()).await.unwrap().text, "ok");
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests[0].headers["authorization"], "Bearer work-derived");
    assert_eq!(requests[0].headers["copilot-integration-id"], "vscode-chat");
    assert_eq!(
        store
            .load("github-copilot-api", "default")
            .unwrap()
            .unwrap()
            .access_token,
        "default-derived"
    );
}

#[cfg(feature = "openai")]
#[tokio::test]
async fn codex_catalog_uses_selected_account_and_never_falls_back_to_static_models() {
    use roci_core::error::RociError;
    use roci_core::models::ModelListOptions;
    use wiremock::matchers::header;

    let server = MockServer::builder().start().await;
    Mock::given(method("GET")).and(path("/models"))
        .and(header("authorization", "Bearer work-codex-token"))
        .and(header("chatgpt-account-id", "work-upstream-account"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models":[{"slug":"gpt-future-account-2099", "display_name":"Future account model", "visibility":"list", "supported_in_api":false}]
        }))).expect(1).mount(&server).await;
    let (_dir, store, config, registry) = configuration();
    let mut primary = token("default-codex-token");
    primary.account_id = Some("default-upstream-account".into());
    store.save("openai-codex", "default", &primary).unwrap();
    let work = config.with_account("work").unwrap();
    work.set_base_url("codex", server.uri());
    let mut selected = token("work-codex-token");
    selected.account_id = Some("work-upstream-account".into());
    work.token_store()
        .unwrap()
        .save("openai-codex", "default", &selected)
        .unwrap();
    let options = ModelListOptions {
        provider_key: Some("codex".into()),
        ..Default::default()
    };
    let catalog = registry.list_models(&work, &options).await.unwrap();
    assert_eq!(catalog.models().len(), 1);
    assert_eq!(catalog.models()[0].model_id, "gpt-future-account-2099");
    assert_eq!(
        store.load("openai-codex", "default").unwrap(),
        Some(primary)
    );

    let missing = work.clone().with_account("missing").unwrap();
    assert!(matches!(
        registry.list_models(&missing, &options).await,
        Err(RociError::MissingCredential { .. })
    ));
    let disabled = ModelListOptions {
        include_dynamic: false,
        ..options
    };
    assert!(registry
        .list_models(&work, &disabled)
        .await
        .unwrap()
        .models()
        .is_empty());
    assert!(registry
        .list_models(&missing, &disabled)
        .await
        .unwrap()
        .models()
        .is_empty());
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[cfg(feature = "github-copilot")]
#[tokio::test]
async fn copilot_catalog_uses_same_endpoint_override_as_execution() {
    use roci_core::models::ModelListOptions;
    use wiremock::matchers::header;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/models"))
        .and(header("authorization", "Bearer scoped-derived"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data":[{"id":"account-override-model"}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let (_dir, _store, config, registry) = configuration();
    let config = config.with_account("work").unwrap();
    let scoped = config.token_store().unwrap();
    scoped
        .save("github-copilot", "default", &token("scoped-primary"))
        .unwrap();
    let mut derived = token("scoped-derived");
    derived.account_id = Some("https://must-not-contact.invalid".into());
    scoped
        .save("github-copilot-api", "default", &derived)
        .unwrap();
    config.set_base_url("github-copilot", server.uri());
    let catalog = registry
        .list_models(
            &config,
            &ModelListOptions {
                provider_key: Some("github-copilot".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(catalog.models().len(), 1);
    assert_eq!(catalog.models()[0].model_id, "account-override-model");
}
