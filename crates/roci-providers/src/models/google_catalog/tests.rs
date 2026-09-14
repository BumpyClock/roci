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
    config.set_base_url("google", server.uri());
    config
}

#[tokio::test]
async fn api_catalog_pages_unknown_ids_and_uses_live_limits() {
    let server = MockServer::start().await;
    Mock::given(method("GET")).and(path("/v1beta/models"))
        .and(header("x-goog-api-key", "google-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models":[
                {"name":"models/gemini-future", "displayName":"Future Gemini", "inputTokenLimit":1234567,"outputTokenLimit":45678,"supportedGenerationMethods":["generateContent"]},
                {"name":"models/embedding-only", "supportedGenerationMethods":["embedContent"]}
            ],"nextPageToken":"page-two"
        }))).up_to_n_times(1).expect(1).mount(&server).await;
    Mock::given(method("GET")).and(path("/v1beta/models"))
        .and(query_param("pageToken", "page-two"))
        .and(header("x-goog-api-key", "google-secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models":[{"name":"models/gemini-other-future", "supportedGenerationMethods":["generateContent"]}]
        }))).expect(1).mount(&server).await;
    let config = config(&server);
    config.set_api_key("google", "google-secret".into());
    let catalog = crate::factories::GoogleFactory
        .list_models(&config, "google", &ModelListOptions::default())
        .await
        .unwrap();
    assert_eq!(catalog.models().len(), 2);
    let future = catalog
        .models()
        .iter()
        .find(|model| model.model_id == "gemini-future")
        .unwrap();
    assert_eq!(future.capabilities.context_length, 1234567);
    assert_eq!(future.capabilities.max_output_tokens, Some(45678));
    assert_eq!(future.display_name.as_deref(), Some("Future Gemini"));
    assert!(matches!(future.source, ModelCatalogSource::Dynamic { .. }));
    for request in server.received_requests().await.unwrap() {
        assert!(!request.url.as_str().contains("google-secret"));
        assert!(!request.headers.contains_key("authorization"));
    }
}

#[tokio::test]
async fn native_oauth_discovery_reports_unavailable_without_fabricating_models() {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let store: Arc<dyn TokenStore> = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
        dir.path().into(),
    )));
    store
        .save(
            "gemini",
            "work",
            &Token {
                access_token: "oauth-secret".into(),
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
    let config = config(&server)
        .with_token_store(Some(store))
        .with_account("work")
        .unwrap();
    assert!(matches!(
        list_models(&config, "google", &ModelListOptions::default()).await,
        Err(RociError::ModelDiscoveryUnsupported { .. })
    ));
    assert!(server.received_requests().await.unwrap().is_empty());
    // The explicit API key selects the supported public listing protocol.
    config.set_api_key("google", "explicit-google-key".into());
    Mock::given(method("GET"))
        .and(path("/v1beta/models"))
        .and(header("x-goog-api-key", "explicit-google-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"models":[]})))
        .expect(1)
        .mount(&server)
        .await;
    assert!(list_models(&config, "google", &ModelListOptions::default())
        .await
        .unwrap()
        .models()
        .is_empty());
}

#[tokio::test]
async fn catalog_fails_on_http_error_without_static_results_or_body_leak() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).set_body_string("private quota details"))
        .mount(&server)
        .await;
    let config = config(&server);
    config.set_api_key("google", "google-secret".into());
    let err = list_models(&config, "google", &ModelListOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, RociError::Api { status: 429, .. }));
    assert!(!err.to_string().contains("private"));
    let options = ModelListOptions {
        include_dynamic: false,
        ..Default::default()
    };
    assert!(list_models(&config, "google", &options)
        .await
        .unwrap()
        .models()
        .is_empty());
}

#[tokio::test]
async fn catalog_rejects_repeating_pagination_and_invalid_success() {
    for body in [
        serde_json::json!({"models":[],"nextPageToken":"repeating"}),
        serde_json::json!({"unexpected":[]}),
    ] {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(&server)
            .await;
        let config = config(&server);
        config.set_api_key("google", "google-secret".into());
        assert!(list_models(&config, "google", &ModelListOptions::default())
            .await
            .is_err());
        assert!(server.received_requests().await.unwrap().len() <= 2);
    }
}

#[tokio::test]
async fn endpoint_queries_survive_pagination_without_leaking_into_catalog_or_errors() {
    let server = MockServer::start().await;
    let cursor = "next /?&=+#%";
    // A root URL still gets the native versioned route before its query.
    let endpoint = format!(
        "{}/?gateway_token=query-secret&api-version=2026#private-fragment",
        server.uri()
    );
    Mock::given(method("GET")).and(path("/v1beta/models"))
        .and(query_param("gateway_token", "query-secret"))
        .and(query_param("api-version", "2026"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models":[{"name":"models/first", "supportedGenerationMethods":["generateContent"]}],"nextPageToken":cursor
        }))).up_to_n_times(1).expect(1).mount(&server).await;
    Mock::given(method("GET"))
        .and(path("/v1beta/models"))
        .and(query_param("gateway_token", "query-secret"))
        .and(query_param("api-version", "2026"))
        .and(query_param("pageToken", cursor))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "models":[{"name":"models/second", "supportedGenerationMethods":["generateContent"]}]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let catalog = fetch("google", &endpoint, "key").await.unwrap();
    assert_eq!(catalog.models().len(), 2);
    for model in catalog.models() {
        assert_eq!(
            model.source,
            ModelCatalogSource::Dynamic {
                endpoint: format!("{}/v1beta/models", server.uri())
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
    for key in ["key", "invalid\nheader"] {
        let error = fetch(
            "google",
            &format!("{}/missing?gateway_token=query-secret", server.uri()),
            key,
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
        "https://user:user-secret@example.test/v1beta",
        "file:///private-secret",
        "not a URL private-secret",
    ] {
        let error = fetch("google", endpoint, "key").await.unwrap_err();
        let visible = format!("{error:?} {error}");
        assert!(!visible.contains("user-secret"));
        assert!(!visible.contains("private-secret"));
    }
}
