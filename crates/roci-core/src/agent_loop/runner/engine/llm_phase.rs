use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;

use super::super::control::{process_stream_delta, AgentEventEmitter, RunEventEmitter};
use super::super::message_events::{
    assistant_message_snapshot, emit_message_end_if_open, emit_message_lifecycle,
};
use super::super::{
    ConvertToLlmHookPayload, ConvertToLlmHookResult, RunEventPayload, RunEventStream, RunRequest,
    TransformContextHookPayload, TransformContextHookResult,
};
use crate::agent::message::{convert_to_llm, AgentMessage};
use crate::context::{
    estimate_context_usage, estimate_message_tokens, AbortReason, CompactionProgress,
    OverflowRecoveryPolicy, RecoveryAction, RecoveryEvent, RecoveryState,
};
use crate::error::RociError;
use crate::provider::{self, ProviderRequest, ToolDefinition};
use crate::types::{AgentToolCall, GenerationSettings, ModelMessage, Usage};
use crate::util::debug::roci_debug_enabled;

/// Compute the effective usage for a single provider call, merging it into
/// the run-local accumulator.
///
/// When the provider reported usage via `call_usage`, that value is used.
/// Otherwise a heuristic estimate is produced from the **provider-facing**
/// request messages and assistant output so the run-local accumulator
/// still moves forward for failed or canceled post-provider exits.
///
/// This **must** be called before every return from the streaming loop
/// that occurs *after* `stream_text` succeeded — including cancel, timeout,
/// and error exits — to avoid dropping usage for partially-consumed streams.
fn finalize_call_usage(
    call_usage: Option<Usage>,
    provider_messages: &[ModelMessage],
    iteration_text: &str,
    tool_calls: &[AgentToolCall],
    run_usage: &mut Usage,
) {
    let effective = call_usage.unwrap_or_else(|| {
        let input_est: usize = provider_messages.iter().map(estimate_message_tokens).sum();
        let output_est = assistant_snapshot_if_present(iteration_text, tool_calls)
            .as_ref()
            .map(estimate_message_tokens)
            .unwrap_or(0);
        Usage {
            input_tokens: input_est as u32,
            output_tokens: output_est as u32,
            total_tokens: (input_est + output_est) as u32,
            ..Usage::default()
        }
    });
    run_usage.merge(&effective);
}

/// Anchor from a prior successful provider call, enabling the
/// [`from_provider_with_tail`](crate::context::ContextUsageSnapshot::from_provider_with_tail)
/// path for more accurate preflight token estimates.
///
/// When the messages sent to a new call are a prefix-match of the
/// anchor's messages plus a heuristic tail, the preflight can combine
/// exact provider counts with the estimated tail instead of re-counting
/// the entire history heuristically.
pub(super) struct ExactUsageAnchor {
    /// The provider messages from the prior call.
    pub(super) provider_messages: Vec<ModelMessage>,
    /// Provider-reported prompt (input) tokens for those messages.
    pub(super) prompt_tokens: usize,
}

pub(super) enum LlmPhaseOutcome {
    Ready {
        iteration_text: String,
        tool_calls: Vec<AgentToolCall>,
    },
    Canceled {
        assistant_message: Option<ModelMessage>,
    },
    Failed {
        reason: String,
        assistant_message: Option<ModelMessage>,
    },
}

pub(super) struct LlmPhaseArgs<'a> {
    pub(super) request: &'a RunRequest,
    pub(super) provider: &'a dyn provider::ModelProvider,
    pub(super) tool_defs: &'a Option<Vec<ToolDefinition>>,
    pub(super) messages: &'a mut Vec<ModelMessage>,
    pub(super) emitter: &'a RunEventEmitter,
    pub(super) agent_emitter: &'a AgentEventEmitter,
    pub(super) input_rx: &'a mut mpsc::UnboundedReceiver<ModelMessage>,
    pub(super) abort_rx: &'a mut oneshot::Receiver<()>,
    pub(super) run_cancel_token: &'a CancellationToken,
    pub(super) iteration: usize,
    /// Run-local usage accumulator; merged after each provider call
    /// (including failed, canceled, and error exits after streaming began).
    pub(super) run_usage: &'a mut Usage,
    /// Optional anchor from a prior call for exact-prefix token estimation.
    pub(super) exact_anchor: &'a mut Option<ExactUsageAnchor>,
}

pub(super) async fn run_llm_phase(args: LlmPhaseArgs<'_>) -> LlmPhaseOutcome {
    let LlmPhaseArgs {
        request,
        provider,
        tool_defs,
        messages,
        emitter,
        agent_emitter,
        input_rx,
        abort_rx,
        run_cancel_token,
        iteration,
        run_usage,
        exact_anchor,
    } = args;

    while let Ok(message) = input_rx.try_recv() {
        emit_message_lifecycle(agent_emitter, &message);
        messages.push(message);
    }

    if let Some(ref get_steering) = request.get_steering_messages {
        for msg in get_steering().await {
            emit_message_lifecycle(agent_emitter, &msg);
            messages.push(msg);
        }
    }

    let should_compact = request.auto_compaction.as_ref().is_some_and(|config| {
        let usage = estimate_context_usage(&*messages, provider.capabilities().context_length);
        usage.used_tokens > usage.context_window.saturating_sub(config.reserve_tokens)
    });
    if should_compact {
        let Some(compact) = request.hooks.compaction.as_ref() else {
            return LlmPhaseOutcome::Failed {
                reason: "auto-compaction is enabled but no compaction hook is configured"
                    .to_string(),
                assistant_message: None,
            };
        };
        let compaction_cancel_token = run_cancel_token.child_token();
        let compaction_future = compact(messages.clone(), compaction_cancel_token.clone());
        tokio::pin!(compaction_future);
        let compaction_result = tokio::select! {
            _ = &mut *abort_rx => {
                run_cancel_token.cancel();
                compaction_cancel_token.cancel();
                return LlmPhaseOutcome::Canceled {
                    assistant_message: None,
                };
            }
            result = &mut compaction_future => result,
        };
        match compaction_result {
            Ok(Some(compacted)) => {
                *messages = compacted;
            }
            Ok(None) => {}
            Err(err) => {
                return LlmPhaseOutcome::Failed {
                    reason: format!("compaction failed: {err}"),
                    assistant_message: None,
                };
            }
        }
    }

    let max_attempts = request.retry_backoff.max_attempts.max(1);
    let mut attempt: u32 = 1;
    let mut next_backoff_ms = request.retry_backoff.initial_delay_ms.max(1);

    // -- Overflow recovery state (separate from generic retry) --
    // Carries per-attempt adjustments such as a reduced `max_tokens`
    // budget without mutating the original run request.
    let mut effective_settings = request.settings.clone();
    let policy = OverflowRecoveryPolicy::new();
    let mut recovery_state = RecoveryState::new();
    let mut in_overflow_episode = false;
    let mut staged_provider_request: Option<ProviderRequest> = None;

    let (mut stream, last_provider_messages) = loop {
        let provider_request = match staged_provider_request.take() {
            Some(request) => request,
            None => match build_provider_request(
                request,
                provider,
                tool_defs,
                messages,
                abort_rx,
                run_cancel_token,
                &effective_settings,
            )
            .await
            {
                Ok(req) => req,
                Err(outcome) => return outcome,
            },
        };

        // -- Preflight budget check (5.3) --
        // Uses exact-anchor path when available: if the prior call's
        // provider messages are a prefix of the current request, combine
        // exact provider counts with a heuristic tail estimate.
        if let Some(ref budget) = request.context_budget {
            let turn_input_tokens =
                estimate_turn_input(&provider_request.messages, exact_anchor.as_ref());
            let prior_input = request.prior_session_input_tokens + run_usage.input_tokens as usize;
            let prior_output =
                request.prior_session_output_tokens + run_usage.output_tokens as usize;
            let context_window = provider.capabilities().context_length;
            let snapshot =
                budget.snapshot(context_window, turn_input_tokens, prior_input, prior_output);
            if snapshot.is_over_budget() {
                let reason = format_budget_rejection(&snapshot);
                return LlmPhaseOutcome::Failed {
                    reason,
                    assistant_message: None,
                };
            }
        }

        match provider.stream_text(&provider_request).await {
            Ok(stream) => {
                if in_overflow_episode {
                    emit_overflow_recovery(
                        emitter,
                        &RecoveryEvent::EpisodeResolved {
                            total_attempts: recovery_state.total_attempts(),
                        },
                    );
                }
                break (stream, provider_request.messages);
            }
            Err(RociError::RateLimited { retry_after_ms }) => {
                let retry_after_ms = retry_after_ms.unwrap_or(0);
                if retry_after_ms == 0 {
                    return LlmPhaseOutcome::Failed {
                        reason: "rate limited without retry_after hint".to_string(),
                        assistant_message: None,
                    };
                }
                if let Some(max_retry_delay_ms) = request.max_retry_delay_ms {
                    if max_retry_delay_ms > 0 && retry_after_ms > max_retry_delay_ms {
                        return LlmPhaseOutcome::Failed {
                            reason: format!(
                                "rate limit retry delay {retry_after_ms}ms exceeds max_retry_delay_ms={max_retry_delay_ms}"
                            ),
                            assistant_message: None,
                        };
                    }
                }
                if attempt >= max_attempts {
                    return LlmPhaseOutcome::Failed {
                        reason: format!(
                            "rate limited after {attempt} attempts; retry budget exhausted"
                        ),
                        assistant_message: None,
                    };
                }
                emit_retry_lifecycle(emitter, attempt, max_attempts, retry_after_ms, "rate_limit");
                if !sleep_with_cancellation(
                    abort_rx,
                    run_cancel_token,
                    Duration::from_millis(retry_after_ms),
                )
                .await
                {
                    return LlmPhaseOutcome::Canceled {
                        assistant_message: None,
                    };
                }
                attempt += 1;
            }
            Err(err) => {
                // -- Overflow recovery (separate from generic retry) --
                if let Some(signal) = provider.classify_overflow(&err) {
                    if !in_overflow_episode {
                        in_overflow_episode = true;
                        emit_overflow_recovery(
                            emitter,
                            &RecoveryEvent::EpisodeStarted {
                                overflow_kind: signal.kind,
                            },
                        );
                    }

                    // Process recovery decisions. When output budget reduction
                    // is not possible (no safe smaller budget), advance the
                    // ladder to the next action without retrying the provider.
                    'recovery: {
                        let mut decision = policy.next_action(&signal, &recovery_state);
                        let attempt_index = recovery_state.total_attempts();

                        if decision.action() == RecoveryAction::ReduceOutputBudget {
                            if let Some(new_max_tokens) =
                                safe_smaller_output_budget(request, &effective_settings)
                            {
                                recovery_state.record_output_reduction();
                                emit_overflow_recovery(
                                    emitter,
                                    &RecoveryEvent::ActionDecided {
                                        decision,
                                        attempt_index,
                                    },
                                );
                                effective_settings.max_tokens = Some(new_max_tokens);
                                break 'recovery; // retry provider call
                            }

                            // Output reduction was not applicable (no safe
                            // smaller budget). Record the attempt on the real
                            // state so the policy advances the ladder.
                            recovery_state.record_output_reduction();
                            decision = policy.next_action(&signal, &recovery_state);
                        }

                        match decision.action() {
                            RecoveryAction::CompactContext => {
                                emit_overflow_recovery(
                                    emitter,
                                    &RecoveryEvent::ActionDecided {
                                        decision,
                                        attempt_index,
                                    },
                                );

                                let Some(compact) = request.hooks.compaction.as_ref() else {
                                    return LlmPhaseOutcome::Failed {
                                        reason:
                                            "overflow detected but no compaction hook is configured"
                                                .to_string(),
                                        assistant_message: None,
                                    };
                                };

                                let tokens_before: usize = provider_request
                                    .messages
                                    .iter()
                                    .map(estimate_message_tokens)
                                    .sum();

                                let compaction_cancel_token = run_cancel_token.child_token();
                                let compaction_future =
                                    compact(messages.clone(), compaction_cancel_token.clone());
                                tokio::pin!(compaction_future);
                                let compaction_result = tokio::select! {
                                    _ = &mut *abort_rx => {
                                        run_cancel_token.cancel();
                                        compaction_cancel_token.cancel();
                                        return LlmPhaseOutcome::Canceled {
                                            assistant_message: None,
                                        };
                                    }
                                    _ = run_cancel_token.cancelled() => {
                                        compaction_cancel_token.cancel();
                                        return LlmPhaseOutcome::Canceled {
                                            assistant_message: None,
                                        };
                                    }
                                    result = &mut compaction_future => result,
                                };

                                match compaction_result {
                                    Ok(Some(compacted)) => {
                                        if compacted == *messages {
                                            return LlmPhaseOutcome::Failed {
                                                reason:
                                                    "overflow recovery compaction made no changes"
                                                        .to_string(),
                                                assistant_message: None,
                                            };
                                        }
                                        let next_provider_request = match build_provider_request(
                                            request,
                                            provider,
                                            tool_defs,
                                            &compacted,
                                            abort_rx,
                                            run_cancel_token,
                                            &effective_settings,
                                        )
                                        .await
                                        {
                                            Ok(req) => req,
                                            Err(outcome) => return outcome,
                                        };
                                        let progress = CompactionProgress::new(
                                            tokens_before,
                                            next_provider_request
                                                .messages
                                                .iter()
                                                .map(estimate_message_tokens)
                                                .sum(),
                                        );
                                        recovery_state.record_compaction(progress);
                                        *messages = compacted;
                                        staged_provider_request = Some(next_provider_request);

                                        if roci_debug_enabled() {
                                            tracing::debug!(
                                                run_id = %request.run_id,
                                                iteration,
                                                total_attempts = recovery_state.total_attempts(),
                                                tokens_freed = progress.tokens_freed(),
                                                "overflow recovery compacted context"
                                            );
                                        }
                                        break 'recovery; // retry provider call
                                    }
                                    Ok(None) => {
                                        return LlmPhaseOutcome::Failed {
                                            reason: "overflow recovery compaction made no changes"
                                                .to_string(),
                                            assistant_message: None,
                                        };
                                    }
                                    Err(compaction_err) => {
                                        return LlmPhaseOutcome::Failed {
                                            reason: format!(
                                                "overflow recovery compaction failed: {compaction_err}"
                                            ),
                                            assistant_message: None,
                                        };
                                    }
                                }
                            }
                            RecoveryAction::Abort => {
                                let abort_reason = decision
                                    .as_abort_reason()
                                    .unwrap_or(AbortReason::NotRecoverable);
                                emit_overflow_recovery(
                                    emitter,
                                    &RecoveryEvent::EpisodeExhausted {
                                        reason: abort_reason,
                                        total_attempts: recovery_state.total_attempts(),
                                    },
                                );
                                return LlmPhaseOutcome::Failed {
                                    reason: format_overflow_recovery_abort(
                                        &err,
                                        recovery_state.total_attempts(),
                                        abort_reason,
                                    ),
                                    assistant_message: None,
                                };
                            }
                            RecoveryAction::ReduceOutputBudget => {
                                return LlmPhaseOutcome::Failed {
                                    reason:
                                        "overflow recovery could not derive a smaller max_tokens budget"
                                            .to_string(),
                                    assistant_message: None,
                                };
                            }
                        }
                    }
                    // Labeled block broke → retry the provider call
                } else if err.is_retryable() {
                    // -- Generic retry (separate from overflow) --
                    if attempt >= max_attempts {
                        return LlmPhaseOutcome::Failed {
                            reason: format!(
                                "retryable provider error after {attempt} attempts: {err}"
                            ),
                            assistant_message: None,
                        };
                    }
                    let delay_ms = jittered_backoff_ms(
                        next_backoff_ms,
                        request.retry_backoff.jitter_ratio,
                        request.retry_backoff.max_delay_ms.max(1),
                    );
                    emit_retry_lifecycle(
                        emitter,
                        attempt,
                        max_attempts,
                        delay_ms,
                        "retryable_error",
                    );
                    if !sleep_with_cancellation(
                        abort_rx,
                        run_cancel_token,
                        Duration::from_millis(delay_ms),
                    )
                    .await
                    {
                        return LlmPhaseOutcome::Canceled {
                            assistant_message: None,
                        };
                    }
                    attempt += 1;
                    next_backoff_ms = next_backoff_ms_for_policy(next_backoff_ms, request);
                } else {
                    return LlmPhaseOutcome::Failed {
                        reason: err.to_string(),
                        assistant_message: None,
                    };
                }
            }
        }
    };

    let mut iteration_text = String::new();
    let mut tool_calls: Vec<AgentToolCall> = Vec::new();
    let mut stream_done = false;
    let mut message_open = false;
    let mut call_usage: Option<Usage> = None;
    let idle_timeout_ms = request.settings.stream_idle_timeout_ms.unwrap_or(120_000);
    let mut idle_sleep = (idle_timeout_ms > 0)
        .then(|| Box::pin(time::sleep(Duration::from_millis(idle_timeout_ms))));
    loop {
        if let Some(ref mut sleep) = idle_sleep {
            tokio::select! {
                _ = &mut *abort_rx => {
                    run_cancel_token.cancel();
                    emit_message_end_if_open(
                        agent_emitter,
                        &mut message_open,
                        &iteration_text,
                        &tool_calls,
                    );
                    finalize_call_usage(call_usage, &last_provider_messages, &iteration_text, &tool_calls, run_usage);
                    return LlmPhaseOutcome::Canceled {
                        assistant_message: assistant_snapshot_if_present(
                            &iteration_text,
                            &tool_calls,
                        ),
                    };
                }
                _ = sleep.as_mut() => {
                    emit_message_end_if_open(
                        agent_emitter,
                        &mut message_open,
                        &iteration_text,
                        &tool_calls,
                    );
                    finalize_call_usage(call_usage, &last_provider_messages, &iteration_text, &tool_calls, run_usage);
                    return LlmPhaseOutcome::Failed {
                        reason: "stream idle timeout".to_string(),
                        assistant_message: assistant_snapshot_if_present(
                            &iteration_text,
                            &tool_calls,
                        ),
                    };
                }
                delta = stream.next() => {
                    let Some(delta) = delta else { break; };
                    match delta {
                        Ok(delta) => {
                            sleep.as_mut().reset(
                                time::Instant::now() + Duration::from_millis(idle_timeout_ms),
                            );
                            if let Some(ref u) = delta.usage {
                                call_usage = Some(u.clone());
                            }
                            if let Some(reason) = process_stream_delta(
                                emitter,
                                agent_emitter,
                                delta,
                                &mut iteration_text,
                                &mut tool_calls,
                                &mut stream_done,
                                &mut message_open,
                            ) {
                                emit_message_end_if_open(
                                    agent_emitter,
                                    &mut message_open,
                                    &iteration_text,
                                    &tool_calls,
                                );
                                finalize_call_usage(call_usage, &last_provider_messages, &iteration_text, &tool_calls, run_usage);
                                return LlmPhaseOutcome::Failed {
                                    reason,
                                    assistant_message: assistant_snapshot_if_present(
                                        &iteration_text,
                                        &tool_calls,
                                    ),
                                };
                            }
                            if stream_done {
                                break;
                            }
                        }
                        Err(err) => {
                            emit_message_end_if_open(
                                agent_emitter,
                                &mut message_open,
                                &iteration_text,
                                &tool_calls,
                            );
                            finalize_call_usage(call_usage, &last_provider_messages, &iteration_text, &tool_calls, run_usage);
                            return LlmPhaseOutcome::Failed {
                                reason: err.to_string(),
                                assistant_message: assistant_snapshot_if_present(
                                    &iteration_text,
                                    &tool_calls,
                                ),
                            };
                        }
                    }
                }
            }
        } else {
            tokio::select! {
                _ = &mut *abort_rx => {
                    run_cancel_token.cancel();
                    emit_message_end_if_open(
                        agent_emitter,
                        &mut message_open,
                        &iteration_text,
                        &tool_calls,
                    );
                    finalize_call_usage(call_usage, &last_provider_messages, &iteration_text, &tool_calls, run_usage);
                    return LlmPhaseOutcome::Canceled {
                        assistant_message: assistant_snapshot_if_present(
                            &iteration_text,
                            &tool_calls,
                        ),
                    };
                }
                delta = stream.next() => {
                    let Some(delta) = delta else { break; };
                    match delta {
                        Ok(delta) => {
                            if let Some(ref u) = delta.usage {
                                call_usage = Some(u.clone());
                            }
                            if let Some(reason) = process_stream_delta(
                                emitter,
                                agent_emitter,
                                delta,
                                &mut iteration_text,
                                &mut tool_calls,
                                &mut stream_done,
                                &mut message_open,
                            ) {
                                emit_message_end_if_open(
                                    agent_emitter,
                                    &mut message_open,
                                    &iteration_text,
                                    &tool_calls,
                                );
                                finalize_call_usage(call_usage, &last_provider_messages, &iteration_text, &tool_calls, run_usage);
                                return LlmPhaseOutcome::Failed {
                                    reason,
                                    assistant_message: assistant_snapshot_if_present(
                                        &iteration_text,
                                        &tool_calls,
                                    ),
                                };
                            }
                            if stream_done {
                                break;
                            }
                        }
                        Err(err) => {
                            emit_message_end_if_open(
                                agent_emitter,
                                &mut message_open,
                                &iteration_text,
                                &tool_calls,
                            );
                            finalize_call_usage(call_usage, &last_provider_messages, &iteration_text, &tool_calls, run_usage);
                            return LlmPhaseOutcome::Failed {
                                reason: err.to_string(),
                                assistant_message: assistant_snapshot_if_present(
                                    &iteration_text,
                                    &tool_calls,
                                ),
                            };
                        }
                    }
                }
            }
        }
    }
    emit_message_end_if_open(
        agent_emitter,
        &mut message_open,
        &iteration_text,
        &tool_calls,
    );

    // Merge call usage into the run accumulator on the happy path.
    // Finalize before updating the anchor so `last_provider_messages` is
    // still available for the heuristic fallback (when call_usage is None,
    // the provider messages are used; when Some, the exact usage is used and
    // the messages param is ignored).
    finalize_call_usage(
        call_usage.clone(),
        &last_provider_messages,
        &iteration_text,
        &tool_calls,
        run_usage,
    );

    // Update the exact anchor only when the provider reported a meaningful
    // nonzero prompt count. Zero/default usage must not replace a prior
    // anchor with a fail-open zero-token prefix.
    if let Some(ref usage) = call_usage {
        if usage.input_tokens > 0 {
            *exact_anchor = Some(ExactUsageAnchor {
                provider_messages: last_provider_messages,
                prompt_tokens: usage.input_tokens as usize,
            });
        }
    }

    if roci_debug_enabled() {
        let tool_names = tool_calls
            .iter()
            .map(|call| call.name.as_str())
            .collect::<Vec<_>>()
            .join(",");
        tracing::debug!(
            run_id = %request.run_id,
            iteration,
            stream_done,
            tool_calls = tool_calls.len(),
            tool_names = %tool_names,
            text_len = iteration_text.len(),
            "roci iteration complete"
        );
    }

    LlmPhaseOutcome::Ready {
        iteration_text,
        tool_calls,
    }
}

async fn build_provider_request(
    request: &RunRequest,
    provider: &dyn provider::ModelProvider,
    tool_defs: &Option<Vec<ToolDefinition>>,
    messages: &[ModelMessage],
    abort_rx: &mut oneshot::Receiver<()>,
    run_cancel_token: &CancellationToken,
    effective_settings: &GenerationSettings,
) -> Result<ProviderRequest, LlmPhaseOutcome> {
    let mut transformed = messages.to_vec();
    if let Some(ref transform) = request.transform_context {
        let transform_cancel = run_cancel_token.child_token();
        let transform_payload = TransformContextHookPayload {
            run_id: request.run_id,
            model: request.model.clone(),
            messages: transformed.clone(),
            cancellation_token: transform_cancel.clone(),
        };
        let transform_future = transform(transform_payload);
        tokio::pin!(transform_future);
        let transform_result = tokio::select! {
            _ = &mut *abort_rx => {
                run_cancel_token.cancel();
                transform_cancel.cancel();
                return Err(LlmPhaseOutcome::Canceled {
                    assistant_message: None,
                });
            }
            _ = run_cancel_token.cancelled() => {
                transform_cancel.cancel();
                return Err(LlmPhaseOutcome::Canceled {
                    assistant_message: None,
                });
            }
            result = &mut transform_future => result,
        };

        transformed = match transform_result {
            Ok(TransformContextHookResult::Continue) => transformed,
            Ok(TransformContextHookResult::ReplaceMessages { messages }) => messages,
            Ok(TransformContextHookResult::Cancel { reason }) => {
                return Err(LlmPhaseOutcome::Failed {
                    reason: reason
                        .unwrap_or_else(|| "transform_context hook canceled LLM phase".to_string()),
                    assistant_message: None,
                });
            }
            Err(err) => {
                return Err(LlmPhaseOutcome::Failed {
                    reason: format!("transform_context hook failed: {err}"),
                    assistant_message: None,
                });
            }
        };
    }

    let llm_context = if let Some(ref convert) = request.convert_to_llm {
        let convert_cancel = run_cancel_token.child_token();
        let agent_messages: Vec<AgentMessage> = transformed
            .iter()
            .cloned()
            .map(AgentMessage::from_model)
            .collect();
        let convert_payload = ConvertToLlmHookPayload {
            run_id: request.run_id,
            model: request.model.clone(),
            messages: agent_messages.clone(),
            cancellation_token: convert_cancel.clone(),
        };
        let convert_future = convert(convert_payload);
        tokio::pin!(convert_future);
        let convert_result = tokio::select! {
            _ = &mut *abort_rx => {
                run_cancel_token.cancel();
                convert_cancel.cancel();
                return Err(LlmPhaseOutcome::Canceled {
                    assistant_message: None,
                });
            }
            _ = run_cancel_token.cancelled() => {
                convert_cancel.cancel();
                return Err(LlmPhaseOutcome::Canceled {
                    assistant_message: None,
                });
            }
            result = &mut convert_future => result,
        };

        match convert_result {
            Ok(ConvertToLlmHookResult::Continue) => convert_to_llm(&agent_messages),
            Ok(ConvertToLlmHookResult::ReplaceMessages { messages }) => messages,
            Ok(ConvertToLlmHookResult::Cancel { reason }) => {
                return Err(LlmPhaseOutcome::Failed {
                    reason: reason
                        .unwrap_or_else(|| "convert_to_llm hook canceled LLM phase".to_string()),
                    assistant_message: None,
                });
            }
            Err(err) => {
                return Err(LlmPhaseOutcome::Failed {
                    reason: format!("convert_to_llm hook failed: {err}"),
                    assistant_message: None,
                });
            }
        }
    } else {
        transformed
    };

    let provider_messages =
        provider::sanitize_messages_for_provider(&llm_context, provider.provider_name());
    Ok(ProviderRequest {
        messages: provider_messages,
        settings: effective_settings.clone(),
        tools: tool_defs.clone(),
        response_format: effective_settings.response_format.clone(),
        api_key_override: request.api_key_override.clone(),
        headers: request.provider_headers.clone(),
        metadata: request.provider_metadata.clone(),
        payload_callback: request.provider_payload_callback.clone(),
        session_id: request.session_id.clone(),
        transport: request.transport.clone(),
    })
}

/// Emit policy-driven overflow recovery progress on the system stream.
fn emit_overflow_recovery(emitter: &RunEventEmitter, event: &RecoveryEvent) {
    let message = match event {
        RecoveryEvent::EpisodeStarted { overflow_kind } => {
            format!("overflow recovery started: {overflow_kind:?}")
        }
        RecoveryEvent::ActionDecided {
            decision,
            attempt_index,
        } => {
            format!(
                "overflow recovery attempt={attempt_index} action={:?} reason={:?}",
                decision.action(),
                decision.reason()
            )
        }
        RecoveryEvent::EpisodeResolved { total_attempts } => {
            format!("overflow recovery resolved after {total_attempts} attempts")
        }
        RecoveryEvent::EpisodeExhausted {
            reason,
            total_attempts,
        } => {
            format!("overflow recovery exhausted after {total_attempts} attempts: {reason:?}")
        }
    };
    emitter.emit(RunEventStream::System, RunEventPayload::Error { message });
}

/// Derive a smaller `max_tokens` budget from the configured output reserve.
///
/// Returns `None` when the run has no context budget, the reserve is zero, or
/// the current effective budget is already at or below the reserve.
fn safe_smaller_output_budget(
    request: &RunRequest,
    effective_settings: &GenerationSettings,
) -> Option<u32> {
    let reserve = u32::try_from(request.context_budget.as_ref()?.reserve_output_tokens).ok()?;
    if reserve == 0 {
        return None;
    }
    match effective_settings.max_tokens {
        Some(current) if current <= reserve => None,
        _ => Some(reserve),
    }
}

/// Build the user-facing failure message for a terminal recovery decision.
fn format_overflow_recovery_abort(
    error: &RociError,
    total_attempts: u8,
    abort_reason: AbortReason,
) -> String {
    match abort_reason {
        AbortReason::NotRecoverable => error.to_string(),
        AbortReason::CompactionAttemptsExhausted => {
            format!("context overflow persisted after {total_attempts} attempts despite recovery")
        }
        AbortReason::CompactionProgressInsufficient => format!(
            "context overflow recovery stopped after {total_attempts} attempts because compaction made insufficient progress"
        ),
    }
}

fn emit_retry_lifecycle(
    emitter: &RunEventEmitter,
    attempt: u32,
    max_attempts: u32,
    delay_ms: u64,
    reason: &str,
) {
    emitter.emit(
        RunEventStream::System,
        RunEventPayload::Error {
            message: format!(
                "provider retry attempt={attempt}/{max_attempts} delay_ms={delay_ms} reason={reason}"
            ),
        },
    );
}

async fn sleep_with_cancellation(
    abort_rx: &mut oneshot::Receiver<()>,
    run_cancel_token: &CancellationToken,
    duration: Duration,
) -> bool {
    tokio::select! {
        _ = &mut *abort_rx => {
            run_cancel_token.cancel();
            false
        }
        _ = run_cancel_token.cancelled() => false,
        _ = time::sleep(duration) => true,
    }
}

fn next_backoff_ms_for_policy(current_backoff_ms: u64, request: &RunRequest) -> u64 {
    let multiplier = request.retry_backoff.multiplier.max(1.0);
    let max_delay_ms = request.retry_backoff.max_delay_ms.max(1);
    ((current_backoff_ms as f64 * multiplier) as u64)
        .max(1)
        .min(max_delay_ms)
}

fn jittered_backoff_ms(base_delay_ms: u64, jitter_ratio: f64, max_delay_ms: u64) -> u64 {
    let jitter_ratio = jitter_ratio.max(0.0);
    let min_factor = (1.0 - jitter_ratio).max(0.0);
    let max_factor = 1.0 + jitter_ratio;
    let factor = min_factor + (rand_factor() * (max_factor - min_factor));
    ((base_delay_ms as f64 * factor) as u64)
        .max(1)
        .min(max_delay_ms.max(1))
}

fn rand_factor() -> f64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);

    let hash = hasher.finish();
    (hash % 10_000) as f64 / 10_000.0
}

fn assistant_snapshot_if_present(
    iteration_text: &str,
    tool_calls: &[AgentToolCall],
) -> Option<ModelMessage> {
    if iteration_text.is_empty() && tool_calls.is_empty() {
        None
    } else {
        Some(assistant_message_snapshot(iteration_text, tool_calls))
    }
}

/// Estimate turn input tokens, using the exact-anchor path when available.
///
/// When the anchor's messages are a prefix of `current_messages`, we
/// re-use the provider-reported prompt token count and only heuristically
/// estimate the "tail" — messages added since the anchored call (the
/// assistant reply, tool results, new user messages, etc.).
///
/// The anchor's `completion_tokens` are NOT added here because the
/// assistant reply becomes a message in the tail and is already counted
/// by the heuristic.
///
/// Falls back to a full heuristic recount when:
/// - No anchor is available.
/// - The anchor messages are not a prefix of the current messages.
fn estimate_turn_input(
    current_messages: &[ModelMessage],
    anchor: Option<&ExactUsageAnchor>,
) -> usize {
    if let Some(anchor) = anchor {
        let prefix_len = anchor.provider_messages.len();
        if current_messages.len() >= prefix_len
            && current_messages[..prefix_len] == anchor.provider_messages[..]
        {
            // Anchor is a prefix — estimate only the tail.
            let tail_tokens: usize = current_messages[prefix_len..]
                .iter()
                .map(estimate_message_tokens)
                .sum();
            return anchor.prompt_tokens + tail_tokens;
        }
    }
    // Full heuristic fallback.
    current_messages.iter().map(estimate_message_tokens).sum()
}

/// Build a concrete rejection reason from a [`BudgetSnapshot`].
fn format_budget_rejection(snap: &crate::context::BudgetSnapshot) -> String {
    use std::fmt::Write;
    let mut reason = String::from("context budget exceeded: ");
    if snap.turn_input_used > snap.turn_input_limit {
        write!(
            reason,
            "turn input {}/{} tokens",
            snap.turn_input_used, snap.turn_input_limit,
        )
        .ok();
    }
    if let Some(limit) = snap.session_input_limit {
        if snap.projected_session_input > limit {
            if reason.len() > "context budget exceeded: ".len() {
                reason.push_str("; ");
            }
            write!(
                reason,
                "projected session input {}/{} tokens",
                snap.projected_session_input, limit,
            )
            .ok();
        }
    }
    if let Some(limit) = snap.session_output_limit {
        if snap.projected_session_output > limit {
            if reason.len() > "context budget exceeded: ".len() {
                reason.push_str("; ");
            }
            write!(
                reason,
                "projected session output {}/{} tokens",
                snap.projected_session_output, limit,
            )
            .ok();
        }
    }
    reason
}
