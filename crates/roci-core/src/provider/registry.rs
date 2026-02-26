//! Provider registry for dynamic provider resolution.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::RociConfig;
use crate::error::RociError;
use super::{ModelProvider, ProviderFactory};

/// Registry mapping provider keys to their factories.
///
/// Used by the meta-crate and agent loop to create providers dynamically.
pub struct ProviderRegistry {
    factories: HashMap<String, Arc<dyn ProviderFactory>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Register a factory for all provider keys it declares.
    pub fn register(&mut self, factory: Arc<dyn ProviderFactory>) {
        for key in factory.provider_keys() {
            self.factories.insert(key.to_string(), factory.clone());
        }
    }

    /// Create a provider instance by looking up the registered factory.
    pub fn create_provider(
        &self,
        provider_key: &str,
        model_id: &str,
        config: &RociConfig,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        self.factories
            .get(provider_key)
            .ok_or_else(|| {
                RociError::ModelNotFound(format!(
                    "No provider factory registered for '{provider_key}'"
                ))
            })?
            .create(config, provider_key, model_id)
    }

    /// Check whether a factory is registered for the given key.
    pub fn has_provider(&self, provider_key: &str) -> bool {
        self.factories.contains_key(provider_key)
    }

    /// List all registered provider keys.
    pub fn provider_keys(&self) -> Vec<&str> {
        self.factories.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RociConfig;
    use crate::error::RociError;
    use crate::models::capabilities::ModelCapabilities;
    use crate::provider::{ModelProvider, ProviderRequest, ProviderResponse};
    use crate::types::{TextStreamDelta, Usage};
    use async_trait::async_trait;
    use futures::stream::BoxStream;
    use std::any::Any;

    struct StubFactory;

    impl ProviderFactory for StubFactory {
        fn provider_keys(&self) -> &[&str] {
            &["stub", "stub-alias"]
        }

        fn create(
            &self,
            _config: &RociConfig,
            _provider_key: &str,
            model_id: &str,
        ) -> Result<Box<dyn ModelProvider>, RociError> {
            Ok(Box::new(StubProvider {
                model_id: model_id.to_string(),
                caps: ModelCapabilities {
                    supports_vision: false,
                    supports_tools: false,
                    supports_streaming: false,
                    supports_json_mode: false,
                    supports_json_schema: false,
                    supports_reasoning: false,
                    supports_system_messages: true,
                    context_length: 4096,
                    max_output_tokens: None,
                },
            }))
        }

        fn parse_model(
            &self,
            _provider_key: &str,
            _model_id: &str,
        ) -> Option<Box<dyn Any + Send + Sync>> {
            None
        }
    }

    struct StubProvider {
        model_id: String,
        caps: ModelCapabilities,
    }

    #[async_trait]
    impl ModelProvider for StubProvider {
        fn provider_name(&self) -> &str {
            "stub"
        }
        fn model_id(&self) -> &str {
            &self.model_id
        }
        fn capabilities(&self) -> &ModelCapabilities {
            &self.caps
        }
        async fn generate_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<ProviderResponse, RociError> {
            Ok(ProviderResponse {
                text: "stub".to_string(),
                usage: Usage::default(),
                tool_calls: vec![],
                finish_reason: None,
                thinking: vec![],
            })
        }
        async fn stream_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
            Err(RociError::UnsupportedOperation(
                "stub does not stream".into(),
            ))
        }
    }

    #[test]
    fn register_and_create() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(StubFactory));

        assert!(registry.has_provider("stub"));
        assert!(registry.has_provider("stub-alias"));
        assert!(!registry.has_provider("unknown"));

        let config = RociConfig::new().with_token_store(None);
        let provider = registry
            .create_provider("stub", "my-model", &config)
            .unwrap();
        assert_eq!(provider.model_id(), "my-model");
        assert_eq!(provider.provider_name(), "stub");
    }

    #[test]
    fn create_unregistered_fails() {
        let registry = ProviderRegistry::new();
        let config = RociConfig::new().with_token_store(None);
        let result = registry.create_provider("nope", "m", &config);
        assert!(result.is_err());
        match result {
            Err(RociError::ModelNotFound(msg)) => assert!(msg.contains("nope")),
            Err(e) => panic!("expected ModelNotFound, got error: {e}"),
            Ok(_) => panic!("expected ModelNotFound, got Ok"),
        }
    }

    struct AnotherFactory;

    impl ProviderFactory for AnotherFactory {
        fn provider_keys(&self) -> &[&str] {
            &["another"]
        }

        fn create(
            &self,
            _config: &RociConfig,
            _provider_key: &str,
            model_id: &str,
        ) -> Result<Box<dyn ModelProvider>, RociError> {
            Ok(Box::new(StubProvider {
                model_id: model_id.to_string(),
                caps: ModelCapabilities {
                    supports_vision: false,
                    supports_tools: false,
                    supports_streaming: false,
                    supports_json_mode: false,
                    supports_json_schema: false,
                    supports_reasoning: false,
                    supports_system_messages: true,
                    context_length: 4096,
                    max_output_tokens: None,
                },
            }))
        }

        fn parse_model(
            &self,
            _provider_key: &str,
            _model_id: &str,
        ) -> Option<Box<dyn Any + Send + Sync>> {
            None
        }
    }

    #[test]
    fn multiple_factories_each_resolve_independently() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(StubFactory));
        registry.register(Arc::new(AnotherFactory));

        let config = RociConfig::new().with_token_store(None);

        let stub = registry.create_provider("stub", "s1", &config).unwrap();
        assert_eq!(stub.model_id(), "s1");

        let another = registry.create_provider("another", "a1", &config).unwrap();
        assert_eq!(another.model_id(), "a1");

        assert!(registry.has_provider("stub"));
        assert!(registry.has_provider("stub-alias"));
        assert!(registry.has_provider("another"));
        assert!(!registry.has_provider("nonexistent"));
    }

    #[test]
    fn provider_keys_lists_all_registered_keys() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(StubFactory));
        registry.register(Arc::new(AnotherFactory));

        let mut keys = registry.provider_keys();
        keys.sort();
        assert_eq!(keys, vec!["another", "stub", "stub-alias"]);
    }

    #[test]
    fn alias_keys_resolve_to_same_factory_output() {
        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(StubFactory));

        let config = RociConfig::new().with_token_store(None);

        let via_primary = registry
            .create_provider("stub", "m1", &config)
            .unwrap();
        let via_alias = registry
            .create_provider("stub-alias", "m1", &config)
            .unwrap();

        assert_eq!(via_primary.provider_name(), via_alias.provider_name());
        assert_eq!(via_primary.model_id(), via_alias.model_id());
    }

    struct CustomFactory {
        keys: Vec<&'static str>,
        provider_name: &'static str,
    }

    impl ProviderFactory for CustomFactory {
        fn provider_keys(&self) -> &[&str] {
            &self.keys
        }

        fn create(
            &self,
            _config: &RociConfig,
            _provider_key: &str,
            model_id: &str,
        ) -> Result<Box<dyn ModelProvider>, RociError> {
            Ok(Box::new(CustomProvider {
                name: self.provider_name.to_string(),
                model_id: model_id.to_string(),
                caps: ModelCapabilities {
                    supports_vision: false,
                    supports_tools: false,
                    supports_streaming: false,
                    supports_json_mode: false,
                    supports_json_schema: false,
                    supports_reasoning: false,
                    supports_system_messages: false,
                    context_length: 1024,
                    max_output_tokens: None,
                },
            }))
        }

        fn parse_model(
            &self,
            _provider_key: &str,
            _model_id: &str,
        ) -> Option<Box<dyn Any + Send + Sync>> {
            None
        }
    }

    struct CustomProvider {
        name: String,
        model_id: String,
        caps: ModelCapabilities,
    }

    #[async_trait]
    impl ModelProvider for CustomProvider {
        fn provider_name(&self) -> &str {
            &self.name
        }
        fn model_id(&self) -> &str {
            &self.model_id
        }
        fn capabilities(&self) -> &ModelCapabilities {
            &self.caps
        }
        async fn generate_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<ProviderResponse, RociError> {
            Ok(ProviderResponse {
                text: format!("custom-{}", self.name),
                usage: Usage::default(),
                tool_calls: vec![],
                finish_reason: None,
                thinking: vec![],
            })
        }
        async fn stream_text(
            &self,
            _request: &ProviderRequest,
        ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
            Err(RociError::UnsupportedOperation("no stream".into()))
        }
    }

    #[test]
    fn custom_provider_can_be_registered_with_arbitrary_selectors() {
        let factory = CustomFactory {
            keys: vec!["my-custom", "custom-alias"],
            provider_name: "my-custom-provider",
        };

        let mut registry = ProviderRegistry::new();
        registry.register(Arc::new(factory));

        assert!(registry.has_provider("my-custom"));
        assert!(registry.has_provider("custom-alias"));

        let config = RociConfig::new().with_token_store(None);
        let provider = registry
            .create_provider("my-custom", "custom-model-v1", &config)
            .unwrap();

        assert_eq!(provider.provider_name(), "my-custom-provider");
        assert_eq!(provider.model_id(), "custom-model-v1");
    }

    #[test]
    fn empty_registry_has_no_keys() {
        let registry = ProviderRegistry::new();
        assert!(registry.provider_keys().is_empty());
        assert!(!registry.has_provider("anything"));
    }
}
