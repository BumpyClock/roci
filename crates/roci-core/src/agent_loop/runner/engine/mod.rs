use async_trait::async_trait;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::models::{HealthSignal, ModelHealthKey};
use crate::provider::{self, ToolDefinition};
use crate::tools::{ToolCatalog, ToolOrigin};
use crate::types::{ModelMessage, Usage};

use super::control::{
    emit_failed_result, resolve_iteration_limit_approval, AgentEventEmitter,
    IterationLimitApprovalContext, RunEventEmitter,
};
use super::limits::RunnerLimits;
use super::message_events::emit_message_lifecycle;
use super::{AgentEvent, ApprovalDecision, LoopRunner, RunEventPayload, RunEventStream, RunHandle};
use super::{RunLifecycle, RunRequest, RunResult, Runner};
use crate::agent_loop::{FailureCategory, RetryEvent, RetryEventKind, RetryMode, RetryNextAction};
use crate::util::debug::roci_debug_enabled;

mod llm_phase;
mod tool_phase;

use llm_phase::{
    failure_category_for_error, run_llm_phase, ExactUsageAnchor, LlmPhaseArgs, LlmPhaseOutcome,
};
use tool_phase::{run_tool_phase, ToolPhaseArgs, ToolPhaseOutcome};

fn canceled_result(
    request: &RunRequest,
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    messages: &[ModelMessage],
    run_usage: Usage,
) -> RunResult {
    if let Some(health) = request.model_health.as_ref() {
        health.observe(HealthSignal::Canceled {
            key: ModelHealthKey::from_model(request.active_model()),
            observed_at_ms: now_ms(),
        });
    }
    emitter.emit(
        RunEventStream::Lifecycle,
        RunEventPayload::Lifecycle {
            state: RunLifecycle::Canceled,
        },
    );
    agent_emitter.emit(AgentEvent::AgentEnd {
        run_id: request.run_id,
        messages: messages.to_vec(),
    });
    if roci_debug_enabled() {
        tracing::debug!(run_id = %request.run_id, "roci run canceled");
    }
    RunResult::canceled_with_messages(messages.to_vec()).with_usage_delta(run_usage)
}

fn failed_result(
    request: &RunRequest,
    emitter: &RunEventEmitter,
    agent_emitter: &AgentEventEmitter,
    messages: &[ModelMessage],
    reason: impl Into<String>,
    run_usage: Usage,
) -> RunResult {
    agent_emitter.emit(AgentEvent::AgentEnd {
        run_id: request.run_id,
        messages: messages.to_vec(),
    });
    emit_failed_result(emitter, reason, messages).with_usage_delta(run_usage)
}

fn should_advance_candidate(
    request: &RunRequest,
    failure_category: FailureCategory,
    partial_output_seen: bool,
) -> bool {
    matches!(request.retry_mode, RetryMode::Bounded { .. })
        && is_transient_for_advance(failure_category)
        && request.candidates_remaining() > 0
        && !partial_output_seen
}

fn is_transient_for_advance(category: FailureCategory) -> bool {
    matches!(
        category,
        FailureCategory::RateLimit
            | FailureCategory::Network
            | FailureCategory::Server
            | FailureCategory::Timeout
    )
}

fn emit_candidate_advancing(
    request: &RunRequest,
    emitter: &RunEventEmitter,
    retry_started_at: Instant,
    from_index: usize,
    from: &crate::models::LanguageModel,
    failure_category: FailureCategory,
    partial_output_seen: bool,
) {
    emitter.emit(
        RunEventStream::System,
        RunEventPayload::Retry {
            event: RetryEvent {
                kind: RetryEventKind::CandidateAdvancing,
                run_id: request.run_id,
                provider: from.provider_name().to_string(),
                model_id: from.model_id().to_string(),
                candidate_index: from_index,
                attempt: bounded_max_attempts(request.retry_mode),
                retry_mode: request.retry_mode,
                failure_category,
                sleep_ms: None,
                elapsed_retry_ms: elapsed_retry_ms(retry_started_at),
                candidates_remaining: request.candidates_remaining(),
                partial_output_seen,
                next_action: RetryNextAction::AdvanceCandidate,
            },
        },
    );
    if roci_debug_enabled() {
        tracing::debug!(
            run_id = %request.run_id,
            from = %from,
            to = %request.active_model(),
            "roci candidate advancing"
        );
    }
}

fn emit_retry_exhausted(
    request: &RunRequest,
    emitter: &RunEventEmitter,
    retry_started_at: Instant,
    failure_category: FailureCategory,
    partial_output_seen: bool,
) {
    let model = request.active_model();
    emitter.emit(
        RunEventStream::System,
        RunEventPayload::Retry {
            event: RetryEvent {
                kind: RetryEventKind::RetryExhausted,
                run_id: request.run_id,
                provider: model.provider_name().to_string(),
                model_id: model.model_id().to_string(),
                candidate_index: request.active_candidate_index,
                attempt: bounded_max_attempts(request.retry_mode),
                retry_mode: request.retry_mode,
                failure_category,
                sleep_ms: None,
                elapsed_retry_ms: elapsed_retry_ms(retry_started_at),
                candidates_remaining: request.candidates_remaining(),
                partial_output_seen,
                next_action: RetryNextAction::ReturnFailure,
            },
        },
    );
}

fn bounded_max_attempts(retry_mode: RetryMode) -> u32 {
    match retry_mode {
        RetryMode::Bounded { max_attempts } => max_attempts,
        RetryMode::Persistent => 0,
    }
}

fn validate_retry_mode(retry_mode: RetryMode) -> Result<(), crate::error::RociError> {
    if matches!(retry_mode, RetryMode::Bounded { max_attempts: 0 }) {
        return Err(crate::error::RociError::Configuration(
            "retry_mode bounded max_attempts must be at least 1".to_string(),
        ));
    }
    Ok(())
}

fn elapsed_retry_ms(retry_started_at: Instant) -> u64 {
    u64::try_from(retry_started_at.elapsed().as_millis()).unwrap_or(u64::MAX)
}

fn observe_failure(request: &RunRequest, category: FailureCategory) {
    let Some(health) = request.model_health.as_ref() else {
        return;
    };
    let key = ModelHealthKey::from_model(request.active_model());
    let observed_at_ms = now_ms();
    if is_transient_for_advance(category) {
        health.observe(HealthSignal::TransientFailure {
            key,
            category,
            observed_at_ms,
        });
    } else {
        health.observe(HealthSignal::NonRetryableFailure {
            key,
            category,
            observed_at_ms,
        });
    }
}

fn observe_retry_exhausted(request: &RunRequest, category: FailureCategory) {
    let Some(health) = request.model_health.as_ref() else {
        return;
    };
    health.observe(HealthSignal::RetryExhausted {
        candidate_index: request.active_candidate_index,
        key: ModelHealthKey::from_model(request.active_model()),
        category,
        observed_at_ms: now_ms(),
    });
}

fn observe_success(request: &RunRequest) {
    let Some(health) = request.model_health.as_ref() else {
        return;
    };
    health.observe(HealthSignal::Success {
        key: ModelHealthKey::from_model(request.active_model()),
        observed_at_ms: now_ms(),
    });
}

async fn resolve_active_provider_api_key(
    request: &mut RunRequest,
    config: &crate::config::RociConfig,
) -> Result<(), crate::error::RociError> {
    let model = request.active_model().clone();
    let provider = model.provider_name().to_string();
    if request.active_api_key_override().is_some() || config.get_api_key(&provider).is_some() {
        return Ok(());
    }
    let Some(get_key) = request.get_api_key.clone() else {
        return Ok(());
    };
    let key = get_key(model).await?;
    request.api_key_overrides.insert(provider, key);
    Ok(())
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

#[async_trait]
impl Runner for LoopRunner {
    async fn start(&self, mut request: RunRequest) -> Result<RunHandle, crate::error::RociError> {
        validate_retry_mode(request.retry_mode)?;
        request.tools = ToolCatalog::from_tools(request.tools, ToolOrigin::Custom)
            .resolve(&request.tool_visibility_policy);
        let (handle, mut abort_rx, result_tx, mut input_rx) = RunHandle::new(request.run_id);
        let config = self.config.clone();
        let provider_factory = self.provider_factory.clone();

        tokio::spawn(async move {
            if roci_debug_enabled() {
                tracing::debug!(
                    run_id = %request.run_id,
                    model = %request.active_model().to_string(),
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

            if roci_debug_enabled() {
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

            // Run-local usage accumulator across all LLM calls in this run.
            let mut run_usage = Usage::default();
            // Anchor from the last successful provider call for exact-prefix
            // token estimation in preflight budget checks.
            let mut exact_anchor: Option<ExactUsageAnchor> = None;
            let mut active_provider: Option<(usize, Box<dyn provider::ModelProvider>)> = None;
            let mut retry_started_at = Instant::now();

            if let Err(err) = provider::validate_transport_preference(request.transport.as_deref())
            {
                let _ = result_tx.send(failed_result(
                    &request,
                    &emitter,
                    &agent_emitter,
                    &messages,
                    err.to_string(),
                    run_usage,
                ));
                return;
            }

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

                    if let Err(err) = resolve_active_provider_api_key(&mut request, &config).await {
                        let _ = result_tx.send(failed_result(
                            &request,
                            &emitter,
                            &agent_emitter,
                            &messages,
                            err.to_string(),
                            run_usage,
                        ));
                        return;
                    }

                    if active_provider.as_ref().map(|(index, _)| *index)
                        != Some(request.active_candidate_index)
                    {
                        active_provider = match provider_factory(request.active_model(), &config) {
                            Ok(provider) => Some((request.active_candidate_index, provider)),
                            Err(err) => {
                                observe_failure(&request, failure_category_for_error(&err));
                                let _ = result_tx.send(failed_result(
                                    &request,
                                    &emitter,
                                    &agent_emitter,
                                    &messages,
                                    err.to_string(),
                                    run_usage,
                                ));
                                return;
                            }
                        };
                    }

                    let provider = active_provider
                        .as_ref()
                        .expect("active provider exists")
                        .1
                        .as_ref();

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
                                run_usage,
                            ));
                            return;
                        }

                        let approval = resolve_iteration_limit_approval(
                            &emitter,
                            &agent_emitter,
                            request.approval_handler.as_ref(),
                            IterationLimitApprovalContext {
                                run_id: request.run_id,
                                iteration,
                                current_limit: max_iterations,
                                extension: limits.iteration_extension,
                                attempt: iteration_extensions_used + 1,
                            },
                        );
                        tokio::pin!(approval);
                        let decision = tokio::select! {
                            _ = &mut abort_rx => {
                                run_cancel_token.cancel();
                                let _ = result_tx.send(canceled_result(
                                    &request,
                                    &emitter,
                                    &agent_emitter,
                                    &messages,
                                    run_usage,
                                ));
                                return;
                            }
                            decision = &mut approval => decision,
                        };

                        match decision {
                            ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
                                max_iterations =
                                    max_iterations.saturating_add(limits.iteration_extension);
                                iteration_extensions_used =
                                    iteration_extensions_used.saturating_add(1);
                                if roci_debug_enabled() {
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
                                    run_usage,
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
                                    run_usage,
                                ));
                                return;
                            }
                        }
                    }

                    let (iteration_text, tool_calls) = match run_llm_phase(LlmPhaseArgs {
                        request: &request,
                        provider,
                        tool_defs: &tool_defs,
                        messages: &mut messages,
                        emitter: &emitter,
                        agent_emitter: &agent_emitter,
                        input_rx: &mut input_rx,
                        abort_rx: &mut abort_rx,
                        run_cancel_token: &run_cancel_token,
                        iteration,
                        run_usage: &mut run_usage,
                        exact_anchor: &mut exact_anchor,
                        retry_started_at: &retry_started_at,
                    })
                    .await
                    {
                        LlmPhaseOutcome::Ready {
                            iteration_text,
                            tool_calls,
                        } => (iteration_text, tool_calls),
                        LlmPhaseOutcome::Canceled { assistant_message } => {
                            if let Some(message) = assistant_message {
                                messages.push(message);
                            }
                            let _ = result_tx.send(canceled_result(
                                &request,
                                &emitter,
                                &agent_emitter,
                                &messages,
                                run_usage,
                            ));
                            return;
                        }
                        LlmPhaseOutcome::Failed {
                            reason,
                            assistant_message,
                            failure_category,
                        } => {
                            let partial_output_seen = assistant_message.is_some();
                            if let Some(message) = assistant_message {
                                messages.push(message);
                            }
                            if should_advance_candidate(
                                &request,
                                failure_category,
                                partial_output_seen,
                            ) {
                                let from_index = request.active_candidate_index;
                                let to_index = from_index + 1;
                                let from = request.active_model().clone();
                                observe_retry_exhausted(&request, failure_category);
                                request.active_candidate_index = to_index;
                                let to = request.active_model().clone();
                                emit_candidate_advancing(
                                    &request,
                                    &emitter,
                                    retry_started_at,
                                    from_index,
                                    &from,
                                    failure_category,
                                    partial_output_seen,
                                );
                                if let Some(health) = request.model_health.as_ref() {
                                    health.observe(HealthSignal::CandidateAdvanced {
                                        from_index,
                                        to_index,
                                        from: ModelHealthKey::from_model(&from),
                                        to: ModelHealthKey::from_model(&to),
                                        reason: failure_category,
                                        observed_at_ms: now_ms(),
                                    });
                                }
                                retry_started_at = Instant::now();
                                active_provider = None;
                                continue 'inner;
                            }
                            emit_retry_exhausted(
                                &request,
                                &emitter,
                                retry_started_at,
                                failure_category,
                                partial_output_seen,
                            );
                            observe_retry_exhausted(&request, failure_category);
                            let _ = result_tx.send(failed_result(
                                &request,
                                &emitter,
                                &agent_emitter,
                                &messages,
                                reason,
                                run_usage,
                            ));
                            return;
                        }
                    };

                    match run_tool_phase(ToolPhaseArgs {
                        request: &request,
                        limits,
                        messages: &mut messages,
                        emitter: &emitter,
                        agent_emitter: &agent_emitter,
                        abort_rx: &mut abort_rx,
                        run_cancel_token: &run_cancel_token,
                        turn_index,
                        tool_calls: &tool_calls,
                        iteration_text,
                        consecutive_failed_iterations: &mut consecutive_failed_iterations,
                    })
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
                                run_usage,
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
                                run_usage,
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
                observe_success(&request);
                agent_emitter.emit(AgentEvent::AgentEnd {
                    run_id: request.run_id,
                    messages: messages.clone(),
                });
                let _ = result_tx
                    .send(RunResult::completed_with_messages(messages).with_usage_delta(run_usage));
                if roci_debug_enabled() {
                    tracing::debug!(run_id = %request.run_id, "roci run completed");
                }
                return;
            }
        });

        Ok(handle)
    }
}
