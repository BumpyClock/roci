use std::sync::Arc;

use futures::future;
use tokio_util::sync::CancellationToken;

use crate::tools::{tool::Tool, ToolUpdateCallback};
use crate::types::{AgentToolCall, AgentToolResult, ModelMessage};

use super::super::events::{RunEventPayload, RunEventStream, ToolUpdatePayload};
use super::control::{AgentEventEmitter, RunEventEmitter};
use super::message_events::emit_message_lifecycle;
use super::{AgentEvent, PreToolUseHookResult, RunHooks};

pub(super) fn declined_tool_result(call: &AgentToolCall) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call.id.clone(),
        result: serde_json::json!({ "error": "approval declined" }),
        is_error: true,
    }
}

pub(super) fn canceled_tool_result(call: &AgentToolCall) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call.id.clone(),
        result: serde_json::json!({ "error": "canceled" }),
        is_error: true,
    }
}

#[derive(Debug, Clone)]
pub(super) struct ToolExecutionOutcome {
    pub(super) call: AgentToolCall,
    pub(super) result: AgentToolResult,
}

fn synthetic_hook_error_result(
    call: &AgentToolCall,
    source: &str,
    error: impl Into<String>,
) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call.id.clone(),
        result: serde_json::json!({
            "error": error.into(),
            "source": source,
        }),
        is_error: true,
    }
}

fn pre_tool_use_block_result(call: &AgentToolCall, reason: Option<String>) -> AgentToolResult {
    let error = reason.unwrap_or_else(|| "tool call blocked by pre_tool_use hook".to_string());
    synthetic_hook_error_result(call, "pre_tool_use", error)
}

async fn apply_pre_tool_use_hook(
    hooks: &RunHooks,
    call: &AgentToolCall,
    cancel: CancellationToken,
) -> Result<AgentToolCall, AgentToolResult> {
    let Some(hook) = hooks.pre_tool_use.as_ref() else {
        return Ok(call.clone());
    };
    match hook(call.clone(), cancel).await {
        Ok(PreToolUseHookResult::Continue) => Ok(call.clone()),
        Ok(PreToolUseHookResult::Block { reason }) => Err(pre_tool_use_block_result(call, reason)),
        Ok(PreToolUseHookResult::ReplaceArgs { args }) => {
            let mut replaced = call.clone();
            replaced.arguments = args;
            Ok(replaced)
        }
        Err(err) => Err(synthetic_hook_error_result(
            call,
            "pre_tool_use",
            format!("pre_tool_use hook failed: {err}"),
        )),
    }
}

pub(super) async fn apply_post_tool_use_hook(
    hooks: &RunHooks,
    call: &AgentToolCall,
    result: AgentToolResult,
) -> AgentToolResult {
    let Some(hook) = hooks.post_tool_use.as_ref() else {
        return result;
    };
    let original_result = result.clone();
    match hook(call.clone(), result).await {
        Ok(next) => next,
        Err(err) => AgentToolResult {
            tool_call_id: original_result.tool_call_id.clone(),
            result: serde_json::json!({
                "error": format!("post_tool_use hook failed: {err}"),
                "source": "post_tool_use",
                "original_result": original_result.result,
                "original_is_error": original_result.is_error,
            }),
            is_error: true,
        },
    }
}

pub(super) fn emit_tool_execution_start(agent_emitter: &AgentEventEmitter, call: &AgentToolCall) {
    agent_emitter.emit(AgentEvent::ToolExecutionStart {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        args: call.arguments.clone(),
    });
}

pub(super) fn emit_tool_execution_end(
    agent_emitter: &AgentEventEmitter,
    call: &AgentToolCall,
    result: &AgentToolResult,
) {
    agent_emitter.emit(AgentEvent::ToolExecutionEnd {
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        result: result.clone(),
        is_error: result.is_error,
    });
}

pub(super) async fn execute_tool_call(
    tools: &[Arc<dyn Tool>],
    hooks: &RunHooks,
    call: &AgentToolCall,
    agent_emitter: &AgentEventEmitter,
    cancel: CancellationToken,
) -> ToolExecutionOutcome {
    let call = match apply_pre_tool_use_hook(hooks, call, cancel.child_token()).await {
        Ok(call) => call,
        Err(result) => {
            return ToolExecutionOutcome {
                call: call.clone(),
                result,
            };
        }
    };
    let tool = tools.iter().find(|t| t.name() == call.name);
    match tool {
        Some(tool) => {
            let schema = &tool.parameters().schema;
            if let Err(validation_error) =
                crate::tools::validation::validate_arguments(&call.arguments, schema)
            {
                let result = AgentToolResult {
                    tool_call_id: call.id.clone(),
                    result: serde_json::json!({
                        "error": format!("Argument validation failed: {}", validation_error)
                    }),
                    is_error: true,
                };
                return ToolExecutionOutcome { call, result };
            }
            let args = crate::tools::arguments::ToolArguments::new(call.arguments.clone());
            let ctx = crate::tools::tool::ToolExecutionContext {
                metadata: serde_json::Value::Null,
                tool_call_id: Some(call.id.clone()),
                tool_name: Some(call.name.clone()),
            };
            let call_id = call.id.clone();
            let call_name = call.name.clone();
            let call_args = call.arguments.clone();
            let update_emitter = agent_emitter.clone();
            let on_update: ToolUpdateCallback =
                Arc::new(move |partial_result: ToolUpdatePayload| {
                    update_emitter.emit(AgentEvent::ToolExecutionUpdate {
                        tool_call_id: call_id.clone(),
                        tool_name: call_name.clone(),
                        args: call_args.clone(),
                        partial_result,
                    });
                });
            let result = match tool.execute_ext(&args, &ctx, cancel, Some(on_update)).await {
                Ok(val) => AgentToolResult {
                    tool_call_id: call.id.clone(),
                    result: val,
                    is_error: false,
                },
                Err(error) => AgentToolResult {
                    tool_call_id: call.id.clone(),
                    result: serde_json::json!({ "error": error.to_string() }),
                    is_error: true,
                },
            };
            ToolExecutionOutcome { call, result }
        }
        None => ToolExecutionOutcome {
            result: AgentToolResult {
                tool_call_id: call.id.clone(),
                result: serde_json::json!({ "error": format!("Tool '{}' not found", call.name) }),
                is_error: true,
            },
            call,
        },
    }
}

pub(super) async fn execute_parallel_tool_calls(
    tools: &[Arc<dyn Tool>],
    hooks: &RunHooks,
    calls: &[AgentToolCall],
    agent_emitter: &AgentEventEmitter,
    cancel: CancellationToken,
) -> Vec<ToolExecutionOutcome> {
    let futures = calls
        .iter()
        .map(|call| execute_tool_call(tools, hooks, call, agent_emitter, cancel.child_token()));
    future::join_all(futures).await
}

pub(super) async fn append_tool_result(
    hooks: &RunHooks,
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    call: &AgentToolCall,
    result: AgentToolResult,
    iteration_failures: &mut usize,
    messages: &mut Vec<ModelMessage>,
) -> AgentToolResult {
    let result = apply_post_tool_use_hook(hooks, call, result).await;

    if result.is_error {
        *iteration_failures = iteration_failures.saturating_add(1);
    }

    emitter.emit(
        RunEventStream::Tool,
        RunEventPayload::ToolResult {
            result: result.clone(),
        },
    );
    emitter.emit(
        RunEventStream::Tool,
        RunEventPayload::ToolCallCompleted { call: call.clone() },
    );

    let tool_result_message = ModelMessage::tool_result(
        result.tool_call_id.clone(),
        result.result.clone(),
        result.is_error,
    );
    emit_message_lifecycle(agent_emitter, &tool_result_message);
    messages.push(tool_result_message);
    result
}

pub(super) async fn append_skipped_tool_call(
    hooks: &RunHooks,
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    call: &AgentToolCall,
    iteration_failures: &mut usize,
    messages: &mut Vec<ModelMessage>,
) -> AgentToolResult {
    let skipped_result = AgentToolResult {
        tool_call_id: call.id.clone(),
        result: serde_json::json!({ "error": "Skipped due to steering message" }),
        is_error: true,
    };
    emit_tool_execution_start(agent_emitter, call);
    emit_tool_execution_end(agent_emitter, call, &skipped_result);
    append_tool_result(
        hooks,
        emitter,
        agent_emitter,
        call,
        skipped_result.clone(),
        iteration_failures,
        messages,
    )
    .await
}
