use super::*;

use crate::models::ModelCapabilities;
use crate::provider::{ModelProvider, ProviderResponse};
use crate::types::{StreamEventType, TextStreamDelta, Usage};
use futures::stream::{self, BoxStream};
use std::sync::atomic::{AtomicUsize, Ordering};

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
        let events = match self.scenario {
            ProviderScenario::MissingOptionalFields => vec![
                Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::Reasoning,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                }),
                Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::ToolCallDelta,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                }),
                Ok(TextStreamDelta {
                    text: "done".to_string(),
                    event_type: StreamEventType::TextDelta,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                }),
                Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::Done,
                    tool_call: None,
                    finish_reason: None,
                    usage: Some(Usage::default()),
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                }),
            ],
            ProviderScenario::TextThenStreamError => vec![
                Ok(TextStreamDelta {
                    text: "partial".to_string(),
                    event_type: StreamEventType::TextDelta,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                }),
                Ok(TextStreamDelta {
                    text: "upstream stream failure".to_string(),
                    event_type: StreamEventType::Error,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                }),
            ],
            ProviderScenario::RepeatedToolFailure => vec![
                Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::ToolCallDelta,
                    tool_call: Some(AgentToolCall {
                        id: "tool-call-1".to_string(),
                        name: "failing_tool".to_string(),
                        arguments: serde_json::json!({}),
                        recipient: None,
                    }),
                    finish_reason: None,
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                }),
                Ok(TextStreamDelta {
                    text: String::new(),
                    event_type: StreamEventType::Done,
                    tool_call: None,
                    finish_reason: None,
                    usage: Some(Usage::default()),
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                }),
            ],
            ProviderScenario::RateLimitedThenComplete => {
                if call_index == 0 {
                    return Err(RociError::RateLimited {
                        retry_after_ms: Some(1),
                    });
                }
                vec![Ok(TextStreamDelta {
                    text: "done".to_string(),
                    event_type: StreamEventType::TextDelta,
                    tool_call: None,
                    finish_reason: None,
                    usage: None,
                    reasoning: None,
                    reasoning_signature: None,
                    reasoning_type: None,
                })]
            }
            ProviderScenario::RateLimitedExceedsCap => {
                return Err(RociError::RateLimited {
                    retry_after_ms: Some(50),
                });
            }
            ProviderScenario::RateLimitedWithoutRetryHint => {
                return Err(RociError::RateLimited {
                    retry_after_ms: None,
                });
            }
            ProviderScenario::ParallelSafeBatchThenComplete => {
                if call_index == 0 {
                    vec![
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "safe-read-1".to_string(),
                                name: "read".to_string(),
                                arguments: serde_json::json!({}),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "safe-ls-2".to_string(),
                                name: "ls".to_string(),
                                arguments: serde_json::json!({}),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                    ]
                } else {
                    vec![Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    })]
                }
            }
            ProviderScenario::MutatingBatchThenComplete => {
                if call_index == 0 {
                    vec![
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "mutating-call-1".to_string(),
                                name: "apply_patch".to_string(),
                                arguments: serde_json::json!({}),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "safe-read-2".to_string(),
                                name: "read".to_string(),
                                arguments: serde_json::json!({}),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                    ]
                } else {
                    vec![Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    })]
                }
            }
            ProviderScenario::MixedTextAndParallelBatchThenComplete => {
                if call_index == 0 {
                    vec![
                        Ok(TextStreamDelta {
                            text: "Gathering context.".to_string(),
                            event_type: StreamEventType::TextDelta,
                            tool_call: None,
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "mixed-read-1".to_string(),
                                name: "read".to_string(),
                                arguments: serde_json::json!({ "path": "README.md" }),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "mixed-ls-2".to_string(),
                                name: "ls".to_string(),
                                arguments: serde_json::json!({ "path": "." }),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                    ]
                } else {
                    vec![
                        Ok(TextStreamDelta {
                            text: "complete".to_string(),
                            event_type: StreamEventType::TextDelta,
                            tool_call: None,
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                    ]
                }
            }
            ProviderScenario::DuplicateToolCallDeltaThenComplete => {
                if call_index == 0 {
                    vec![
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "dup-read-1".to_string(),
                                name: "read".to_string(),
                                arguments: serde_json::json!({ "path": "first" }),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "dup-read-1".to_string(),
                                name: "read".to_string(),
                                arguments: serde_json::json!({ "path": "second" }),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                    ]
                } else {
                    vec![Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    })]
                }
            }
            ProviderScenario::StreamEndsWithoutDoneThenComplete => {
                if call_index == 0 {
                    vec![Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::ToolCallDelta,
                        tool_call: Some(AgentToolCall {
                            id: "fallback-read-1".to_string(),
                            name: "read".to_string(),
                            arguments: serde_json::json!({ "path": "fallback" }),
                            recipient: None,
                        }),
                        finish_reason: None,
                        usage: None,
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    })]
                } else {
                    vec![Ok(TextStreamDelta {
                        text: String::new(),
                        event_type: StreamEventType::Done,
                        tool_call: None,
                        finish_reason: None,
                        usage: Some(Usage::default()),
                        reasoning: None,
                        reasoning_signature: None,
                        reasoning_type: None,
                    })]
                }
            }
            ProviderScenario::ToolUpdateThenComplete => {
                if call_index == 0 {
                    vec![
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "update-tool-1".to_string(),
                                name: "update_tool".to_string(),
                                arguments: serde_json::json!({ "path": "README.md" }),
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                    ]
                } else {
                    vec![
                        Ok(TextStreamDelta {
                            text: "done".to_string(),
                            event_type: StreamEventType::TextDelta,
                            tool_call: None,
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                    ]
                }
            }
            ProviderScenario::SchemaToolBadArgs
            | ProviderScenario::SchemaToolValidArgs
            | ProviderScenario::SchemaToolTypeMismatch => {
                let args = match self.scenario {
                    ProviderScenario::SchemaToolBadArgs => serde_json::json!({}),
                    ProviderScenario::SchemaToolValidArgs => {
                        serde_json::json!({ "path": "/tmp/test" })
                    }
                    ProviderScenario::SchemaToolTypeMismatch => {
                        serde_json::json!({ "path": 42 })
                    }
                    _ => unreachable!(),
                };
                if call_index == 0 {
                    vec![
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::ToolCallDelta,
                            tool_call: Some(AgentToolCall {
                                id: "schema-call-1".to_string(),
                                name: "schema_tool".to_string(),
                                arguments: args,
                                recipient: None,
                            }),
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                    ]
                } else {
                    vec![
                        Ok(TextStreamDelta {
                            text: "done".to_string(),
                            event_type: StreamEventType::TextDelta,
                            tool_call: None,
                            finish_reason: None,
                            usage: None,
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                        Ok(TextStreamDelta {
                            text: String::new(),
                            event_type: StreamEventType::Done,
                            tool_call: None,
                            finish_reason: None,
                            usage: Some(Usage::default()),
                            reasoning: None,
                            reasoning_signature: None,
                            reasoning_type: None,
                        }),
                    ]
                }
            }
        };
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
