use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::types::{AgentToolCall, AgentToolResult, ModelMessage};

use super::super::control::{
    approval_allows_execution, resolve_approval, AgentEventEmitter, RunEventEmitter,
};
use super::super::limits::RunnerLimits;
use super::super::message_events::assistant_message_snapshot;
use super::super::message_events::emit_message_lifecycle;
use super::super::tooling::{
    append_skipped_tool_call, append_tool_result, apply_pre_tool_use_hook, canceled_tool_result,
    declined_tool_result, emit_tool_execution_end, emit_tool_execution_start,
    execute_parallel_tool_calls, execute_tool_call, finalize_tool_result, resolve_tool_call,
    safety_plan_for_finalized_call, validate_finalized_tool_call, ResolvedToolCall,
    ToolExecutionInputs, ToolExecutionOutcome,
};
use super::super::{AgentEvent, ApprovalDecision, RunRequest};

pub(super) enum ToolPhaseOutcome {
    ContinueInner,
    BreakInner,
    Canceled,
    Failed(String),
}

pub(super) struct ToolPhaseArgs<'a> {
    pub(super) request: &'a RunRequest,
    pub(super) limits: RunnerLimits,
    pub(super) messages: &'a mut Vec<ModelMessage>,
    pub(super) emitter: &'a RunEventEmitter,
    pub(super) agent_emitter: &'a AgentEventEmitter,
    pub(super) abort_rx: &'a mut oneshot::Receiver<()>,
    pub(super) run_cancel_token: &'a CancellationToken,
    pub(super) turn_index: usize,
    pub(super) tool_calls: &'a [AgentToolCall],
    pub(super) iteration_text: String,
    pub(super) consecutive_failed_iterations: &'a mut usize,
}

pub(super) async fn run_tool_phase(args: ToolPhaseArgs<'_>) -> ToolPhaseOutcome {
    let ToolPhaseArgs {
        request,
        limits,
        messages,
        emitter,
        agent_emitter,
        abort_rx,
        run_cancel_token,
        turn_index,
        tool_calls,
        iteration_text,
        consecutive_failed_iterations,
    } = args;

    let resolved_tool_calls = tool_calls
        .iter()
        .map(|call| resolve_tool_call(&request.tools, call))
        .collect::<Vec<_>>();
    let normalized_tool_calls = resolved_tool_calls
        .iter()
        .map(|resolved| resolved.call.clone())
        .collect::<Vec<_>>();

    let assistant_message = if iteration_text.is_empty() && normalized_tool_calls.is_empty() {
        None
    } else {
        Some(assistant_message_snapshot(
            &iteration_text,
            &normalized_tool_calls,
        ))
    };
    if let Some(message) = assistant_message.as_ref() {
        messages.push(message.clone());
    }

    if normalized_tool_calls.is_empty() {
        agent_emitter.emit(AgentEvent::TurnEnd {
            run_id: request.run_id,
            turn_index,
            assistant_message,
            tool_results: vec![],
        });
        return ToolPhaseOutcome::BreakInner;
    }

    let mut iteration_failures = 0usize;
    let mut turn_tool_results: Vec<AgentToolResult> = Vec::new();
    let mut steering_interrupted = false;
    let mut pending_parallel_calls: Vec<ResolvedToolCall> = Vec::new();
    let tool_inputs = ToolExecutionInputs::new(
        request.session_fs.clone(),
        request.session_cwd.clone(),
        request.sandbox_provider.clone(),
        #[cfg(feature = "agent")]
        request.user_input_callback.as_ref(),
    );

    for (call_idx, resolved_call) in resolved_tool_calls.iter().cloned().enumerate() {
        let pre_tool_use = apply_pre_tool_use_hook(
            &request.hooks,
            &resolved_call.call,
            run_cancel_token.child_token(),
        );
        tokio::pin!(pre_tool_use);
        let pre_tool_use_result = tokio::select! {
            _ = &mut *abort_rx => {
                run_cancel_token.cancel();
                return ToolPhaseOutcome::Canceled;
            }
            result = &mut pre_tool_use => result,
        };
        let finalized_call = match pre_tool_use_result {
            Ok(finalized_call) => finalized_call,
            Err(result) => {
                if !pending_parallel_calls.is_empty() {
                    for parallel_call in &pending_parallel_calls {
                        emit_tool_execution_start(agent_emitter, &parallel_call.call);
                    }
                    let parallel_results = tokio::select! {
                        _ = &mut *abort_rx => {
                            run_cancel_token.cancel();
                            for parallel_call in &pending_parallel_calls {
                                let canceled_result = finalize_tool_result(
                                    &request.hooks,
                                    &parallel_call.call,
                                    parallel_call.tool.as_deref(),
                                    canceled_tool_result(&parallel_call.call),
                                )
                                .await;
                                emit_tool_execution_end(
                                    agent_emitter,
                                    &parallel_call.call,
                                    &canceled_result,
                                );
                            }
                            return ToolPhaseOutcome::Canceled;
                        }
                        results = execute_parallel_tool_calls(
                            &pending_parallel_calls,
                            agent_emitter,
                            run_cancel_token.child_token(),
                            tool_inputs.clone(),
                        ) => results,
                    };
                    pending_parallel_calls.clear();
                    for parallel_outcome in parallel_results {
                        let final_result = finalize_tool_result(
                            &request.hooks,
                            &parallel_outcome.call,
                            parallel_outcome.tool.as_deref(),
                            parallel_outcome.result,
                        )
                        .await;
                        emit_tool_execution_end(
                            agent_emitter,
                            &parallel_outcome.call,
                            &final_result,
                        );
                        let final_result = append_tool_result(
                            emitter,
                            agent_emitter,
                            &parallel_outcome.call,
                            final_result,
                            &mut iteration_failures,
                            messages,
                        );
                        turn_tool_results.push(final_result);
                    }
                }
                emit_tool_execution_start(agent_emitter, &resolved_call.call);
                let final_result = finalize_tool_result(
                    &request.hooks,
                    &resolved_call.call,
                    resolved_call.tool.as_deref(),
                    result,
                )
                .await;
                emit_tool_execution_end(agent_emitter, &resolved_call.call, &final_result);
                let final_result = append_tool_result(
                    emitter,
                    agent_emitter,
                    &resolved_call.call,
                    final_result,
                    &mut iteration_failures,
                    messages,
                );
                turn_tool_results.push(final_result);

                if let Some(ref get_steering) = request.get_steering_messages {
                    let steering = get_steering().await;
                    if !steering.is_empty() {
                        for remaining_call in &resolved_tool_calls[call_idx + 1..] {
                            let skipped = append_skipped_tool_call(
                                &request.hooks,
                                emitter,
                                agent_emitter,
                                &remaining_call.call,
                                remaining_call.tool.as_deref(),
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
                continue;
            }
        };
        let mut resolved_call = ResolvedToolCall {
            call: finalized_call,
            tool: resolved_call.tool.clone(),
            safety_plan: Default::default(),
        };
        if let Err(result) =
            validate_finalized_tool_call(&resolved_call.call, resolved_call.tool.as_deref())
        {
            if !pending_parallel_calls.is_empty() {
                for parallel_call in &pending_parallel_calls {
                    emit_tool_execution_start(agent_emitter, &parallel_call.call);
                }
                let parallel_results = tokio::select! {
                    _ = &mut *abort_rx => {
                        run_cancel_token.cancel();
                        for parallel_call in &pending_parallel_calls {
                            let canceled_result = finalize_tool_result(
                                &request.hooks,
                                &parallel_call.call,
                                parallel_call.tool.as_deref(),
                                canceled_tool_result(&parallel_call.call),
                            )
                            .await;
                            emit_tool_execution_end(
                                agent_emitter,
                                &parallel_call.call,
                                &canceled_result,
                            );
                        }
                        return ToolPhaseOutcome::Canceled;
                    }
                    results = execute_parallel_tool_calls(
                        &pending_parallel_calls,
                        agent_emitter,
                        run_cancel_token.child_token(),
                        tool_inputs.clone(),
                    ) => results,
                };
                pending_parallel_calls.clear();
                for parallel_outcome in parallel_results {
                    let final_result = finalize_tool_result(
                        &request.hooks,
                        &parallel_outcome.call,
                        parallel_outcome.tool.as_deref(),
                        parallel_outcome.result,
                    )
                    .await;
                    emit_tool_execution_end(agent_emitter, &parallel_outcome.call, &final_result);
                    let final_result = append_tool_result(
                        emitter,
                        agent_emitter,
                        &parallel_outcome.call,
                        final_result,
                        &mut iteration_failures,
                        messages,
                    );
                    turn_tool_results.push(final_result);
                }
            }

            let final_result = finalize_tool_result(
                &request.hooks,
                &resolved_call.call,
                resolved_call.tool.as_deref(),
                result,
            )
            .await;
            let final_result = append_tool_result(
                emitter,
                agent_emitter,
                &resolved_call.call,
                final_result,
                &mut iteration_failures,
                messages,
            );
            turn_tool_results.push(final_result);

            if let Some(ref get_steering) = request.get_steering_messages {
                let steering = get_steering().await;
                if !steering.is_empty() {
                    for remaining_call in &resolved_tool_calls[call_idx + 1..] {
                        let skipped = append_skipped_tool_call(
                            &request.hooks,
                            emitter,
                            agent_emitter,
                            &remaining_call.call,
                            remaining_call.tool.as_deref(),
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
            continue;
        }
        resolved_call.safety_plan =
            safety_plan_for_finalized_call(&resolved_call.call, resolved_call.tool.as_deref());
        let approval_tool = resolved_call.tool.clone();
        let decision = {
            let approval = resolve_approval(
                emitter,
                agent_emitter,
                &request.approval_policy,
                request.approval_handler.as_ref(),
                request.human_interaction_coordinator.as_ref(),
                &request.tool_permission_session_approvals,
                &resolved_call.call,
                approval_tool.as_deref(),
                &resolved_call.safety_plan,
            );
            tokio::pin!(approval);
            tokio::select! {
                _ = &mut *abort_rx => {
                    run_cancel_token.cancel();
                    return ToolPhaseOutcome::Canceled;
                }
                decision = &mut approval => decision,
            }
        };

        if matches!(decision, ApprovalDecision::Cancel) {
            run_cancel_token.cancel();
            return ToolPhaseOutcome::Canceled;
        }

        let can_execute = approval_allows_execution(decision);
        if can_execute && resolved_call.safety_plan.concurrency_safe {
            pending_parallel_calls.push(resolved_call);
            continue;
        }

        if !pending_parallel_calls.is_empty() {
            for parallel_call in &pending_parallel_calls {
                emit_tool_execution_start(agent_emitter, &parallel_call.call);
            }
            let parallel_results = tokio::select! {
                _ = &mut *abort_rx => {
                    run_cancel_token.cancel();
                    for parallel_call in &pending_parallel_calls {
                        let canceled_result = finalize_tool_result(
                            &request.hooks,
                            &parallel_call.call,
                            parallel_call.tool.as_deref(),
                            canceled_tool_result(&parallel_call.call),
                        )
                        .await;
                        emit_tool_execution_end(
                            agent_emitter,
                            &parallel_call.call,
                            &canceled_result,
                        );
                    }
                    return ToolPhaseOutcome::Canceled;
                }
                results = execute_parallel_tool_calls(
                    &pending_parallel_calls,
                    agent_emitter,
                    run_cancel_token.child_token(),
                    tool_inputs.clone(),
                ) => results,
            };
            pending_parallel_calls.clear();
            for parallel_outcome in parallel_results {
                let final_result = finalize_tool_result(
                    &request.hooks,
                    &parallel_outcome.call,
                    parallel_outcome.tool.as_deref(),
                    parallel_outcome.result,
                )
                .await;
                emit_tool_execution_end(agent_emitter, &parallel_outcome.call, &final_result);
                let final_result = append_tool_result(
                    emitter,
                    agent_emitter,
                    &parallel_outcome.call,
                    final_result,
                    &mut iteration_failures,
                    messages,
                );
                turn_tool_results.push(final_result);
            }

            if let Some(ref get_steering) = request.get_steering_messages {
                let steering = get_steering().await;
                if !steering.is_empty() {
                    for remaining_call in &resolved_tool_calls[call_idx + 1..] {
                        let skipped = append_skipped_tool_call(
                            &request.hooks,
                            emitter,
                            agent_emitter,
                            &remaining_call.call,
                            remaining_call.tool.as_deref(),
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
            let call_for_cancel = resolved_call.call.clone();
            let tool_for_cancel = resolved_call.tool.clone();
            emit_tool_execution_start(agent_emitter, &call_for_cancel);
            tokio::select! {
                _ = &mut *abort_rx => {
                    run_cancel_token.cancel();
                    let canceled_result = finalize_tool_result(
                        &request.hooks,
                        &call_for_cancel,
                        tool_for_cancel.as_deref(),
                        canceled_tool_result(&call_for_cancel),
                    )
                    .await;
                    emit_tool_execution_end(agent_emitter, &call_for_cancel, &canceled_result);
                    return ToolPhaseOutcome::Canceled;
                }
                outcome = execute_tool_call(
                    resolved_call,
                    agent_emitter,
                    run_cancel_token.child_token(),
                    tool_inputs.clone(),
                ) => outcome,
            }
        } else {
            ToolExecutionOutcome {
                call: resolved_call.call.clone(),
                tool: resolved_call.tool.clone(),
                result: declined_tool_result(&resolved_call.call),
            }
        };
        let final_result = finalize_tool_result(
            &request.hooks,
            &outcome.call,
            outcome.tool.as_deref(),
            outcome.result,
        )
        .await;
        if can_execute {
            emit_tool_execution_end(agent_emitter, &outcome.call, &final_result);
        }

        let final_result = append_tool_result(
            emitter,
            agent_emitter,
            &outcome.call,
            final_result,
            &mut iteration_failures,
            messages,
        );
        turn_tool_results.push(final_result);

        if let Some(ref get_steering) = request.get_steering_messages {
            let steering = get_steering().await;
            if !steering.is_empty() {
                for remaining_call in &resolved_tool_calls[call_idx + 1..] {
                    let skipped = append_skipped_tool_call(
                        &request.hooks,
                        emitter,
                        agent_emitter,
                        &remaining_call.call,
                        remaining_call.tool.as_deref(),
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
            assistant_message: assistant_message.clone(),
            tool_results: turn_tool_results,
        });
        return ToolPhaseOutcome::ContinueInner;
    }

    if !pending_parallel_calls.is_empty() {
        for parallel_call in &pending_parallel_calls {
            emit_tool_execution_start(agent_emitter, &parallel_call.call);
        }
        let parallel_results = tokio::select! {
            _ = &mut *abort_rx => {
                run_cancel_token.cancel();
                for parallel_call in &pending_parallel_calls {
                    let canceled_result = finalize_tool_result(
                        &request.hooks,
                        &parallel_call.call,
                        parallel_call.tool.as_deref(),
                        canceled_tool_result(&parallel_call.call),
                    )
                    .await;
                    emit_tool_execution_end(agent_emitter, &parallel_call.call, &canceled_result);
                }
                return ToolPhaseOutcome::Canceled;
            }
            results = execute_parallel_tool_calls(
                &pending_parallel_calls,
                agent_emitter,
                run_cancel_token.child_token(),
                tool_inputs.clone(),
            ) => results,
        };
        pending_parallel_calls.clear();
        for parallel_outcome in parallel_results {
            let final_result = finalize_tool_result(
                &request.hooks,
                &parallel_outcome.call,
                parallel_outcome.tool.as_deref(),
                parallel_outcome.result,
            )
            .await;
            emit_tool_execution_end(agent_emitter, &parallel_outcome.call, &final_result);
            let final_result = append_tool_result(
                emitter,
                agent_emitter,
                &parallel_outcome.call,
                final_result,
                &mut iteration_failures,
                messages,
            );
            turn_tool_results.push(final_result);
        }
    }

    agent_emitter.emit(AgentEvent::TurnEnd {
        run_id: request.run_id,
        turn_index,
        assistant_message,
        tool_results: turn_tool_results,
    });

    if iteration_failures == normalized_tool_calls.len() {
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
