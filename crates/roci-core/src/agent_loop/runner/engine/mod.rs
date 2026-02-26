use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use crate::provider::{self, ToolDefinition};
use crate::types::ModelMessage;

use super::control::{
    debug_enabled, emit_failed_result, resolve_iteration_limit_approval, AgentEventEmitter,
    RunEventEmitter,
};
use super::limits::RunnerLimits;
use super::message_events::emit_message_lifecycle;
use super::{AgentEvent, ApprovalDecision, LoopRunner, RunEventPayload, RunEventStream, RunHandle};
use super::{RunLifecycle, RunRequest, RunResult, Runner};

mod llm_phase;
mod tool_phase;

use llm_phase::{run_llm_phase, LlmPhaseOutcome};
use tool_phase::{run_tool_phase, ToolPhaseOutcome};

fn canceled_result(
    request: &RunRequest,
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    messages: &[ModelMessage],
) -> RunResult {
    emitter.emit(
        RunEventStream::Lifecycle,
        RunEventPayload::Lifecycle {
            state: RunLifecycle::Canceled,
        },
    );
    agent_emitter.emit(AgentEvent::AgentEnd {
        run_id: request.run_id,
    });
    if debug_enabled() {
        tracing::debug!(run_id = %request.run_id, "roci run canceled");
    }
    RunResult::canceled_with_messages(messages.to_vec())
}

fn failed_result(
    request: &RunRequest,
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    messages: &[ModelMessage],
    reason: impl Into<String>,
) -> RunResult {
    agent_emitter.emit(AgentEvent::AgentEnd {
        run_id: request.run_id,
    });
    emit_failed_result(emitter, reason, messages)
}

#[async_trait]
impl Runner for LoopRunner {
    async fn start(&self, request: RunRequest) -> Result<RunHandle, crate::error::RociError> {
        let (handle, mut abort_rx, result_tx, mut input_rx) = RunHandle::new(request.run_id);
        let config = self.config.clone();
        let provider_factory = self.provider_factory.clone();

        tokio::spawn(async move {
            if debug_enabled() {
                tracing::debug!(
                    run_id = %request.run_id,
                    model = %request.model.to_string(),
                    "roci run start"
                );
            }
            let limits = RunnerLimits::from_request(&request);
            let emitter = RunEventEmitter::new(request.run_id, request.event_sink.clone());
            let agent_emitter = AgentEventEmitter::new(request.agent_event_sink.clone());
            emitter.emit(
                RunEventStream::Lifecycle,
                RunEventPayload::Lifecycle {
                    state: RunLifecycle::Started,
                },
            );
            agent_emitter.emit(AgentEvent::AgentStart {
                run_id: request.run_id,
            });

            if debug_enabled() {
                tracing::debug!(
                    run_id = %request.run_id,
                    max_iterations = limits.max_iterations,
                    max_tool_failures = limits.max_tool_failures,
                    iteration_extension = limits.iteration_extension,
                    max_iteration_extensions = limits.max_iteration_extensions,
                    "roci runner limits"
                );
            }

            let mut messages = request.messages.clone();
            for message in &messages {
                emit_message_lifecycle(&agent_emitter, message);
            }

            if let Err(err) = provider::validate_transport_preference(request.transport.as_deref())
            {
                let _ = result_tx.send(failed_result(
                    &request,
                    &emitter,
                    &agent_emitter,
                    &messages,
                    err.to_string(),
                ));
                return;
            }

            let provider = match provider_factory(&request.model, &config) {
                Ok(provider) => provider,
                Err(err) => {
                    let _ = result_tx.send(failed_result(
                        &request,
                        &emitter,
                        &agent_emitter,
                        &messages,
                        err.to_string(),
                    ));
                    return;
                }
            };

            let tool_defs: Option<Vec<ToolDefinition>> = if request.tools.is_empty() {
                None
            } else {
                Some(
                    request
                        .tools
                        .iter()
                        .map(|t| ToolDefinition {
                            name: t.name().to_string(),
                            description: t.description().to_string(),
                            parameters: t.parameters().schema.clone(),
                        })
                        .collect(),
                )
            };

            let mut iteration = 0usize;
            let mut consecutive_failed_iterations = 0usize;
            let mut max_iterations = limits.max_iterations;
            let mut iteration_extensions_used = 0usize;
            let mut turn_index = 0usize;
            let run_cancel_token = CancellationToken::new();

            'outer: loop {
                'inner: loop {
                    iteration += 1;
                    turn_index += 1;
                    agent_emitter.emit(AgentEvent::TurnStart {
                        run_id: request.run_id,
                        turn_index,
                    });

                    if iteration > max_iterations {
                        if iteration_extensions_used >= limits.max_iteration_extensions {
                            let reason = format!(
                                "tool loop exceeded max iterations (max_iterations={}, extensions_used={})",
                                max_iterations, iteration_extensions_used
                            );
                            let _ = result_tx.send(failed_result(
                                &request,
                                &emitter,
                                &agent_emitter,
                                &messages,
                                reason,
                            ));
                            return;
                        }

                        let decision = resolve_iteration_limit_approval(
                            &emitter,
                            request.approval_handler.as_ref(),
                            request.run_id,
                            iteration,
                            max_iterations,
                            limits.iteration_extension,
                            iteration_extensions_used + 1,
                        )
                        .await;

                        match decision {
                            ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
                                max_iterations =
                                    max_iterations.saturating_add(limits.iteration_extension);
                                iteration_extensions_used =
                                    iteration_extensions_used.saturating_add(1);
                                if debug_enabled() {
                                    tracing::debug!(
                                        run_id = %request.run_id,
                                        iteration,
                                        max_iterations,
                                        iteration_extensions_used,
                                        "roci iteration limit extended"
                                    );
                                }
                            }
                            ApprovalDecision::Cancel => {
                                let _ = result_tx.send(canceled_result(
                                    &request,
                                    &emitter,
                                    &agent_emitter,
                                    &messages,
                                ));
                                return;
                            }
                            ApprovalDecision::Decline => {
                                let reason = format!(
                                    "tool loop exceeded max iterations (max_iterations={max_iterations}); continuation declined"
                                );
                                let _ = result_tx.send(failed_result(
                                    &request,
                                    &emitter,
                                    &agent_emitter,
                                    &messages,
                                    reason,
                                ));
                                return;
                            }
                        }
                    }

                    let (iteration_text, tool_calls) = match run_llm_phase(
                        &request,
                        provider.as_ref(),
                        &tool_defs,
                        &mut messages,
                        &emitter,
                        &agent_emitter,
                        &mut input_rx,
                        &mut abort_rx,
                        &run_cancel_token,
                        iteration,
                    )
                    .await
                    {
                        LlmPhaseOutcome::Ready {
                            iteration_text,
                            tool_calls,
                        } => (iteration_text, tool_calls),
                        LlmPhaseOutcome::Canceled => {
                            let _ = result_tx.send(canceled_result(
                                &request,
                                &emitter,
                                &agent_emitter,
                                &messages,
                            ));
                            return;
                        }
                        LlmPhaseOutcome::Failed(reason) => {
                            let _ = result_tx.send(failed_result(
                                &request,
                                &emitter,
                                &agent_emitter,
                                &messages,
                                reason,
                            ));
                            return;
                        }
                    };

                    match run_tool_phase(
                        &request,
                        limits,
                        &mut messages,
                        &emitter,
                        &agent_emitter,
                        &mut abort_rx,
                        &run_cancel_token,
                        turn_index,
                        &tool_calls,
                        iteration_text,
                        &mut consecutive_failed_iterations,
                    )
                    .await
                    {
                        ToolPhaseOutcome::ContinueInner => continue 'inner,
                        ToolPhaseOutcome::BreakInner => break 'inner,
                        ToolPhaseOutcome::Canceled => {
                            let _ = result_tx.send(canceled_result(
                                &request,
                                &emitter,
                                &agent_emitter,
                                &messages,
                            ));
                            return;
                        }
                        ToolPhaseOutcome::Failed(reason) => {
                            let _ = result_tx.send(failed_result(
                                &request,
                                &emitter,
                                &agent_emitter,
                                &messages,
                                reason,
                            ));
                            return;
                        }
                    }
                }

                if let Some(ref get_follow_ups) = request.get_follow_up_messages {
                    let follow_ups = get_follow_ups().await;
                    if !follow_ups.is_empty() {
                        for msg in follow_ups {
                            emit_message_lifecycle(&agent_emitter, &msg);
                            messages.push(msg);
                        }
                        continue 'outer;
                    }
                }

                emitter.emit(
                    RunEventStream::Lifecycle,
                    RunEventPayload::Lifecycle {
                        state: RunLifecycle::Completed,
                    },
                );
                agent_emitter.emit(AgentEvent::AgentEnd {
                    run_id: request.run_id,
                });
                let _ = result_tx.send(RunResult::completed_with_messages(messages));
                if debug_enabled() {
                    tracing::debug!(run_id = %request.run_id, "roci run completed");
                }
                return;
            }
        });

        Ok(handle)
    }
}
