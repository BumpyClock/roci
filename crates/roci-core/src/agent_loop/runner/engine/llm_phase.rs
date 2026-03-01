use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;

use super::super::super::compaction::estimate_context_usage;
use super::super::control::{
    debug_enabled, process_stream_delta, AgentEventEmitter, RunEventEmitter,
};
use super::super::message_events::{
    assistant_message_snapshot, emit_message_end_if_open, emit_message_lifecycle,
};
use super::super::{
    ConvertToLlmHookPayload, ConvertToLlmHookResult, RunEventPayload, RunEventStream, RunRequest,
    TransformContextHookPayload, TransformContextHookResult,
};
use crate::agent::message::{convert_to_llm, AgentMessage};
use crate::error::{ErrorCode, RociError};
use crate::provider::{self, ProviderRequest, ToolDefinition};
use crate::types::{AgentToolCall, ModelMessage};

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
    let mut overflow_recovery_attempts: u32 = 0;

    let mut stream = loop {
        let provider_request = match build_provider_request(
            request,
            provider,
            tool_defs,
            messages,
            abort_rx,
            run_cancel_token,
        )
        .await
        {
            Ok(req) => req,
            Err(outcome) => return outcome,
        };

        match provider.stream_text(&provider_request).await {
            Ok(stream) => break stream,
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
            Err(err) if is_typed_overflow_error(&err) => {
                if attempt >= max_attempts {
                    return LlmPhaseOutcome::Failed {
                        reason: format!(
                            "context overflow persisted after {attempt} attempts despite recovery"
                        ),
                        assistant_message: None,
                    };
                }
                let Some(compact) = request.hooks.compaction.as_ref() else {
                    return LlmPhaseOutcome::Failed {
                        reason: "context overflow detected but no compaction hook is configured"
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
                        *messages = compacted;
                        overflow_recovery_attempts += 1;
                        attempt += 1;
                        emit_retry_lifecycle(
                            emitter,
                            attempt - 1,
                            max_attempts,
                            0,
                            "overflow_compaction",
                        );
                        if debug_enabled() {
                            tracing::debug!(
                                run_id = %request.run_id,
                                iteration,
                                overflow_recovery_attempts,
                                "typed overflow recovery compacted context"
                            );
                        }
                    }
                    Ok(None) => {
                        return LlmPhaseOutcome::Failed {
                            reason: "context overflow recovery compaction made no changes"
                                .to_string(),
                            assistant_message: None,
                        };
                    }
                    Err(compaction_error) => {
                        return LlmPhaseOutcome::Failed {
                            reason: format!(
                                "context overflow recovery compaction failed: {compaction_error}"
                            ),
                            assistant_message: None,
                        };
                    }
                }
            }
            Err(err) if err.is_retryable() => {
                if attempt >= max_attempts {
                    return LlmPhaseOutcome::Failed {
                        reason: format!("retryable provider error after {attempt} attempts: {err}"),
                        assistant_message: None,
                    };
                }
                let delay_ms = jittered_backoff_ms(
                    next_backoff_ms,
                    request.retry_backoff.jitter_ratio,
                    request.retry_backoff.max_delay_ms.max(1),
                );
                emit_retry_lifecycle(emitter, attempt, max_attempts, delay_ms, "retryable_error");
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
            }
            Err(err) => {
                return LlmPhaseOutcome::Failed {
                    reason: err.to_string(),
                    assistant_message: None,
                };
            }
        }
    };

    let mut iteration_text = String::new();
    let mut tool_calls: Vec<AgentToolCall> = Vec::new();
    let mut stream_done = false;
    let mut message_open = false;
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

    if debug_enabled() {
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
        settings: request.settings.clone(),
        tools: tool_defs.clone(),
        response_format: request.settings.response_format.clone(),
        api_key_override: request.api_key_override.clone(),
        headers: request.provider_headers.clone(),
        metadata: request.provider_metadata.clone(),
        payload_callback: request.provider_payload_callback.clone(),
        session_id: request.session_id.clone(),
        transport: request.transport.clone(),
    })
}

fn is_typed_overflow_error(error: &RociError) -> bool {
    matches!(
        error,
        RociError::Api {
            details: Some(details),
            ..
        } if details.code == Some(ErrorCode::ContextLengthExceeded)
    )
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
