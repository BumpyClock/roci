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

mod auto_compaction;
mod request_pipeline;
mod schema_and_hooks;
mod stream_lifecycle;
mod tool_execution;
