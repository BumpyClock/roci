use super::*;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::time::{timeout, Duration};

use crate::agent::message::{convert_to_llm, AgentMessage};
use crate::agent_loop::events::ToolUpdatePayload;
use crate::agent_loop::RunStatus;
use crate::tools::arguments::ToolArguments;
use crate::tools::tool::{AgentTool, ToolExecutionContext, ToolUpdateCallback};
use crate::tools::types::AgentToolParameters;
use crate::types::{ContentPart, StreamEventType};

mod support;

use support::{capture_agent_events, capture_events, test_model, test_runner, ProviderScenario};

struct UpdateStreamingTool {
    params: AgentToolParameters,
    wait_for_cancel: bool,
}

impl UpdateStreamingTool {
    fn new(wait_for_cancel: bool) -> Self {
        Self {
            params: AgentToolParameters::empty(),
            wait_for_cancel,
        }
    }
}

#[async_trait]
impl Tool for UpdateStreamingTool {
    fn name(&self) -> &str {
        "update_tool"
    }

    fn description(&self) -> &str {
        "tool that emits partial updates"
    }

    fn parameters(&self) -> &AgentToolParameters {
        &self.params
    }

    async fn execute(
        &self,
        _args: &ToolArguments,
        _ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        Ok(serde_json::json!({ "tool": "update_tool", "status": "ok" }))
    }

    async fn execute_ext(
        &self,
        _args: &ToolArguments,
        _ctx: &ToolExecutionContext,
        cancel: CancellationToken,
        on_update: Option<ToolUpdateCallback>,
    ) -> Result<serde_json::Value, RociError> {
        if let Some(callback) = on_update.as_ref() {
            callback(ToolUpdatePayload {
                content: vec![ContentPart::Text {
                    text: "partial-1".to_string(),
                }],
                details: serde_json::json!({ "step": 1 }),
            });
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        if let Some(callback) = on_update.as_ref() {
            callback(ToolUpdatePayload {
                content: vec![ContentPart::Text {
                    text: "partial-2".to_string(),
                }],
                details: serde_json::json!({ "step": 2 }),
            });
        }

        if self.wait_for_cancel {
            tokio::select! {
                _ = cancel.cancelled() => Err(RociError::ToolExecution {
                    tool_name: "update_tool".to_string(),
                    message: "canceled".to_string(),
                }),
                _ = tokio::time::sleep(Duration::from_secs(5)) => Ok(serde_json::json!({
                    "tool": "update_tool",
                    "status": "late_ok",
                })),
            }
        } else {
            Ok(serde_json::json!({
                "tool": "update_tool",
                "status": "ok",
            }))
        }
    }
}

fn update_streaming_tool(wait_for_cancel: bool) -> Arc<dyn Tool> {
    Arc::new(UpdateStreamingTool::new(wait_for_cancel))
}

fn failing_tool() -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "failing_tool",
        "always fails",
        AgentToolParameters::empty(),
        |_args, _ctx: ToolExecutionContext| async move {
            Err(RociError::ToolExecution {
                tool_name: "failing_tool".to_string(),
                message: "forced failure".to_string(),
            })
        },
    ))
}

fn tracked_success_tool(
    name: &str,
    delay: Duration,
    active_calls: Arc<AtomicUsize>,
    max_active_calls: Arc<AtomicUsize>,
) -> Arc<dyn Tool> {
    let tool_name = name.to_string();
    Arc::new(AgentTool::new(
        tool_name.clone(),
        format!("{tool_name} tool"),
        AgentToolParameters::empty(),
        move |_args, _ctx: ToolExecutionContext| {
            let tool_name = tool_name.clone();
            let active_calls = active_calls.clone();
            let max_active_calls = max_active_calls.clone();
            async move {
                let active_now = active_calls.fetch_add(1, Ordering::SeqCst) + 1;
                let mut observed_max = max_active_calls.load(Ordering::SeqCst);
                while active_now > observed_max {
                    match max_active_calls.compare_exchange(
                        observed_max,
                        active_now,
                        Ordering::SeqCst,
                        Ordering::SeqCst,
                    ) {
                        Ok(_) => break,
                        Err(next) => observed_max = next,
                    }
                }
                tokio::time::sleep(delay).await;
                active_calls.fetch_sub(1, Ordering::SeqCst);
                Ok(serde_json::json!({ "tool": tool_name }))
            }
        },
    ))
}

fn tool_result_ids_from_messages(messages: &[ModelMessage]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|message| {
            message.content.iter().find_map(|part| match part {
                ContentPart::ToolResult(result) => Some(result.tool_call_id.clone()),
                _ => None,
            })
        })
        .collect()
}

fn assistant_tool_call_message_count(messages: &[ModelMessage]) -> usize {
    messages
        .iter()
        .filter(|message| {
            matches!(message.role, crate::types::Role::Assistant)
                && message
                    .content
                    .iter()
                    .any(|part| matches!(part, ContentPart::ToolCall(_)))
        })
        .count()
}

fn assistant_tool_calls(messages: &[ModelMessage]) -> Vec<AgentToolCall> {
    messages
        .iter()
        .filter(|message| matches!(message.role, crate::types::Role::Assistant))
        .flat_map(|message| {
            message.content.iter().filter_map(|part| match part {
                ContentPart::ToolCall(call) => Some(call.clone()),
                _ => None,
            })
        })
        .collect()
}

fn assistant_text_content(messages: &[ModelMessage]) -> String {
    messages
        .iter()
        .filter(|message| matches!(message.role, crate::types::Role::Assistant))
        .flat_map(|message| {
            message.content.iter().filter_map(|part| match part {
                ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn tool_result_ids_from_events(events: &[RunEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::ToolResult { result } => Some(result.tool_call_id.clone()),
            _ => None,
        })
        .collect()
}

fn tool_call_completed_ids_from_events(events: &[RunEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::ToolCallCompleted { call } => Some(call.id.clone()),
            _ => None,
        })
        .collect()
}

fn tool_result_id_from_message(message: &ModelMessage) -> Option<&str> {
    message.content.iter().find_map(|part| match part {
        ContentPart::ToolResult(result) => Some(result.tool_call_id.as_str()),
        _ => None,
    })
}

#[tokio::test]
async fn no_panic_when_stream_optional_fields_missing() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let (sink, _events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert!(
        !result.messages.is_empty(),
        "completed runs should carry final conversation messages"
    );
    assert!(
        result
            .messages
            .iter()
            .any(|message| matches!(message.role, crate::types::Role::User)),
        "result should include persisted prompt context"
    );
}

#[tokio::test]
async fn agent_message_lifecycle_events_emit_for_text_stream() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.agent_event_sink = Some(agent_sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let events = agent_events.lock().expect("agent event lock");
    let start_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageStart { message }
                    if message.role == crate::types::Role::Assistant
            )
        })
        .expect("expected assistant MessageStart");
    let update_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageUpdate {
                    message,
                    assistant_message_event,
                    ..
                } if message.role == crate::types::Role::Assistant
                    && assistant_message_event.event_type == StreamEventType::TextDelta
                    && assistant_message_event.text == "done"
            )
        })
        .expect("expected MessageUpdate(done)");
    let end_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageEnd { message }
                    if message.role == crate::types::Role::Assistant
            )
        })
        .expect("expected assistant MessageEnd");
    assert!(start_idx < update_idx);
    assert!(update_idx < end_idx);
}

#[tokio::test]
async fn message_lifecycle_events_cover_prompt_and_tool_results() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolUpdateThenComplete);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run update tool")]);
    request.tools = vec![update_streaming_tool(false)];
    request.approval_policy = ApprovalPolicy::Always;
    request.agent_event_sink = Some(agent_sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let events = agent_events.lock().expect("agent event lock");
    let user_start_count = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::MessageStart { message }
                    if message.role == crate::types::Role::User
            )
        })
        .count();
    let user_end_count = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::MessageEnd { message }
                    if message.role == crate::types::Role::User
            )
        })
        .count();
    assert_eq!(user_start_count, 1);
    assert_eq!(user_end_count, 1);

    let tool_start = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageStart { message }
                if message.role == crate::types::Role::Tool
                    && tool_result_id_from_message(message) == Some("update-tool-1")
        )
    });
    let tool_end = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageEnd { message }
                if message.role == crate::types::Role::Tool
                    && tool_result_id_from_message(message) == Some("update-tool-1")
        )
    });
    assert!(
        tool_start,
        "expected tool result MessageStart for update-tool-1"
    );
    assert!(
        tool_end,
        "expected tool result MessageEnd for update-tool-1"
    );
}

#[tokio::test]
async fn agent_message_end_is_emitted_before_failure_terminal_event() {
    let (runner, _requests) = test_runner(ProviderScenario::TextThenStreamError);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.agent_event_sink = Some(agent_sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Failed);
    assert!(result
        .error
        .as_deref()
        .unwrap_or_default()
        .contains("upstream stream failure"));

    let events = agent_events.lock().expect("agent event lock");
    let start_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageStart { message }
                    if message.role == crate::types::Role::Assistant
            )
        })
        .expect("expected assistant MessageStart");
    let update_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageUpdate {
                    message,
                    assistant_message_event,
                    ..
                } if message.role == crate::types::Role::Assistant
                    && assistant_message_event.event_type == StreamEventType::TextDelta
                    && assistant_message_event.text == "partial"
            )
        })
        .expect("expected MessageUpdate(partial)");
    let message_end_idx = events
        .iter()
        .position(|event| {
            matches!(
                event,
                AgentEvent::MessageEnd { message }
                    if message.role == crate::types::Role::Assistant
            )
        })
        .expect("expected assistant MessageEnd");
    let agent_end_idx = events
        .iter()
        .position(|event| matches!(event, AgentEvent::AgentEnd { .. }))
        .expect("expected AgentEnd");
    assert!(start_idx < update_idx);
    assert!(update_idx < message_end_idx);
    assert!(message_end_idx < agent_end_idx);
}

#[tokio::test]
async fn tool_execution_updates_stream_with_deterministic_order() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolUpdateThenComplete);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run update tool")]);
    request.tools = vec![update_streaming_tool(false)];
    request.approval_policy = ApprovalPolicy::Always;
    request.agent_event_sink = Some(agent_sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(
        tool_result_ids_from_messages(&result.messages),
        vec!["update-tool-1".to_string()]
    );

    let events = agent_events.lock().expect("agent event lock");
    let mut sequence: Vec<String> = Vec::new();
    for event in events.iter() {
        match event {
            AgentEvent::ToolExecutionStart {
                tool_call_id,
                tool_name,
                ..
            } if tool_call_id == "update-tool-1" && tool_name == "update_tool" => {
                sequence.push("start".to_string());
            }
            AgentEvent::ToolExecutionUpdate {
                tool_call_id,
                partial_result,
                ..
            } if tool_call_id == "update-tool-1" => {
                let step = partial_result
                    .details
                    .get("step")
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or_default();
                sequence.push(format!("update-{step}"));
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                is_error,
                ..
            } if tool_call_id == "update-tool-1" => {
                assert!(!is_error);
                sequence.push("end".to_string());
            }
            _ => {}
        }
    }
    assert_eq!(
        sequence,
        vec![
            "start".to_string(),
            "update-1".to_string(),
            "update-2".to_string(),
            "end".to_string(),
        ]
    );
}

#[tokio::test]
async fn canceling_during_tool_execution_emits_error_end_event() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolUpdateThenComplete);
    let (agent_sink, agent_events) = capture_agent_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("cancel update tool")]);
    request.tools = vec![update_streaming_tool(true)];
    request.approval_policy = ApprovalPolicy::Always;
    request.agent_event_sink = Some(agent_sink);

    let mut handle = runner.start(request).await.expect("start run");
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(handle.abort(), "abort should be accepted");

    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Canceled);

    let events = agent_events.lock().expect("agent event lock");
    let end_event = events.iter().find_map(|event| match event {
        AgentEvent::ToolExecutionEnd {
            tool_call_id,
            is_error,
            ..
        } if tool_call_id == "update-tool-1" => Some(*is_error),
        _ => None,
    });
    assert_eq!(end_event, Some(true));
}

#[tokio::test]
async fn steering_skip_emits_tool_and_message_lifecycle_for_skipped_calls() {
    let (runner, _requests) = test_runner(ProviderScenario::MutatingBatchThenComplete);
    let (agent_sink, agent_events) = capture_agent_events();
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run tools")]);
    request.tools = vec![
        tracked_success_tool(
            "apply_patch",
            Duration::from_millis(40),
            active_calls.clone(),
            max_active_calls.clone(),
        ),
        tracked_success_tool(
            "read",
            Duration::from_millis(40),
            active_calls,
            max_active_calls,
        ),
    ];
    request.approval_policy = ApprovalPolicy::Always;
    request.agent_event_sink = Some(agent_sink);

    let steering_tick = Arc::new(AtomicUsize::new(0));
    let steering_tick_clone = steering_tick.clone();
    request.get_steering_messages = Some(Arc::new(move || {
        let tick = steering_tick_clone.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            if tick >= 2 {
                vec![ModelMessage::user("interrupt")]
            } else {
                Vec::new()
            }
        })
    }));

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(4), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert!(
        tool_result_ids_from_messages(&result.messages)
            .iter()
            .any(|id| id == "safe-read-2"),
        "expected skipped tool result for safe-read-2"
    );

    let events = agent_events.lock().expect("agent event lock");
    let skipped_tool_start = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionStart { tool_call_id, .. } if tool_call_id == "safe-read-2"
        )
    });
    let skipped_tool_end = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::ToolExecutionEnd { tool_call_id, .. } if tool_call_id == "safe-read-2"
        )
    });
    let skipped_msg_start = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageStart { message }
                if message.role == crate::types::Role::Tool
                    && tool_result_id_from_message(message) == Some("safe-read-2")
        )
    });
    let skipped_msg_end = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::MessageEnd { message }
                if message.role == crate::types::Role::Tool
                    && tool_result_id_from_message(message) == Some("safe-read-2")
        )
    });

    assert!(
        skipped_tool_start,
        "expected ToolExecutionStart for skipped call"
    );
    assert!(
        skipped_tool_end,
        "expected ToolExecutionEnd for skipped call"
    );
    assert!(
        skipped_msg_start,
        "expected MessageStart for skipped tool result"
    );
    assert!(
        skipped_msg_end,
        "expected MessageEnd for skipped tool result"
    );
}

#[tokio::test]
async fn tool_failures_are_bounded_with_deterministic_reason() {
    let (runner, _requests) = test_runner(ProviderScenario::RepeatedToolFailure);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run tool")]);
    request.tools = vec![failing_tool()];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request
        .metadata
        .insert("runner.max_iterations".to_string(), "20".to_string());
    request
        .metadata
        .insert("runner.max_tool_failures".to_string(), "2".to_string());

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    let expected_error = "tool call failure limit reached (max_failures=2, consecutive_failures=2)";
    assert_eq!(result.status, RunStatus::Failed);
    assert_eq!(result.error.as_deref(), Some(expected_error));
    assert!(
        !result.messages.is_empty(),
        "failed runs should still expose conversation state"
    );
    let result_tool_ids = tool_result_ids_from_messages(&result.messages);
    assert_eq!(result_tool_ids.len(), 2);
    assert!(result_tool_ids.iter().all(|id| id == "tool-call-1"));

    let events = events.lock().expect("event lock");
    let failure_events: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::Lifecycle {
                state: RunLifecycle::Failed { error },
            } => Some(error.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        failure_events.last().map(String::as_str),
        Some(expected_error)
    );

    let tool_results = events
        .iter()
        .filter(|event| matches!(event.payload, RunEventPayload::ToolResult { .. }))
        .count();
    assert_eq!(tool_results, 2);
}

#[tokio::test]
async fn request_transport_is_forwarded_to_provider_request() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.transport = Some(provider::TRANSPORT_PROXY.to_string());

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert!(
        !requests.is_empty(),
        "provider should receive at least one request"
    );
    assert_eq!(
        requests[0].transport.as_deref(),
        Some(provider::TRANSPORT_PROXY)
    );
}

#[tokio::test]
async fn unsupported_request_transport_is_rejected_before_provider_call() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.transport = Some("satellite".to_string());

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("unsupported provider transport 'satellite'"),
        "expected unsupported transport error, got: {:?}",
        result.error
    );

    let requests = requests.lock().expect("request lock");
    assert!(
        requests.is_empty(),
        "provider should not be called for unsupported transports"
    );
}

#[tokio::test]
async fn convert_to_llm_hook_can_append_and_filter_custom_messages() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.convert_to_llm = Some(Arc::new(|mut messages: Vec<AgentMessage>| {
        Box::pin(async move {
            messages.push(AgentMessage::custom(
                "artifact",
                serde_json::json!({ "hidden": true }),
            ));
            messages.push(AgentMessage::user("hook-added"));
            convert_to_llm(&messages)
        })
    }));

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert!(!requests.is_empty(), "provider should receive one request");
    let first = &requests[0].messages;
    assert!(
        first.iter().any(|m| m.text() == "hook-added"),
        "conversion hook should be able to append LLM-visible messages"
    );
    assert!(
        first.iter().all(|m| matches!(
            m.role,
            crate::types::Role::System
                | crate::types::Role::User
                | crate::types::Role::Assistant
                | crate::types::Role::Tool
        )),
        "provider messages must remain LLM message roles after conversion"
    );
}

#[tokio::test]
async fn rate_limited_stream_retries_within_max_delay_cap() {
    let (runner, requests) = test_runner(ProviderScenario::RateLimitedThenComplete);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);
    request.max_retry_delay_ms = Some(10);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
}

#[tokio::test]
async fn rate_limited_stream_fails_when_retry_delay_exceeds_cap() {
    let (runner, requests) = test_runner(ProviderScenario::RateLimitedExceedsCap);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);
    request.max_retry_delay_ms = Some(10);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("exceeds max_retry_delay_ms"),
        "expected max retry delay failure, got: {:?}",
        result.error
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn rate_limited_without_retry_hint_fails_immediately() {
    let (runner, requests) = test_runner(ProviderScenario::RateLimitedWithoutRetryHint);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("retry")]);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("without retry_after hint"),
        "expected missing retry hint failure, got: {:?}",
        result.error
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 1);
}

#[tokio::test]
async fn parallel_safe_tools_execute_concurrently_and_append_results_in_call_order() {
    let (runner, requests) = test_runner(ProviderScenario::ParallelSafeBatchThenComplete);
    let (sink, events) = capture_events();
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("parallel tools")]);
    request.tools = vec![
        tracked_success_tool(
            "read",
            Duration::from_millis(150),
            active_calls.clone(),
            max_active_calls.clone(),
        ),
        tracked_success_tool(
            "ls",
            Duration::from_millis(150),
            active_calls,
            max_active_calls.clone(),
        ),
    ];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert!(max_active_calls.load(Ordering::SeqCst) >= 2);

    let requests = requests.lock().expect("request lock");
    assert!(requests.len() >= 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        assistant_tool_call_message_count(second_request_messages),
        1
    );
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["safe-read-1".to_string(), "safe-ls-2".to_string()]
    );

    let events = events.lock().expect("event lock");
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["safe-read-1".to_string(), "safe-ls-2".to_string()]
    );
    assert_eq!(
        tool_call_completed_ids_from_events(events.as_slice()),
        vec!["safe-read-1".to_string(), "safe-ls-2".to_string()]
    );
}

#[tokio::test]
async fn mutating_tools_remain_serialized_even_when_safe_tools_exist() {
    let (runner, requests) = test_runner(ProviderScenario::MutatingBatchThenComplete);
    let (sink, events) = capture_events();
    let active_calls = Arc::new(AtomicUsize::new(0));
    let max_active_calls = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("mutating tools")]);
    request.tools = vec![
        tracked_success_tool(
            "apply_patch",
            Duration::from_millis(150),
            active_calls.clone(),
            max_active_calls.clone(),
        ),
        tracked_success_tool(
            "read",
            Duration::from_millis(150),
            active_calls,
            max_active_calls.clone(),
        ),
    ];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(max_active_calls.load(Ordering::SeqCst), 1);

    let requests = requests.lock().expect("request lock");
    assert!(requests.len() >= 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        assistant_tool_call_message_count(second_request_messages),
        1
    );
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["mutating-call-1".to_string(), "safe-read-2".to_string()]
    );

    let events = events.lock().expect("event lock");
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["mutating-call-1".to_string(), "safe-read-2".to_string()]
    );
    assert_eq!(
        tool_call_completed_ids_from_events(events.as_slice()),
        vec!["mutating-call-1".to_string(), "safe-read-2".to_string()]
    );
}

#[tokio::test]
async fn mixed_text_and_parallel_tools_are_batched_before_single_followup() {
    let (runner, requests) = test_runner(ProviderScenario::MixedTextAndParallelBatchThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("mixed stream")]);
    request.tools = vec![
        tracked_success_tool(
            "read",
            Duration::from_millis(80),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
        tracked_success_tool(
            "ls",
            Duration::from_millis(80),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
    ];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        assistant_tool_call_message_count(second_request_messages),
        1
    );
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["mixed-read-1".to_string(), "mixed-ls-2".to_string()]
    );
    assert!(assistant_text_content(second_request_messages).contains("Gathering context."));

    let events = events.lock().expect("event lock");
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["mixed-read-1".to_string(), "mixed-ls-2".to_string()]
    );
}

#[tokio::test]
async fn duplicate_tool_call_deltas_are_deduplicated_by_call_id() {
    let (runner, requests) = test_runner(ProviderScenario::DuplicateToolCallDeltaThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("dup tool call")]);
    request.tools = vec![tracked_success_tool(
        "read",
        Duration::from_millis(50),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    )];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["dup-read-1".to_string()]
    );
    let calls = assistant_tool_calls(second_request_messages);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].id, "dup-read-1");
    assert_eq!(calls[0].arguments["path"], serde_json::json!("second"));

    let events = events.lock().expect("event lock");
    let tool_starts = events
        .iter()
        .filter(|event| matches!(event.payload, RunEventPayload::ToolCallStarted { .. }))
        .count();
    assert_eq!(tool_starts, 1);
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["dup-read-1".to_string()]
    );
}

#[tokio::test]
async fn stream_end_without_done_falls_back_to_tool_execution_and_completion() {
    let (runner, requests) = test_runner(ProviderScenario::StreamEndsWithoutDoneThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(
        test_model(),
        vec![ModelMessage::user("fallback completion")],
    );
    request.tools = vec![tracked_success_tool(
        "read",
        Duration::from_millis(50),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    )];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run wait timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    let second_request_messages = &requests[1].messages;
    assert_eq!(
        tool_result_ids_from_messages(second_request_messages),
        vec!["fallback-read-1".to_string()]
    );

    let events = events.lock().expect("event lock");
    assert_eq!(
        tool_result_ids_from_events(events.as_slice()),
        vec!["fallback-read-1".to_string()]
    );
    assert!(
        events.iter().all(|event| {
            !matches!(
                event.payload,
                RunEventPayload::Lifecycle {
                    state: RunLifecycle::Failed { .. },
                }
            )
        }),
        "stream-end fallback should not emit failed lifecycle"
    );
}

/// Tool with a required string `path` parameter for schema-validation integration tests.
fn schema_tool() -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "schema_tool",
        "tool with required path param",
        AgentToolParameters::object()
            .string("path", "file path", true)
            .build(),
        |_args, _ctx: ToolExecutionContext| async move { Ok(serde_json::json!({ "ok": true })) },
    ))
}

fn tracked_schema_path_tool(executions: Arc<AtomicUsize>) -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "schema_tool",
        "tool with required path param",
        AgentToolParameters::object()
            .string("path", "file path", true)
            .build(),
        move |args, _ctx: ToolExecutionContext| {
            let executions = executions.clone();
            async move {
                executions.fetch_add(1, Ordering::SeqCst);
                Ok(serde_json::json!({
                    "path": args.get_str("path")?,
                }))
            }
        },
    ))
}

/// Extract `(tool_call_id, result_json, is_error)` triples from ToolResult events.
fn tool_results_from_events(events: &[RunEvent]) -> Vec<(String, serde_json::Value, bool)> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::ToolResult { result } => Some((
                result.tool_call_id.clone(),
                result.result.clone(),
                result.is_error,
            )),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn tool_with_schema_rejects_bad_args_through_runner() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolBadArgs);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![schema_tool()];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");

    // The run must not panic and should complete (provider returns text-only on call 1).
    assert_eq!(result.status, RunStatus::Completed);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (call_id, result_json, is_error) = &tool_results[0];
    assert_eq!(call_id, "schema-call-1");
    assert!(is_error, "validation failure must set is_error: true");
    let error_msg = result_json["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("Argument validation failed"),
        "expected validation error prefix, got: {error_msg}"
    );
    assert!(
        error_msg.contains("missing required field 'path'"),
        "expected missing field detail, got: {error_msg}"
    );
}

#[tokio::test]
async fn tool_with_schema_accepts_valid_args_through_runner() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![schema_tool()];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Completed);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (call_id, result_json, is_error) = &tool_results[0];
    assert_eq!(call_id, "schema-call-1");
    assert!(!is_error, "valid args must not set is_error");
    assert_eq!(
        result_json["ok"], true,
        "tool handler should execute and return ok"
    );
}

#[tokio::test]
async fn tool_with_type_mismatch_rejects_through_runner() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolTypeMismatch);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![schema_tool()];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Completed);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (call_id, result_json, is_error) = &tool_results[0];
    assert_eq!(call_id, "schema-call-1");
    assert!(is_error, "type mismatch must set is_error: true");
    let error_msg = result_json["error"].as_str().unwrap_or("");
    assert!(
        error_msg.contains("Argument validation failed"),
        "expected validation error prefix, got: {error_msg}"
    );
    assert!(
        error_msg.contains("expected type 'string'"),
        "expected type mismatch detail, got: {error_msg}"
    );
}

#[tokio::test]
async fn pre_tool_use_hook_can_block_and_skip_tool_execution() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|_call, _cancel| {
            Box::pin(async {
                Ok(PreToolUseHookResult::Block {
                    reason: Some("blocked-by-test".to_string()),
                })
            })
        })),
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(*is_error, "blocked call must be an error");
    assert_eq!(result_json["source"], serde_json::json!("pre_tool_use"));
    assert!(
        result_json["error"]
            .as_str()
            .unwrap_or_default()
            .contains("blocked-by-test"),
        "blocked reason should be surfaced"
    );
}

#[tokio::test]
async fn pre_tool_use_hook_replace_args_are_used_by_tool() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|_call, _cancel| {
            Box::pin(async {
                Ok(PreToolUseHookResult::ReplaceArgs {
                    args: serde_json::json!({ "path": "/tmp/replaced-by-hook" }),
                })
            })
        })),
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(!is_error, "hook-rewritten args should still be valid");
    assert_eq!(
        result_json["path"],
        serde_json::json!("/tmp/replaced-by-hook")
    );
}

#[tokio::test]
async fn pre_tool_use_hook_error_returns_synthetic_tool_error() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: Some(Arc::new(|_call, _cancel| {
            Box::pin(async {
                Err(RociError::InvalidState(
                    "forced pre hook failure".to_string(),
                ))
            })
        })),
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 0);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(*is_error, "pre-hook errors must become tool errors");
    assert_eq!(result_json["source"], serde_json::json!("pre_tool_use"));
    assert!(result_json["error"]
        .as_str()
        .unwrap_or_default()
        .contains("forced pre hook failure"));
}

#[tokio::test]
async fn post_tool_use_hook_can_mutate_tool_result() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: None,
        post_tool_use: Some(Arc::new(|_call, mut result| {
            Box::pin(async move {
                if let Some(map) = result.result.as_object_mut() {
                    map.insert("post_mutated".to_string(), serde_json::json!(true));
                }
                Ok(result)
            })
        })),
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(!is_error);
    assert_eq!(result_json["post_mutated"], serde_json::json!(true));
}

#[tokio::test]
async fn post_tool_use_hook_runs_for_skipped_synthetic_errors() {
    let (runner, _requests) = test_runner(ProviderScenario::MutatingBatchThenComplete);
    let (sink, events) = capture_events();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("run tools")]);
    request.tools = vec![
        tracked_success_tool(
            "apply_patch",
            Duration::from_millis(40),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
        tracked_success_tool(
            "read",
            Duration::from_millis(40),
            Arc::new(AtomicUsize::new(0)),
            Arc::new(AtomicUsize::new(0)),
        ),
    ];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    let seen_calls = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_calls_for_hook = seen_calls.clone();
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: None,
        post_tool_use: Some(Arc::new(move |call, mut result| {
            let seen_calls_for_hook = seen_calls_for_hook.clone();
            Box::pin(async move {
                seen_calls_for_hook
                    .lock()
                    .expect("seen calls lock")
                    .push(call.id.clone());
                if let Some(map) = result.result.as_object_mut() {
                    map.insert("post_seen".to_string(), serde_json::json!(true));
                }
                Ok(result)
            })
        })),
    };
    let steering_tick = Arc::new(AtomicUsize::new(0));
    let steering_tick_clone = steering_tick.clone();
    request.get_steering_messages = Some(Arc::new(move || {
        let tick = steering_tick_clone.fetch_add(1, Ordering::SeqCst) + 1;
        Box::pin(async move {
            if tick >= 2 {
                vec![ModelMessage::user("interrupt")]
            } else {
                Vec::new()
            }
        })
    }));

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(4), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let seen_calls = seen_calls.lock().expect("seen calls lock");
    assert!(
        seen_calls.iter().any(|id| id == "safe-read-2"),
        "post hook should run for skipped synthetic result"
    );
    drop(seen_calls);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    let (_call_id, result_json, is_error) = tool_results
        .iter()
        .find(|(call_id, _, _)| call_id == "safe-read-2")
        .expect("expected skipped safe-read-2 result");
    assert!(*is_error);
    assert_eq!(result_json["post_seen"], serde_json::json!(true));
}

#[tokio::test]
async fn post_tool_use_hook_error_returns_deterministic_synthetic_error() {
    let (runner, _requests) = test_runner(ProviderScenario::SchemaToolValidArgs);
    let (sink, events) = capture_events();
    let executions = Arc::new(AtomicUsize::new(0));
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("call schema_tool")]);
    request.tools = vec![tracked_schema_path_tool(executions.clone())];
    request.approval_policy = ApprovalPolicy::Always;
    request.event_sink = Some(sink);
    request.hooks = RunHooks {
        compaction: None,
        pre_tool_use: None,
        post_tool_use: Some(Arc::new(|_call, _result| {
            Box::pin(async {
                Err(RociError::InvalidState(
                    "forced post hook failure".to_string(),
                ))
            })
        })),
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(3), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(executions.load(Ordering::SeqCst), 1);

    let events = events.lock().expect("event lock");
    let tool_results = tool_results_from_events(&events);
    assert_eq!(tool_results.len(), 1, "expected exactly one tool result");
    let (_call_id, result_json, is_error) = &tool_results[0];
    assert!(*is_error);
    assert_eq!(result_json["source"], serde_json::json!("post_tool_use"));
    assert_eq!(
        result_json["original_result"]["path"],
        serde_json::json!("/tmp/test")
    );
    assert!(result_json["error"]
        .as_str()
        .unwrap_or_default()
        .contains("forced post hook failure"));
}

#[tokio::test]
async fn auto_compaction_triggers_when_context_exceeds_reserved_window() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = calls.clone();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.hooks = RunHooks {
        compaction: Some(Arc::new(move |_messages, _cancel| {
            let calls_clone = calls_clone.clone();
            Box::pin(async move {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };
    request.auto_compaction = Some(AutoCompactionConfig {
        reserve_tokens: 4096,
    });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn auto_compaction_replaces_messages_before_provider_call() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(
        test_model(),
        vec![
            ModelMessage::system("system must stay"),
            ModelMessage::user("old context"),
            ModelMessage::user("new context"),
        ],
    );
    request.hooks = RunHooks {
        compaction: Some(Arc::new(move |messages, _cancel| {
            Box::pin(async move {
                Ok(Some(vec![
                    messages[0].clone(),
                    ModelMessage::user("<compaction_summary>\nsummary\n</compaction_summary>"),
                    messages
                        .last()
                        .cloned()
                        .expect("compaction input should have latest message"),
                ]))
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };
    request.auto_compaction = Some(AutoCompactionConfig {
        reserve_tokens: 4096,
    });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let recorded = requests.lock().expect("request lock");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].messages.len(), 3);
    assert_eq!(recorded[0].messages[0].role, crate::types::Role::System);
    assert_eq!(recorded[0].messages[0].text(), "system must stay");
    assert!(recorded[0].messages[1]
        .text()
        .contains("<compaction_summary>"));
    assert_eq!(recorded[0].messages[2].text(), "new context");
}

#[tokio::test]
async fn compaction_failure_fails_run_and_surfaces_error() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.hooks = RunHooks {
        compaction: Some(Arc::new(move |_messages, _cancel| {
            Box::pin(async {
                Err(RociError::InvalidState(
                    "forced compaction failure".to_string(),
                ))
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };
    request.auto_compaction = Some(AutoCompactionConfig {
        reserve_tokens: 4096,
    });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Failed);
    let error = result.error.unwrap_or_default();
    assert!(error.contains("compaction failed"));
    assert!(error.contains("forced compaction failure"));
}

#[tokio::test]
async fn abort_during_auto_compaction_cancels_compaction_token_and_run() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let (started_tx, started_rx) = oneshot::channel::<()>();
    let started_tx = Arc::new(std::sync::Mutex::new(Some(started_tx)));
    let compaction_cancel_observed = Arc::new(AtomicBool::new(false));
    let compaction_cancel_observed_for_hook = compaction_cancel_observed.clone();

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.hooks = RunHooks {
        compaction: Some(Arc::new(move |_messages, cancel| {
            let started_tx = started_tx.clone();
            let compaction_cancel_observed = compaction_cancel_observed_for_hook.clone();
            Box::pin(async move {
                if let Some(tx) = started_tx.lock().expect("start signal lock").take() {
                    let _ = tx.send(());
                }
                tokio::spawn(async move {
                    cancel.cancelled().await;
                    compaction_cancel_observed.store(true, Ordering::SeqCst);
                });
                std::future::pending::<Result<Option<Vec<ModelMessage>>, RociError>>().await
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };
    request.auto_compaction = Some(AutoCompactionConfig {
        reserve_tokens: 4096,
    });

    let mut handle = runner.start(request).await.expect("start run");
    timeout(Duration::from_secs(1), started_rx)
        .await
        .expect("compaction hook should start")
        .expect("compaction hook start signal should send");
    assert!(handle.abort(), "abort should be accepted");

    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Canceled);
    timeout(Duration::from_secs(1), async {
        while !compaction_cancel_observed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("compaction cancel token should be canceled");
}
