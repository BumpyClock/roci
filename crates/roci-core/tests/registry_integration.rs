//! Integration tests for ProviderRegistry and AuthService in roci-core.
//!
//! Uses mock implementations to verify the public API round-trip without
//! real network calls or API keys.

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::BoxStream;
use tempfile::TempDir;

use roci_core::auth::{
    AuthBackend, AuthError, AuthPollResult, AuthService, AuthStep, DeviceCodeSession, Token,
    TokenStore,
};
use roci_core::config::RociConfig;
use roci_core::error::RociError;
use roci_core::models::capabilities::ModelCapabilities;
use roci_core::provider::{
    ModelProvider, ProviderFactory, ProviderRegistry, ProviderRequest, ProviderResponse,
};
use roci_core::types::{TextStreamDelta, Usage};

// ---------------------------------------------------------------------------
// Mock ProviderFactory + ModelProvider
// ---------------------------------------------------------------------------

struct MockFactory {
    keys: &'static [&'static str],
    name: &'static str,
}

impl ProviderFactory for MockFactory {
    fn provider_keys(&self) -> &[&str] {
        self.keys
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        Ok(Box::new(MockProvider {
            name: self.name.to_string(),
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

struct MockProvider {
    name: String,
    model_id: String,
    caps: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for MockProvider {
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
            text: format!("response-from-{}", self.name),
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
            "mock does not stream".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Mock AuthBackend
// ---------------------------------------------------------------------------

struct MockAuthBackend {
    aliases: &'static [&'static str],
    display: &'static str,
    store_key: &'static str,
}

#[async_trait]
impl AuthBackend for MockAuthBackend {
    fn aliases(&self) -> &[&str] {
        self.aliases
    }

    fn display_name(&self) -> &str {
        self.display
    }

    fn store_key(&self) -> &str {
        self.store_key
    }

    async fn start_login(&self, _store: &Arc<dyn TokenStore>) -> Result<AuthStep, AuthError> {
        let token = Token {
            access_token: format!("mock-token-{}", self.store_key),
            refresh_token: None,
            id_token: None,
            expires_at: None,
            last_refresh: None,
            scopes: None,
            account_id: None,
        };
        Ok(AuthStep::Imported { token })
    }

    async fn poll_device_code(
        &self,
        _store: &Arc<dyn TokenStore>,
        _session: &DeviceCodeSession,
    ) -> Result<AuthPollResult, AuthError> {
        Err(AuthError::Unsupported("mock backend".into()))
    }

    async fn complete_pkce(
        &self,
        _store: &Arc<dyn TokenStore>,
        _code: &str,
        _state: &str,
    ) -> Result<Token, AuthError> {
        Err(AuthError::Unsupported("mock backend".into()))
    }

    fn get_status(&self, store: &Arc<dyn TokenStore>) -> Result<Option<Token>, AuthError> {
        store.load(self.store_key, "default")
    }

    fn logout(&self, store: &Arc<dyn TokenStore>) -> Result<(), AuthError> {
        store.clear(self.store_key, "default")
    }
}

// ---------------------------------------------------------------------------
// ProviderRegistry round-trip
// ---------------------------------------------------------------------------

#[test]
fn register_then_create_provider_round_trip() {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(MockFactory {
        keys: &["mock-a", "a-alias"],
        name: "mock-provider-a",
    }));
    registry.register(Arc::new(MockFactory {
        keys: &["mock-b"],
        name: "mock-provider-b",
    }));

    let config = RociConfig::new().with_token_store(None);

    let a = registry.create_provider("mock-a", "model-1", &config).unwrap();
    assert_eq!(a.provider_name(), "mock-provider-a");
    assert_eq!(a.model_id(), "model-1");

    let a_alias = registry.create_provider("a-alias", "model-2", &config).unwrap();
    assert_eq!(a_alias.provider_name(), "mock-provider-a");
    assert_eq!(a_alias.model_id(), "model-2");

    let b = registry.create_provider("mock-b", "model-3", &config).unwrap();
    assert_eq!(b.provider_name(), "mock-provider-b");

    let mut keys = registry.provider_keys();
    keys.sort();
    assert_eq!(keys, vec!["a-alias", "mock-a", "mock-b"]);
}

#[test]
fn unregistered_key_returns_model_not_found() {
    let registry = ProviderRegistry::new();
    let config = RociConfig::new().with_token_store(None);
    let result = registry.create_provider("missing", "m", &config);
    match result {
        Err(RociError::ModelNotFound(_)) => {}
        Err(e) => panic!("expected ModelNotFound, got: {e}"),
        Ok(_) => panic!("expected ModelNotFound, got Ok"),
    }
}

// ---------------------------------------------------------------------------
// AuthService round-trip with mock backend
// ---------------------------------------------------------------------------

fn temp_auth_service() -> (TempDir, AuthService) {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(roci_core::auth::FileTokenStore::new(
        roci_core::auth::TokenStoreConfig::new(dir.path().to_path_buf()),
    ));
    let svc = AuthService::new(store);
    (dir, svc)
}

#[tokio::test]
async fn auth_service_start_login_with_mock_backend() {
    let (_dir, mut svc) = temp_auth_service();
    svc.register_backend(Arc::new(MockAuthBackend {
        aliases: &["test-provider", "tp"],
        display: "Test Provider",
        store_key: "test-provider",
    }));

    let step = svc.start_login("test-provider").await.unwrap();
    match step {
        AuthStep::Imported { token } => {
            assert_eq!(token.access_token, "mock-token-test-provider");
        }
        other => panic!("expected Imported, got {other:?}"),
    }
}

#[tokio::test]
async fn auth_service_alias_resolves_to_backend() {
    let (_dir, mut svc) = temp_auth_service();
    svc.register_backend(Arc::new(MockAuthBackend {
        aliases: &["provider", "prov-alias"],
        display: "Provider",
        store_key: "provider",
    }));

    let step = svc.start_login("prov-alias").await.unwrap();
    assert!(matches!(step, AuthStep::Imported { .. }));
}

#[tokio::test]
async fn auth_service_rejects_unknown_provider() {
    let (_dir, svc) = temp_auth_service();
    let err = svc.start_login("unknown").await.unwrap_err();
    assert!(matches!(err, AuthError::Unsupported(_)));
}

#[test]
fn auth_service_all_statuses_lists_registered_backends() {
    let (_dir, mut svc) = temp_auth_service();
    svc.register_backend(Arc::new(MockAuthBackend {
        aliases: &["alpha"],
        display: "Alpha",
        store_key: "alpha",
    }));
    svc.register_backend(Arc::new(MockAuthBackend {
        aliases: &["beta"],
        display: "Beta",
        store_key: "beta",
    }));

    let statuses = svc.all_statuses();
    assert_eq!(statuses.len(), 2);

    let names: Vec<&str> = statuses.iter().map(|(name, _, _)| *name).collect();
    assert!(names.contains(&"Alpha"));
    assert!(names.contains(&"Beta"));
}
