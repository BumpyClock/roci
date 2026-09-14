//! Credential selection is shared by discovery and generation, including endpoints.
#![cfg(feature = "openai")]

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use roci_core::{
    auth::{
        ProviderApiKey, ProviderCredentialRecord, ProviderCredentialStore,
        ProviderCredentialStoreError, ProviderEndpoint,
    },
    config::RociConfig,
    models::ModelListOptions,
    provider::{ProviderRegistry, ProviderRequest},
    types::{GenerationSettings, ModelMessage},
};
use wiremock::{
    matchers::{header, method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

struct RotatingStore {
    first: ProviderCredentialRecord,
    second: ProviderCredentialRecord,
    loads: AtomicUsize,
}

impl ProviderCredentialStore for RotatingStore {
    fn load(
        &self,
        _: &str,
    ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
        Ok(Some(if self.loads.fetch_add(1, Ordering::SeqCst) == 0 {
            self.first.clone()
        } else {
            self.second.clone()
        }))
    }
    fn save(
        &self,
        _: &str,
        _: &ProviderCredentialRecord,
    ) -> Result<(), ProviderCredentialStoreError> {
        unreachable!()
    }
    fn clear(&self, _: &str) -> Result<(), ProviderCredentialStoreError> {
        unreachable!()
    }
}

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

#[tokio::test]
async fn discovery_and_generation_use_one_paired_record_per_operation() {
    let providers = [
        "openai",
        #[cfg(feature = "grok")]
        "grok",
        #[cfg(feature = "groq")]
        "groq",
        #[cfg(feature = "mistral")]
        "mistral",
        #[cfg(feature = "openrouter")]
        "openrouter",
        #[cfg(feature = "together")]
        "together",
        #[cfg(feature = "anthropic")]
        "anthropic",
        #[cfg(feature = "google")]
        "google",
    ];
    for provider_key in providers {
        let server = MockServer::start().await;
        let other = MockServer::start().await;
        let store = Arc::new(RotatingStore {
            first: ProviderCredentialRecord::new(
                ProviderApiKey::new("first-key"),
                Some(ProviderEndpoint::new(format!("{}/v1", server.uri()))),
            ),
            second: ProviderCredentialRecord::new(
                ProviderApiKey::new("second-key"),
                Some(ProviderEndpoint::new(format!("{}/v1", other.uri()))),
            ),
            loads: AtomicUsize::new(0),
        });
        let config = RociConfig::new()
            .with_token_store(None)
            .with_provider_credential_store(Some(store.clone()));
        let mut registry = ProviderRegistry::new();
        roci_providers::register_default_providers(&mut registry);
        let catalog = if provider_key == "google" {
            serde_json::json!({"models":[{"name":"models/future-model", "supportedGenerationMethods":["generateContent"]}]})
        } else {
            serde_json::json!({"data":[{"id":"future-model"}]})
        };
        let generation = match provider_key {
            "google" => {
                serde_json::json!({"candidates":[{"content":{"parts":[{"text":"ok"}]},"finishReason":"STOP"}]})
            }
            "anthropic" => {
                serde_json::json!({"id":"message", "type":"message", "role":"assistant", "model":"future-model", "content":[{"type":"text","text":"ok"}], "stop_reason":"end_turn", "usage":{"input_tokens":1,"output_tokens":1}})
            }
            _ => {
                serde_json::json!({"id":"completion", "object":"chat.completion", "created":0, "model":"future-model", "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}], "usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}})
            }
        };
        let generation_path = match provider_key {
            "google" => "/v1/models/future-model:generateContent",
            "anthropic" => "/v1/messages",
            _ => "/v1/chat/completions",
        };
        for (verb, route, body) in [
            ("GET", "/v1/models", catalog),
            ("POST", generation_path, generation),
        ] {
            let mock = Mock::given(method(verb)).and(path(route));
            let mock = match provider_key {
                "google" if verb == "GET" => mock.and(header("x-goog-api-key", "first-key")),
                "google" => mock.and(query_param("key", "first-key")),
                "anthropic" => mock.and(header("x-api-key", "first-key")),
                _ => mock.and(header("authorization", "Bearer first-key")),
            };
            mock.respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;
        }
        let listed = registry
            .list_models(
                &config,
                &ModelListOptions {
                    provider_key: Some(provider_key.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            listed.models()[0].model_id,
            "future-model",
            "{provider_key}"
        );
        assert_eq!(
            store.loads.swap(0, Ordering::SeqCst),
            1,
            "discovery {provider_key}"
        );
        let provider = registry
            .create_provider(provider_key, "future-model", &config)
            .unwrap();
        assert_eq!(
            provider.generate_text(&request()).await.unwrap().text,
            "ok",
            "{provider_key}"
        );
        assert_eq!(
            store.loads.load(Ordering::SeqCst),
            1,
            "generation {provider_key}"
        );
        assert!(other.received_requests().await.unwrap().is_empty());
    }
}

struct UnreadableStore;
impl ProviderCredentialStore for UnreadableStore {
    fn load(
        &self,
        _: &str,
    ) -> Result<Option<ProviderCredentialRecord>, ProviderCredentialStoreError> {
        Err(ProviderCredentialStoreError::InvalidRecord)
    }
    fn save(
        &self,
        _: &str,
        _: &ProviderCredentialRecord,
    ) -> Result<(), ProviderCredentialStoreError> {
        unreachable!()
    }
    fn clear(&self, _: &str) -> Result<(), ProviderCredentialStoreError> {
        unreachable!()
    }
}

#[tokio::test]
async fn storage_failure_is_preserved_by_discovery_and_construction() {
    let config = RociConfig::new()
        .with_token_store(None)
        .with_provider_credential_store(Some(Arc::new(UnreadableStore)));
    let mut registry = ProviderRegistry::new();
    roci_providers::register_default_providers(&mut registry);
    for provider in registry.provider_keys() {
        let create_error = match registry.create_provider(provider, "future-model", &config) {
            Err(error) => error,
            Ok(_) => panic!("{provider} suppressed store error"),
        };
        assert!(
            create_error
                .to_string()
                .contains("provider credential store could not be read"),
            "{provider}: {create_error}"
        );
        let discovery_error = registry
            .list_models(
                &config,
                &ModelListOptions {
                    provider_key: Some(provider.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap_err();
        assert!(
            discovery_error
                .to_string()
                .contains("provider credential store could not be read"),
            "{provider}: {discovery_error}"
        );
    }
}
