use super::*;
use crate::agent_loop::runner::RetryBackoffPolicy;
use crate::models::ModelCapabilities;
use crate::provider::{ModelProvider, ProviderFactory, ProviderResponse};
use crate::tools::arguments::ToolArguments;
use crate::tools::dynamic::{DynamicTool, DynamicToolProvider};
use crate::tools::tool::ToolExecutionContext;
use crate::tools::{AgentTool, AgentToolParameters};
use crate::types::AgentToolCall;
use async_trait::async_trait;
use futures::stream::BoxStream;
use std::sync::{Arc, Mutex};

pub(super) fn test_registry() -> Arc<ProviderRegistry> {
    Arc::new(ProviderRegistry::new())
}

pub(super) fn test_config() -> RociConfig {
    RociConfig::new()
}

struct SummaryFactory {
    provider_key: &'static str,
    summary_text: String,
    created_models: Arc<Mutex<Vec<String>>>,
}

impl SummaryFactory {
    fn new(
        provider_key: &'static str,
        summary_text: impl Into<String>,
        created_models: Arc<Mutex<Vec<String>>>,
    ) -> Self {
        Self {
            provider_key,
            summary_text: summary_text.into(),
            created_models,
        }
    }
}

impl ProviderFactory for SummaryFactory {
    fn provider_keys(&self) -> &[&str] {
        std::slice::from_ref(&self.provider_key)
    }

    fn create(
        &self,
        _config: &RociConfig,
        _provider_key: &str,
        model_id: &str,
    ) -> Result<Box<dyn ModelProvider>, RociError> {
        self.created_models
            .lock()
            .expect("created_models lock")
            .push(model_id.to_string());
        Ok(Box::new(SummaryProvider {
            provider_key: self.provider_key.to_string(),
            model_id: model_id.to_string(),
            summary_text: self.summary_text.clone(),
            capabilities: ModelCapabilities::default(),
        }))
    }

    fn parse_model(
        &self,
        _provider_key: &str,
        _model_id: &str,
    ) -> Option<Box<dyn std::any::Any + Send + Sync>> {
        None
    }
}

struct SummaryProvider {
    provider_key: String,
    model_id: String,
    summary_text: String,
    capabilities: ModelCapabilities,
}

#[async_trait]
impl ModelProvider for SummaryProvider {
    fn provider_name(&self) -> &str {
        &self.provider_key
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Ok(ProviderResponse {
            text: self.summary_text.clone(),
            usage: crate::types::Usage::default(),
            tool_calls: Vec::new(),
            finish_reason: None,
            thinking: Vec::new(),
        })
    }

    async fn stream_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<crate::types::TextStreamDelta, RociError>>, RociError>
    {
        Err(RociError::UnsupportedOperation(
            "summary test provider does not stream".to_string(),
        ))
    }
}

pub(super) fn test_agent_config() -> AgentConfig {
    let model: LanguageModel = "openai:gpt-4o".parse().unwrap();
    AgentConfig {
        model,
        system_prompt: None,
        tools: Vec::new(),
        dynamic_tool_providers: Vec::new(),
        settings: GenerationSettings::default(),
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        event_sink: None,
        session_id: None,
        steering_mode: QueueDrainMode::All,
        follow_up_mode: QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        retry_backoff: RetryBackoffPolicy::default(),
        api_key_override: None,
        provider_headers: reqwest::header::HeaderMap::new(),
        provider_metadata: HashMap::new(),
        provider_payload_callback: None,
        get_api_key: None,
        compaction: CompactionSettings::default(),
        session_before_compact: None,
        session_before_tree: None,
        pre_tool_use: None,
        post_tool_use: None,
    }
}

pub(super) fn registry_with_summary_provider(
    provider_key: &'static str,
    summary_text: &str,
    created_models: Arc<Mutex<Vec<String>>>,
) -> Arc<ProviderRegistry> {
    let mut registry = ProviderRegistry::new();
    registry.register(Arc::new(SummaryFactory::new(
        provider_key,
        summary_text,
        created_models,
    )));
    Arc::new(registry)
}

pub(super) fn dummy_tool(name: &str) -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        name,
        "test tool",
        AgentToolParameters::empty(),
        |_args, _ctx| async move { Ok(serde_json::json!({ "ok": true })) },
    ))
}

pub(super) fn assistant_tool_call(
    id: &str,
    name: &str,
    arguments: serde_json::Value,
) -> ModelMessage {
    ModelMessage {
        role: Role::Assistant,
        content: vec![crate::types::ContentPart::ToolCall(AgentToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
            recipient: None,
        })],
        name: None,
        timestamp: None,
    }
}

pub(super) struct MockDynamicToolProvider {
    tools: Vec<DynamicTool>,
    calls: Arc<Mutex<Vec<String>>>,
}

impl MockDynamicToolProvider {
    pub(super) fn new(tools: Vec<DynamicTool>) -> Self {
        Self {
            tools,
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[async_trait]
impl DynamicToolProvider for MockDynamicToolProvider {
    async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
        Ok(self.tools.clone())
    }

    async fn execute_tool(
        &self,
        name: &str,
        _args: &ToolArguments,
        _ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        let mut calls = self
            .calls
            .lock()
            .expect("calls lock should not be poisoned");
        calls.push(name.to_string());
        Ok(serde_json::json!({ "ok": true }))
    }
}
