use std::sync::Arc;

use futures::future;
use tokio_util::sync::CancellationToken;

use crate::session::{LogicalPath, SessionFs};
use crate::tools::SandboxProvider;
use crate::tools::{tool::Tool, ToolArguments, ToolSafetyPlan, ToolUpdateCallback};
use crate::types::{AgentToolCall, AgentToolResult, ModelMessage};

use super::super::events::{RunEventPayload, RunEventStream, ToolUpdatePayload};
use super::control::{AgentEventEmitter, RunEventEmitter};
use super::message_events::emit_message_lifecycle;
use super::{AgentEvent, PreToolUseHookResult, RunHooks};

const TOOL_RESULT_SIZE_LIMIT_REASON: &str = "tool_result_size_limit_exceeded";
const TOOL_RESULT_PREVIEW_MARKER: &str = "...<truncated>...";

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

#[derive(Clone)]
pub(super) struct ToolExecutionOutcome {
    pub(super) call: AgentToolCall,
    pub(super) tool: Option<Arc<dyn Tool>>,
    pub(super) result: AgentToolResult,
}

#[derive(Clone)]
pub(super) struct ResolvedToolCall {
    pub(super) call: AgentToolCall,
    pub(super) tool: Option<Arc<dyn Tool>>,
    pub(super) safety_plan: ToolSafetyPlan,
}

#[derive(Clone)]
pub(super) struct ToolExecutionInputs<'a> {
    session_fs: Option<Arc<dyn SessionFs + Send + Sync>>,
    session_cwd: Option<LogicalPath>,
    sandbox_provider: Option<Arc<dyn SandboxProvider>>,
    #[cfg(feature = "agent")]
    user_input_callback: Option<&'a crate::tools::user_input::RequestUserInputFn>,
}

impl<'a> ToolExecutionInputs<'a> {
    pub(super) fn new(
        session_fs: Option<Arc<dyn SessionFs + Send + Sync>>,
        session_cwd: Option<LogicalPath>,
        sandbox_provider: Option<Arc<dyn SandboxProvider>>,
        #[cfg(feature = "agent")] user_input_callback: Option<
            &'a crate::tools::user_input::RequestUserInputFn,
        >,
    ) -> Self {
        Self {
            session_fs,
            session_cwd,
            sandbox_provider,
            #[cfg(feature = "agent")]
            user_input_callback,
        }
    }
}

pub(super) fn resolve_tool_call(tools: &[Arc<dyn Tool>], call: &AgentToolCall) -> ResolvedToolCall {
    if let Some(tool) = tools.iter().find(|tool| tool.name() == call.name).cloned() {
        return ResolvedToolCall {
            call: call.clone(),
            tool: Some(tool),
            safety_plan: ToolSafetyPlan::default(),
        };
    }

    let mut normalized = call.clone();
    if let Some(tool) = normalize_tool_call_alias(tools, &mut normalized) {
        return ResolvedToolCall {
            call: normalized,
            tool: Some(Arc::clone(tool)),
            safety_plan: ToolSafetyPlan::default(),
        };
    }

    ResolvedToolCall {
        call: call.clone(),
        tool: None,
        safety_plan: ToolSafetyPlan::default(),
    }
}

pub(super) fn normalize_tool_call_alias<'a>(
    tools: &'a [Arc<dyn Tool>],
    call: &mut AgentToolCall,
) -> Option<&'a Arc<dyn Tool>> {
    let tool = tools.iter().find(|tool| {
        call.name != tool.name() && tool.aliases().iter().any(|alias| alias == &call.name)
    })?;
    let called_as = call.name.clone();
    call.called_as.get_or_insert(called_as);
    call.name = tool.name().to_string();
    Some(tool)
}

pub(super) fn validation_error_result(
    call: &AgentToolCall,
    validation_error: impl std::fmt::Display,
) -> AgentToolResult {
    AgentToolResult {
        tool_call_id: call.id.clone(),
        result: serde_json::json!({
            "error": format!("Argument validation failed: {validation_error}")
        }),
        is_error: true,
    }
}

pub(super) fn validate_finalized_tool_call(
    call: &AgentToolCall,
    tool: Option<&dyn Tool>,
) -> Result<(), AgentToolResult> {
    let Some(tool) = tool else {
        return Ok(());
    };
    crate::tools::validation::validate_arguments(&call.arguments, &tool.parameters().schema)
        .map_err(|validation_error| validation_error_result(call, validation_error))
}

pub(super) fn safety_plan_for_finalized_call(
    call: &AgentToolCall,
    tool: Option<&dyn Tool>,
) -> ToolSafetyPlan {
    let Some(tool) = tool else {
        return ToolSafetyPlan::default();
    };
    let safety_args = ToolArguments::new(call.arguments.clone());
    tool.safety(&safety_args)
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

pub(super) async fn apply_pre_tool_use_hook(
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

pub(super) fn apply_result_size_policy(
    _call: &AgentToolCall,
    tool: Option<&dyn Tool>,
    result: AgentToolResult,
) -> AgentToolResult {
    let Some(max) = tool.and_then(|tool| tool.result_policy().max_result_size_bytes) else {
        return result;
    };
    let Ok(serialized) = serde_json::to_string(&result.result) else {
        return result;
    };
    if serialized.len() <= max {
        return result;
    }

    AgentToolResult {
        tool_call_id: result.tool_call_id,
        result: tool_result_truncation_envelope(&serialized, max),
        is_error: result.is_error,
    }
}

pub(super) async fn finalize_tool_result(
    hooks: &RunHooks,
    call: &AgentToolCall,
    tool: Option<&dyn Tool>,
    result: AgentToolResult,
) -> AgentToolResult {
    let result = apply_post_tool_use_hook(hooks, call, result).await;
    apply_result_size_policy(call, tool, result)
}

fn tool_result_truncation_envelope(serialized: &str, max: usize) -> serde_json::Value {
    let original_size_bytes = serialized.len();
    let minimal = tool_result_truncation_envelope_with_preview(original_size_bytes, max, "");
    let Ok(minimal_serialized) = serde_json::to_string(&minimal) else {
        return minimal;
    };
    if minimal_serialized.len() > max {
        return minimal;
    }

    let mut low = 0usize;
    let mut high = serialized.len();
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        let preview = middle_preview(serialized, mid);
        let candidate =
            tool_result_truncation_envelope_with_preview(original_size_bytes, max, &preview);
        let Ok(candidate_serialized) = serde_json::to_string(&candidate) else {
            high = mid.saturating_sub(1);
            continue;
        };
        if candidate_serialized.len() <= max {
            low = mid;
        } else {
            high = mid.saturating_sub(1);
        }
    }

    let preview = middle_preview(serialized, low);
    tool_result_truncation_envelope_with_preview(original_size_bytes, max, &preview)
}

fn tool_result_truncation_envelope_with_preview(
    original_size_bytes: usize,
    max_result_size_bytes: usize,
    preview: &str,
) -> serde_json::Value {
    serde_json::json!({
        "truncated": true,
        "reason": TOOL_RESULT_SIZE_LIMIT_REASON,
        "original_size_bytes": original_size_bytes,
        "max_result_size_bytes": max_result_size_bytes,
        "preview": preview,
    })
}

fn middle_preview(source: &str, budget_bytes: usize) -> String {
    if budget_bytes <= TOOL_RESULT_PREVIEW_MARKER.len() {
        return String::new();
    }
    let content_budget = budget_bytes - TOOL_RESULT_PREVIEW_MARKER.len();
    let head_budget = content_budget.div_ceil(2);
    let tail_budget = content_budget / 2;
    let head = utf8_prefix(source, head_budget);
    let tail = utf8_suffix(source, tail_budget);
    format!("{head}{TOOL_RESULT_PREVIEW_MARKER}{tail}")
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(value.len());
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn utf8_suffix(value: &str, max_bytes: usize) -> &str {
    let mut start = value.len().saturating_sub(max_bytes);
    while !value.is_char_boundary(start) {
        start += 1;
    }
    &value[start..]
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
    resolved: ResolvedToolCall,
    agent_emitter: &AgentEventEmitter,
    cancel: CancellationToken,
    inputs: ToolExecutionInputs<'_>,
) -> ToolExecutionOutcome {
    let ResolvedToolCall { call, tool, .. } = resolved;
    match tool {
        Some(tool) => {
            let args = ToolArguments::new(call.arguments.clone());
            let ctx = crate::tools::tool::ToolExecutionContext {
                metadata: serde_json::Value::Null,
                tool_call_id: Some(call.id.clone()),
                tool_name: Some(call.name.clone()),
                session_fs: inputs.session_fs,
                session_cwd: inputs.session_cwd,
                sandbox_provider: inputs.sandbox_provider,
                #[cfg(feature = "agent")]
                request_user_input: inputs.user_input_callback.cloned(),
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
            ToolExecutionOutcome {
                call,
                tool: Some(tool),
                result,
            }
        }
        None => ToolExecutionOutcome {
            result: AgentToolResult {
                tool_call_id: call.id.clone(),
                result: serde_json::json!({ "error": format!("Tool '{}' not found", call.name) }),
                is_error: true,
            },
            tool: None,
            call,
        },
    }
}

pub(super) async fn execute_parallel_tool_calls(
    calls: &[ResolvedToolCall],
    agent_emitter: &AgentEventEmitter,
    cancel: CancellationToken,
    inputs: ToolExecutionInputs<'_>,
) -> Vec<ToolExecutionOutcome> {
    let futures = calls
        .iter()
        .cloned()
        .map(|resolved| {
            execute_tool_call(
                resolved,
                agent_emitter,
                cancel.child_token(),
                inputs.clone(),
            )
        })
        .collect::<Vec<_>>();
    future::join_all(futures).await
}

pub(super) fn append_tool_result(
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    call: &AgentToolCall,
    result: AgentToolResult,
    iteration_failures: &mut usize,
    messages: &mut Vec<ModelMessage>,
) -> AgentToolResult {
    append_final_tool_result(
        emitter,
        agent_emitter,
        call,
        result,
        iteration_failures,
        messages,
    )
}

fn append_final_tool_result(
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    call: &AgentToolCall,
    result: AgentToolResult,
    iteration_failures: &mut usize,
    messages: &mut Vec<ModelMessage>,
) -> AgentToolResult {
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
    tool: Option<&dyn Tool>,
    iteration_failures: &mut usize,
    messages: &mut Vec<ModelMessage>,
) -> AgentToolResult {
    let skipped_result = AgentToolResult {
        tool_call_id: call.id.clone(),
        result: serde_json::json!({ "error": "Skipped due to steering message" }),
        is_error: true,
    };
    emit_tool_execution_start(agent_emitter, call);
    let skipped_result = finalize_tool_result(hooks, call, tool, skipped_result).await;
    emit_tool_execution_end(agent_emitter, call, &skipped_result);
    append_final_tool_result(
        emitter,
        agent_emitter,
        call,
        skipped_result.clone(),
        iteration_failures,
        messages,
    )
}
