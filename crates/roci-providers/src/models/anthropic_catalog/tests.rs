use super::*;
use roci_core::{
    auth::{FileTokenStore, Token, TokenStore, TokenStoreConfig},
    provider::ProviderFactory,
};
use std::sync::Arc;
use wiremock::{
    matchers::{header, method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

fn config(server: &MockServer) -> RociConfig {
    let config = RociConfig::new()
        .with_token_store(None)
        .with_provider_credential_store(None);
    config.set_base_url("anthropic", format!("{}/v1", server.uri()));
    config
}

#[tokio::test]
async fn api_catalog_fetches_all_pages_and_unknown_model_ids() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("x-api-key", "api-secret"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data":[{"id":"claude-future-model","display_name":"Future model"}],
            "has_more":true,"last_id":"claude-future-model"
        })))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("after_id", "claude-future-model"))
        .and(header("x-api-key", "api-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data":[{"id":"claude-another-model"}],"has_more":false
        })))
        .expect(1)
        .mount(&server)
        .await;
    let config = config(&server);
    config.set_api_key("anthropic", "api-secret".into());
    let catalog = crate::factories::AnthropicFactory
        .list_models(&config, "anthropic", &ModelListOptions::default())
        .await
        .unwrap();
    assert_eq!(catalog.models().len(), 2);
    assert!(catalog
        .models()
        .iter()
        .all(|model| matches!(model.source, ModelCatalogSource::Dynamic { .. })));
    assert!(catalog
        .models()
        .iter()
        .any(|model| model.model_id == "claude-future-model"
            && model.display_name.as_deref() == Some("Future model")));
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| !request.headers.contains_key("authorization")));
}

#[tokio::test]
async fn oauth_catalog_uses_selected_account_bearer_and_api_key_takes_precedence() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
        dir.path().into(),
    )));
    for (profile, access) in [("work", "work-oauth"), ("default", "personal-oauth")] {
        store
            .save(
                "claude-code",
                profile,
                &Token {
                    access_token: access.into(),
                    refresh_token: None,
                    id_token: None,
                    expires_at: Some(chrono::Utc::now() + chrono::Duration::hours(1)),
                    last_refresh: Some(chrono::Utc::now()),
                    scopes: None,
                    account_id: None,
                    provider_metadata: None,
                },
            )
            .unwrap();
    }
    let config = config(&server)
        .with_token_store(Some(store))
        .with_account("work")
        .unwrap();
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer work-oauth"))
        .and(header("anthropic-beta", "oauth-2025-04-20"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"data":[{"id":"oauth-model"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let catalog = list_models(&config, "anthropic", &ModelListOptions::default())
        .await
        .unwrap();
    assert_eq!(catalog.models()[0].model_id, "oauth-model");
    config.set_api_key("anthropic", "explicit-key".into());
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("x-api-key", "explicit-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"data":[{"id":"api-model"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let catalog = list_models(&config, "anthropic", &ModelListOptions::default())
        .await
        .unwrap();
    assert_eq!(catalog.models()[0].model_id, "api-model");
    let requests = server.received_requests().await.unwrap();
    assert!(!requests[0].headers.contains_key("x-api-key"));
    assert!(!requests[1].headers.contains_key("authorization"));
}

#[tokio::test]
async fn catalog_errors_have_no_static_fallback_or_provider_body_leak() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_string("sensitive provider details"))
        .mount(&server)
        .await;
    let config = config(&server);
    config.set_api_key("anthropic", "api-secret".into());
    let err = list_models(&config, "anthropic", &ModelListOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, RociError::Api { status: 403, .. }));
    assert!(!err.to_string().contains("sensitive"));
    let options = ModelListOptions {
        include_dynamic: false,
        ..Default::default()
    };
    assert!(list_models(&config, "anthropic", &options)
        .await
        .unwrap()
        .models()
        .is_empty());
}

#[tokio::test]
async fn catalog_rejects_repeating_pagination_and_malformed_success() {
    for body in [
        serde_json::json!({"data":[],"has_more":true,"last_id":"repeating"}),
        serde_json::json!({"unexpected":[]}),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let config = config(&server);
        config.set_api_key("anthropic", "api-secret".into());
        assert!(
            list_models(&config, "anthropic", &ModelListOptions::default())
                .await
                .is_err()
        );
        assert!(server.received_requests().await.unwrap().len() <= 2);
    }
}

#[cfg(feature = "anthropic-compatible")]
#[tokio::test]
async fn compatible_catalog_uses_dedicated_endpoint_and_key() {
    let server = MockServer::start().await;
    let config = config(&server);
    config.set_api_key("anthropic", "wrong-key".into());
    config.set_api_key("anthropic-compatible", "compatible-key".into());
    config.set_base_url(
        "anthropic-compatible",
        format!("{}/custom/v1", server.uri()),
    );
    Mock::given(method("GET"))
        .and(path("/custom/v1/models"))
        .and(header("x-api-key", "compatible-key"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"data":[{"id":"custom-model"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let catalog = crate::factories::AnthropicCompatibleFactory
        .list_models(
            &config,
            "anthropic-compatible",
            &ModelListOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(catalog.models()[0].provider_key, "anthropic-compatible");
    assert_eq!(catalog.models()[0].model_id, "custom-model");
}

#[tokio::test]
async fn endpoint_queries_survive_pagination_without_leaking_into_catalog_or_errors() {
    let server = MockServer::start().await;
    let cursor = "next /?&=+#%";
    let endpoint = format!(
        "{}/v1/?gateway_token=query-secret&api-version=2026#private-fragment",
        server.uri()
    );
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("gateway_token", "query-secret"))
        .and(query_param("api-version", "2026"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data":[{"id":"first"}],"has_more":true,"last_id":cursor
        })))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("gateway_token", "query-secret"))
        .and(query_param("api-version", "2026"))
        .and(query_param("after_id", cursor))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"data":[{"id":"second"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let catalog = fetch("anthropic", &endpoint, "key", false).await.unwrap();
    assert_eq!(catalog.models().len(), 2);
    for model in catalog.models() {
        assert_eq!(
            model.source,
            ModelCatalogSource::Dynamic {
                endpoint: format!("{}/v1/models", server.uri())
            }
        );
    }
    let serialized = serde_json::to_string(&catalog).unwrap();
    assert!(!serialized.contains("query-secret"));
    assert!(!serialized.contains("private-fragment"));
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|request| request.url.fragment().is_none()));

    // A failed request, including a rejected header at request construction,
    // must not retain the configured URL or credential in its error chain.
    for key in ["key", "invalid\nheader"] {
        let error = fetch(
            "anthropic",
            &format!("{}/missing?gateway_token=query-secret", server.uri()),
            key,
            false,
        )
        .await
        .unwrap_err();
        let visible = format!("{error:?} {error}");
        assert!(!visible.contains("query-secret"));
        assert!(!visible.contains("invalid\nheader"));
    }
}

#[tokio::test]
async fn endpoint_rejects_non_http_and_embedded_credentials_without_disclosure() {
    for endpoint in [
        "https://user:user-secret@example.test/v1",
        "file:///private-secret",
        "not a URL private-secret",
    ] {
        let error = fetch("anthropic", endpoint, "key", false)
            .await
            .unwrap_err();
        let visible = format!("{error:?} {error}");
        assert!(!visible.contains("user-secret"));
        assert!(!visible.contains("private-secret"));
    }
}
