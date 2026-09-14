//! Bounded, authenticated discovery for OpenAI-compatible model catalogs.

#![cfg(feature = "openai")]

use std::time::Duration;

use roci_core::{
    config::RociConfig,
    error::RociError,
    models::{
        ModelCapabilities, ModelCatalog, ModelCatalogSource, ModelInfo, ModelListOptions,
        ModelPolicy,
    },
};
use serde_json::Value;

fn discovery_error(provider: &str, message: String) -> RociError {
    RociError::Provider {
        provider: provider.into(),
        message,
    }
}

const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;

/// Resolve key and endpoint together before the first network await.
pub(crate) async fn list_configured_models(
    config: &RociConfig,
    provider_key: &str,
    options: &ModelListOptions,
    default_base_url: &str,
    capabilities: fn(&str) -> ModelCapabilities,
) -> Result<ModelCatalog, RociError> {
    if !options.include_dynamic {
        return Ok(ModelCatalog::default());
    }
    let resolved = config.resolve_provider_credential(provider_key)?;
    let (key, base_url) = match resolved {
        Some(roci_core::auth::ResolvedProviderCredential {
            material: roci_core::auth::CredentialMaterial::ApiKey(key),
            endpoint,
        }) => (
            Some(key.expose_secret().to_string()),
            endpoint.map(|url| url.as_str().to_string()),
        ),
        Some(roci_core::auth::ResolvedProviderCredential {
            material: roci_core::auth::CredentialMaterial::OAuth(_),
            ..
        }) => {
            return Err(RociError::ModelDiscoveryUnsupported {
                provider: provider_key.into(),
                reason: "OAuth model discovery has no verified native endpoint; use an API-key account for catalog discovery".into(),
            });
        }
        None => (None, None),
    };
    let Some(key) = key.filter(|key| !key.trim().is_empty()) else {
        return Err(RociError::MissingCredential {
            provider: provider_key.into(),
        });
    };
    let endpoint = models_endpoint(base_url.as_deref().unwrap_or(default_base_url))?;
    fetch_openai_models(provider_key, &endpoint, Some(&key), capabilities, false).await
}

/// Append the model-list path while preserving endpoint query configuration.
pub(crate) fn models_endpoint(base_url: &str) -> Result<String, RociError> {
    let mut endpoint = reqwest::Url::parse(base_url)
        .map_err(|_| RociError::Configuration("invalid model catalog endpoint".into()))?;
    let path = format!("{}/models", endpoint.path().trim_end_matches('/'));
    endpoint.set_path(&path);
    Ok(endpoint.into())
}

/// Fetch only upstream-advertised IDs. There is no bundled catalog fallback.
pub(crate) async fn fetch_openai_models(
    provider_key: &str,
    endpoint: &str,
    api_key: Option<&str>,
    capabilities: fn(&str) -> ModelCapabilities,
    local: bool,
) -> Result<ModelCatalog, RociError> {
    let url = reqwest::Url::parse(endpoint)
        .map_err(|_| RociError::Configuration("invalid model catalog endpoint".into()))?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(RociError::Configuration(
            "model catalog endpoint must be HTTP(S) without embedded credentials".into(),
        ));
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| {
            RociError::Configuration("could not initialize model catalog client".into())
        })?;
    let mut request = client
        .get(url.clone())
        .header(reqwest::header::ACCEPT, "application/json");
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let mut response = request.send().await.map_err(|_| {
        discovery_error(
            provider_key,
            format!("{provider_key} model catalog request failed"),
        )
    })?;
    let status = response.status();
    if !status.is_success() {
        // Error bodies and endpoint query strings may contain credentials.
        return Err(RociError::api(
            status.as_u16(),
            format!("{provider_key} model catalog request failed ({status})"),
        ));
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_CATALOG_BYTES as u64)
    {
        return Err(discovery_error(
            provider_key,
            "model catalog response exceeds size limit".into(),
        ));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        discovery_error(
            provider_key,
            "model catalog response could not be read".into(),
        )
    })? {
        if body.len().saturating_add(chunk.len()) > MAX_CATALOG_BYTES {
            return Err(discovery_error(
                provider_key,
                "model catalog response exceeds size limit".into(),
            ));
        }
        body.extend_from_slice(&chunk);
    }
    let payload: Value = serde_json::from_slice(&body).map_err(|_| {
        discovery_error(
            provider_key,
            "model catalog response is not valid JSON".into(),
        )
    })?;
    let mut source = url;
    source.set_query(None);
    source.set_fragment(None);
    parse_models(provider_key, source.as_str(), payload, capabilities, local)
}

fn parse_models(
    provider_key: &str,
    endpoint: &str,
    payload: Value,
    capabilities: fn(&str) -> ModelCapabilities,
    local: bool,
) -> Result<ModelCatalog, RociError> {
    let entries = payload
        .as_array()
        .or_else(|| payload.get("data").and_then(Value::as_array))
        .ok_or_else(|| {
            discovery_error(
                provider_key,
                "model catalog response is missing its model array".into(),
            )
        })?;
    let mut catalog = ModelCatalog::default();
    for entry in entries {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                discovery_error(
                    provider_key,
                    "model catalog contains a missing or empty model ID".into(),
                )
            })?;
        let mut caps = capabilities(id);
        if let Some(context) = entry
            .get("context_length")
            .or_else(|| entry.get("context_window"))
            .or_else(|| entry.get("max_context_length"))
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n > 0)
        {
            caps.context_length = context;
        }
        if let Some(max) = entry
            .get("max_completion_tokens")
            .or_else(|| entry.pointer("/top_provider/max_completion_tokens"))
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n > 0)
        {
            caps.max_output_tokens = Some(max);
        }
        if let Some(vision) = entry
            .pointer("/capabilities/vision")
            .and_then(Value::as_bool)
        {
            caps.supports_vision = vision;
            caps.input = roci_core::models::ModelInputCapabilities::from_vision_support(vision);
        }
        if let Some(tools) = entry
            .pointer("/capabilities/function_calling")
            .and_then(Value::as_bool)
        {
            caps.supports_tools = tools;
        }
        if let Some(parameters) = entry.get("supported_parameters").and_then(Value::as_array) {
            caps.supports_tools = parameters
                .iter()
                .any(|p| matches!(p.as_str(), Some("tools" | "tool_choice")));
        }
        let mut metadata = std::collections::BTreeMap::new();
        for key in [
            "owned_by",
            "created",
            "description",
            "type",
            "active",
            "supported_parameters",
        ] {
            if let Some(value) = entry.get(key) {
                metadata.insert(key.into(), value.clone());
            }
        }
        catalog.insert(ModelInfo {
            provider_key: provider_key.into(),
            model_id: id.into(),
            display_name: Some(
                entry
                    .get("name")
                    .or_else(|| entry.get("display_name"))
                    .and_then(Value::as_str)
                    .unwrap_or(id)
                    .into(),
            ),
            capabilities: caps,
            policy: ModelPolicy {
                requires_credentials: !local,
                local,
                deprecated: entry
                    .get("deprecated")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                default_for_provider: false,
            },
            source: ModelCatalogSource::Dynamic {
                endpoint: endpoint.into(),
            },
            metadata,
        });
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    fn caps(_: &str) -> ModelCapabilities {
        ModelCapabilities::default()
    }

    #[tokio::test]
    async fn remote_catalog_preserves_unknown_ids_metadata_and_empty_results() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/v1/models"))
            .and(header("authorization", "Bearer account-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[
                {"id":"future-unlisted-model", "name":"Future", "context_length":77777, "top_provider":{"max_completion_tokens":321}, "capabilities":{"vision":true,"function_calling":true}},
                {"id":"another-model"}
            ]}))).expect(1).mount(&server).await;
        let endpoint = format!("{}/v1/models?secret=must-not-persist", server.uri());
        let catalog = fetch_openai_models("test", &endpoint, Some("account-key"), caps, false)
            .await
            .unwrap();
        assert_eq!(catalog.models().len(), 2);
        let model = catalog
            .models()
            .iter()
            .find(|m| m.model_id == "future-unlisted-model")
            .unwrap();
        assert_eq!(model.display_name.as_deref(), Some("Future"));
        assert_eq!(model.capabilities.context_length, 77777);
        assert_eq!(model.capabilities.max_output_tokens, Some(321));
        assert!(model.capabilities.supports_vision && model.capabilities.supports_tools);
        assert!(!serde_json::to_string(&catalog)
            .unwrap()
            .contains("must-not-persist"));
        server.reset().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[]})))
            .mount(&server)
            .await;
        assert!(fetch_openai_models("test", &endpoint, None, caps, true)
            .await
            .unwrap()
            .models()
            .is_empty());
    }

    #[tokio::test]
    async fn remote_catalog_rejects_missing_ids_and_redacts_status_errors() {
        let server = MockServer::start().await;
        for body in [
            json!({"unexpected":[]}),
            json!({"data":[{}]}),
            json!({"data":[{"id":" "}]}),
            json!({"data":[{"id":42}]}),
        ] {
            server.reset().await;
            Mock::given(method("GET"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .mount(&server)
                .await;
            assert!(
                fetch_openai_models("test", &server.uri(), None, caps, false)
                    .await
                    .is_err()
            );
        }
        for status in [401, 403, 429, 500, 302] {
            server.reset().await;
            Mock::given(method("GET"))
                .respond_with(
                    ResponseTemplate::new(status)
                        .insert_header("location", "https://example.invalid/credential-secret")
                        .set_body_string("upstream-credential-secret"),
                )
                .mount(&server)
                .await;
            let error = fetch_openai_models(
                "test",
                &format!("{}?key=query-secret", server.uri()),
                Some("bearer-secret"),
                caps,
                false,
            )
            .await
            .unwrap_err();
            assert!(matches!(error, RociError::Api {status: actual,..} if actual == status));
            let text = format!("{error:?} {error}");
            for secret in ["credential-secret", "query-secret", "bearer-secret"] {
                assert!(!text.contains(secret));
            }
        }
    }

    #[test]
    fn remote_catalog_accepts_together_array_and_preserves_query_parameters() {
        let catalog = parse_models(
            "together",
            "https://example.test/models",
            json!([{"id":"new/model"}]),
            caps,
            false,
        )
        .unwrap();
        assert_eq!(catalog.models()[0].model_id, "new/model");
        assert_eq!(
            models_endpoint("https://example.test/v1/?api-version=one").unwrap(),
            "https://example.test/v1/models?api-version=one"
        );
    }

    #[tokio::test]
    async fn configured_remote_catalog_honors_filters_without_static_fallback() {
        let config = RociConfig::new()
            .with_token_store(None)
            .with_provider_credential_store(None);
        for include_unavailable in [false, true] {
            let options = ModelListOptions {
                include_unavailable,
                ..Default::default()
            };
            assert!(matches!(
                list_configured_models(
                    &config,
                    "openai",
                    &options,
                    "https://unused.invalid/v1",
                    caps
                )
                .await,
                Err(RociError::MissingCredential { .. })
            ));
            let options = ModelListOptions {
                include_dynamic: false,
                ..options
            };
            assert!(list_configured_models(
                &config,
                "openai",
                &options,
                "https://unused.invalid/v1",
                caps
            )
            .await
            .unwrap()
            .models()
            .is_empty());
        }
    }

    #[tokio::test]
    async fn configured_remote_catalog_uses_selected_account_key_and_endpoint() {
        use roci_core::auth::{
            InMemoryProviderCredentialStore, ProviderApiKey, ProviderCredentialRecord,
            ProviderEndpoint,
        };
        use std::sync::Arc;
        let first = MockServer::start().await;
        let second = MockServer::start().await;
        let store = Arc::new(InMemoryProviderCredentialStore::default());
        let default = RociConfig::new()
            .with_token_store(None)
            .with_provider_credential_store(Some(store));
        let work = default.clone().with_account("work").unwrap();
        for (config, server, key, id) in [
            (&default, &first, "default-key", "default-only"),
            (&work, &second, "work-key", "work-only"),
        ] {
            config
                .provider_credential_store()
                .unwrap()
                .save(
                    "openai",
                    &ProviderCredentialRecord::new(
                        ProviderApiKey::new(key),
                        Some(ProviderEndpoint::new(format!("{}/v1", server.uri()))),
                    ),
                )
                .unwrap();
            Mock::given(method("GET"))
                .and(path("/v1/models"))
                .and(header("authorization", format!("Bearer {key}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":[{"id":id}]})))
                .expect(1)
                .mount(server)
                .await;
        }
        for (config, id) in [(&default, "default-only"), (&work, "work-only")] {
            let catalog = list_configured_models(
                config,
                "openai",
                &ModelListOptions::default(),
                "https://unused.invalid/v1",
                caps,
            )
            .await
            .unwrap();
            assert_eq!(catalog.models()[0].model_id, id);
        }
    }
}

#[cfg(all(test, feature = "openai"))]
mod factory_tests {
    use super::*;
    use roci_core::provider::ProviderFactory;
    use wiremock::{
        matchers::{header, method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn openai_family_factories_discover_configured_endpoint_without_whitelist() {
        let factories: Vec<(&str, Box<dyn ProviderFactory>)> = vec![
            ("openai", Box::new(crate::factories::OpenAiFactory)),
            #[cfg(feature = "grok")]
            ("grok", Box::new(crate::factories::GrokFactory)),
            #[cfg(feature = "groq")]
            ("groq", Box::new(crate::factories::GroqFactory)),
            #[cfg(feature = "mistral")]
            ("mistral", Box::new(crate::factories::MistralFactory)),
            #[cfg(feature = "openrouter")]
            ("openrouter", Box::new(crate::factories::OpenRouterFactory)),
            #[cfg(feature = "together")]
            ("together", Box::new(crate::factories::TogetherFactory)),
            #[cfg(feature = "openai-compatible")]
            (
                "openai-compatible",
                Box::new(crate::factories::OpenAiCompatibleFactory),
            ),
        ];
        for (provider, factory) in factories {
            let server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path("/v1/models"))
                .and(header("authorization", "Bearer dedicated-key"))
                .respond_with(ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"data":[{"id":"not-in-any-static-catalog"}]}),
                ))
                .expect(1)
                .mount(&server)
                .await;
            let config = RociConfig::new()
                .with_token_store(None)
                .with_provider_credential_store(None);
            config.set_api_key(provider, "dedicated-key".into());
            config.set_base_url(provider, format!("{}/v1", server.uri()));
            let catalog = factory
                .list_models(&config, provider, &ModelListOptions::default())
                .await
                .unwrap();
            assert_eq!(catalog.models().len(), 1, "{provider}");
            assert_eq!(catalog.models()[0].provider_key, provider);
            assert_eq!(catalog.models()[0].model_id, "not-in-any-static-catalog");
        }
    }

    #[tokio::test]
    async fn explicit_discovery_surfaces_missing_configuration_even_when_including_unavailable() {
        let factories: Vec<(&str, Box<dyn ProviderFactory>)> = vec![
            ("openai", Box::new(crate::factories::OpenAiFactory)),
            #[cfg(feature = "openai-compatible")]
            (
                "openai-compatible",
                Box::new(crate::factories::OpenAiCompatibleFactory),
            ),
            #[cfg(feature = "azure")]
            ("azure", Box::new(crate::factories::AzureFactory)),
        ];
        for (provider, factory) in factories {
            for include_unavailable in [false, true] {
                let config = RociConfig::new()
                    .with_token_store(None)
                    .with_provider_credential_store(None);
                let options = ModelListOptions {
                    include_unavailable,
                    ..Default::default()
                };
                assert!(
                    factory
                        .list_models(&config, provider, &options)
                        .await
                        .is_err(),
                    "{provider}"
                );
                if provider != "openai" {
                    config.set_api_key(provider, "configured-key-without-endpoint".into());
                    assert!(
                        factory
                            .list_models(&config, provider, &options)
                            .await
                            .is_err(),
                        "{provider}"
                    );
                }
                let options = ModelListOptions {
                    include_dynamic: false,
                    ..options
                };
                assert!(factory
                    .list_models(&config, provider, &options)
                    .await
                    .unwrap()
                    .models()
                    .is_empty());
            }
        }
    }

    #[cfg(feature = "grok")]
    #[tokio::test]
    async fn grok_oauth_discovery_never_sends_native_token_to_api_key_endpoint() {
        use roci_core::auth::{FileTokenStore, Token, TokenStore, TokenStoreConfig};
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(FileTokenStore::new(TokenStoreConfig::new(
            dir.path().into(),
        )));
        let server = MockServer::start().await;
        store
            .save(
                "xai",
                "default",
                &Token {
                    access_token: "native-oauth-secret".into(),
                    refresh_token: None,
                    id_token: None,
                    expires_at: None,
                    last_refresh: None,
                    scopes: None,
                    account_id: None,
                    provider_metadata: None,
                },
            )
            .unwrap();
        let config = RociConfig::new()
            .with_token_store(Some(store))
            .with_provider_credential_store(None);
        config.set_base_url("grok", server.uri());
        assert!(matches!(
            crate::factories::GrokFactory
                .list_models(&config, "grok", &ModelListOptions::default())
                .await,
            Err(RociError::ModelDiscoveryUnsupported { .. })
        ));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
