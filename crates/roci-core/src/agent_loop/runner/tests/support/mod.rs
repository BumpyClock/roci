use super::*;

use crate::models::ModelCapabilities;
use crate::provider::{ModelProvider, ProviderRequest, ProviderResponse};
use crate::types::TextStreamDelta;
use futures::stream::{self, BoxStream};
use std::sync::atomic::{AtomicUsize, Ordering};

mod scenario_events;

#[derive(Clone, Copy)]
pub(super) enum ProviderScenario {
    MissingOptionalFields,
    TextThenStreamError,
    RepeatedToolFailure,
    RateLimitedThenComplete,
    RateLimitedExceedsCap,
    RateLimitedWithoutRetryHint,
    ParallelSafeBatchThenComplete,
    MutatingBatchThenComplete,
    MixedTextAndParallelBatchThenComplete,
    DuplicateToolCallDeltaThenComplete,
    StreamEndsWithoutDoneThenComplete,
    ToolUpdateThenComplete,
    /// Tool call for "schema_tool" with empty args on call 0, then text "done" on call 1+.
    SchemaToolBadArgs,
    /// Tool call for "schema_tool" with valid args on call 0, then text "done" on call 1+.
    SchemaToolValidArgs,
    /// Tool call for "schema_tool" with type-mismatched args on call 0, then text "done" on call 1+.
    SchemaToolTypeMismatch,
}

struct StubProvider {
    scenario: ProviderScenario,
    calls: AtomicUsize,
    capabilities: ModelCapabilities,
    requests: Arc<std::sync::Mutex<Vec<ProviderRequest>>>,
}

impl StubProvider {
    fn new(
        scenario: ProviderScenario,
        requests: Arc<std::sync::Mutex<Vec<ProviderRequest>>>,
    ) -> Self {
        Self {
            scenario,
            calls: AtomicUsize::new(0),
            capabilities: ModelCapabilities::default(),
            requests,
        }
    }
}

#[async_trait]
impl ModelProvider for StubProvider {
    fn provider_name(&self) -> &str {
        "stub"
    }

    fn model_id(&self) -> &str {
        "stub-model"
    }

    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        _request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        Err(RociError::UnsupportedOperation(
            "stream-only stub provider".to_string(),
        ))
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        self.requests
            .lock()
            .expect("request lock")
            .push(request.clone());
        let call_index = self.calls.fetch_add(1, Ordering::SeqCst);
        let events = scenario_events::events_for_scenario(self.scenario, call_index)?;
        Ok(Box::pin(stream::iter(events)))
    }
}

pub(super) fn test_runner(
    scenario: ProviderScenario,
) -> (LoopRunner, Arc<std::sync::Mutex<Vec<ProviderRequest>>>) {
    let requests = Arc::new(std::sync::Mutex::new(Vec::<ProviderRequest>::new()));
    let provider_requests = requests.clone();
    let factory: ProviderFactory = Arc::new(move |_model, _config| {
        Ok(Box::new(StubProvider::new(
            scenario,
            provider_requests.clone(),
        )))
    });
    (
        LoopRunner::with_provider_factory(RociConfig::new(), factory),
        requests,
    )
}

pub(super) fn test_model() -> LanguageModel {
    LanguageModel::Custom {
        provider: "stub".to_string(),
        model_id: "stub-model".to_string(),
    }
}

pub(super) fn capture_events() -> (RunEventSink, Arc<std::sync::Mutex<Vec<RunEvent>>>) {
    let events = Arc::new(std::sync::Mutex::new(Vec::<RunEvent>::new()));
    let sink_events = events.clone();
    let sink: RunEventSink = Arc::new(move |event| {
        if let Ok(mut guard) = sink_events.lock() {
            guard.push(event);
        }
    });
    (sink, events)
}

pub(super) fn capture_agent_events() -> (AgentEventSink, Arc<std::sync::Mutex<Vec<AgentEvent>>>) {
    let events = Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
    let sink_events = events.clone();
    let sink: AgentEventSink = Arc::new(move |event| {
        if let Ok(mut guard) = sink_events.lock() {
            guard.push(event);
        }
    });
    (sink, events)
}
