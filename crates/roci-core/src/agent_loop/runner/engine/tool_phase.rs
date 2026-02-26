use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::types::{message::ContentPart, AgentToolCall, AgentToolResult, ModelMessage};

use super::super::control::{
    approval_allows_execution, resolve_approval, AgentEventEmitter, RunEventEmitter,
};
use super::super::limits::{is_parallel_safe_tool, RunnerLimits};
use super::super::message_events::emit_message_lifecycle;
use super::super::tooling::{
    append_skipped_tool_call, append_tool_result, apply_post_tool_use_hook, canceled_tool_result,
    declined_tool_result, emit_tool_execution_end, emit_tool_execution_start,
    execute_parallel_tool_calls, execute_tool_call, ToolExecutionOutcome,
};
use super::super::{AgentEvent, ApprovalDecision, RunRequest};

pub(super) enum ToolPhaseOutcome {
    ContinueInner,
    BreakInner,
    Canceled,
    Failed(String),
}

pub(super) async fn run_tool_phase(
    request: &RunRequest,
    limits: RunnerLimits,
    messages: &mut Vec<ModelMessage>,
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    abort_rx: &mut oneshot::Receiver<()>,
    run_cancel_token: &CancellationToken,
    turn_index: usize,
    tool_calls: &[AgentToolCall],
    iteration_text: String,
    consecutive_failed_iterations: &mut usize,
) -> ToolPhaseOutcome {
    if tool_calls.is_empty() {
        agent_emitter.emit(AgentEvent::TurnEnd {
            run_id: request.run_id,
            turn_index,
            tool_results: vec![],
        });
        return ToolPhaseOutcome::BreakInner;
    }

    let mut assistant_content: Vec<ContentPart> = Vec::new();
    if !iteration_text.is_empty() {
        assistant_content.push(ContentPart::Text {
            text: iteration_text,
        });
    }
    for call in tool_calls {
        assistant_content.push(ContentPart::ToolCall(call.clone()));
    }
    messages.push(ModelMessage {
        role: crate::types::Role::Assistant,
        content: assistant_content,
        name: None,
        timestamp: Some(chrono::Utc::now()),
    });

    let mut iteration_failures = 0usize;
    let mut turn_tool_results: Vec<AgentToolResult> = Vec::new();
    let mut steering_interrupted = false;
    let mut pending_parallel_calls: Vec<AgentToolCall> = Vec::new();

    for (call_idx, call) in tool_calls.iter().enumerate() {
        let decision = resolve_approval(
            emitter,
            &request.approval_policy,
            request.approval_handler.as_ref(),
            call,
        )
        .await;

        if matches!(decision, ApprovalDecision::Cancel) {
            run_cancel_token.cancel();
            return ToolPhaseOutcome::Canceled;
        }

        let can_execute = approval_allows_execution(decision);
        if can_execute && is_parallel_safe_tool(&call.name) {
            pending_parallel_calls.push(call.clone());
            continue;
        }

        if !pending_parallel_calls.is_empty() {
            for parallel_call in &pending_parallel_calls {
                emit_tool_execution_start(agent_emitter, parallel_call);
            }
            let parallel_results = tokio::select! {
                _ = &mut *abort_rx => {
                    run_cancel_token.cancel();
                    for parallel_call in &pending_parallel_calls {
                        let canceled_result = apply_post_tool_use_hook(
                            &request.hooks,
                            parallel_call,
                            canceled_tool_result(parallel_call),
                        )
                        .await;
                        emit_tool_execution_end(agent_emitter, parallel_call, &canceled_result);
                    }
                    return ToolPhaseOutcome::Canceled;
                }
                results = execute_parallel_tool_calls(
                    &request.tools,
                    &request.hooks,
                    &pending_parallel_calls,
                    agent_emitter,
                    run_cancel_token.child_token(),
                ) => results,
            };
            pending_parallel_calls.clear();
            for parallel_outcome in parallel_results {
                emit_tool_execution_end(
                    agent_emitter,
                    &parallel_outcome.call,
                    &parallel_outcome.result,
                );
                let final_result = append_tool_result(
                    &request.hooks,
                    emitter,
                    agent_emitter,
                    &parallel_outcome.call,
                    parallel_outcome.result,
                    &mut iteration_failures,
                    messages,
                )
                .await;
                turn_tool_results.push(final_result);
            }

            if let Some(ref get_steering) = request.get_steering_messages {
                let steering = get_steering().await;
                if !steering.is_empty() {
                    for remaining_call in &tool_calls[call_idx + 1..] {
                        let skipped = append_skipped_tool_call(
                            &request.hooks,
                            emitter,
                            agent_emitter,
                            remaining_call,
                            &mut iteration_failures,
                            messages,
                        )
                        .await;
                        turn_tool_results.push(skipped);
                    }
                    for msg in steering {
                        emit_message_lifecycle(agent_emitter, &msg);
                        messages.push(msg);
                    }
                    steering_interrupted = true;
                    break;
                }
            }
        }

        let outcome = if can_execute {
            emit_tool_execution_start(agent_emitter, call);
            tokio::select! {
                _ = &mut *abort_rx => {
                    run_cancel_token.cancel();
                    let canceled_result = apply_post_tool_use_hook(
                        &request.hooks,
                        call,
                        canceled_tool_result(call),
                    )
                    .await;
                    emit_tool_execution_end(agent_emitter, call, &canceled_result);
                    return ToolPhaseOutcome::Canceled;
                }
                outcome = execute_tool_call(
                    &request.tools,
                    &request.hooks,
                    call,
                    agent_emitter,
                    run_cancel_token.child_token(),
                ) => outcome,
            }
        } else {
            ToolExecutionOutcome {
                call: call.clone(),
                result: declined_tool_result(call),
            }
        };
        if can_execute {
            emit_tool_execution_end(agent_emitter, &outcome.call, &outcome.result);
        }

        let final_result = append_tool_result(
            &request.hooks,
            emitter,
            agent_emitter,
            &outcome.call,
            outcome.result,
            &mut iteration_failures,
            messages,
        )
        .await;
        turn_tool_results.push(final_result);

        if let Some(ref get_steering) = request.get_steering_messages {
            let steering = get_steering().await;
            if !steering.is_empty() {
                for remaining_call in &tool_calls[call_idx + 1..] {
                    let skipped = append_skipped_tool_call(
                        &request.hooks,
                        emitter,
                        agent_emitter,
                        remaining_call,
                        &mut iteration_failures,
                        messages,
                    )
                    .await;
                    turn_tool_results.push(skipped);
                }
                for msg in steering {
                    emit_message_lifecycle(agent_emitter, &msg);
                    messages.push(msg);
                }
                steering_interrupted = true;
                break;
            }
        }
    }

    if steering_interrupted {
        agent_emitter.emit(AgentEvent::TurnEnd {
            run_id: request.run_id,
            turn_index,
            tool_results: turn_tool_results,
        });
        return ToolPhaseOutcome::ContinueInner;
    }

    if !pending_parallel_calls.is_empty() {
        for parallel_call in &pending_parallel_calls {
            emit_tool_execution_start(agent_emitter, parallel_call);
        }
        let parallel_results = tokio::select! {
            _ = &mut *abort_rx => {
                run_cancel_token.cancel();
                for parallel_call in &pending_parallel_calls {
                    let canceled_result = apply_post_tool_use_hook(
                        &request.hooks,
                        parallel_call,
                        canceled_tool_result(parallel_call),
                    )
                    .await;
                    emit_tool_execution_end(agent_emitter, parallel_call, &canceled_result);
                }
                return ToolPhaseOutcome::Canceled;
            }
            results = execute_parallel_tool_calls(
                &request.tools,
                &request.hooks,
                &pending_parallel_calls,
                agent_emitter,
                run_cancel_token.child_token(),
            ) => results,
        };
        pending_parallel_calls.clear();
        for parallel_outcome in parallel_results {
            emit_tool_execution_end(
                agent_emitter,
                &parallel_outcome.call,
                &parallel_outcome.result,
            );
            let final_result = append_tool_result(
                &request.hooks,
                emitter,
                agent_emitter,
                &parallel_outcome.call,
                parallel_outcome.result,
                &mut iteration_failures,
                messages,
            )
            .await;
            turn_tool_results.push(final_result);
        }
    }

    agent_emitter.emit(AgentEvent::TurnEnd {
        run_id: request.run_id,
        turn_index,
        tool_results: turn_tool_results,
    });

    if iteration_failures == tool_calls.len() {
        *consecutive_failed_iterations = consecutive_failed_iterations.saturating_add(1);
    } else {
        *consecutive_failed_iterations = 0;
    }

    if *consecutive_failed_iterations >= limits.max_tool_failures {
        return ToolPhaseOutcome::Failed(format!(
            "tool call failure limit reached (max_failures={}, consecutive_failures={})",
            limits.max_tool_failures, *consecutive_failed_iterations
        ));
    }

    ToolPhaseOutcome::ContinueInner
}
