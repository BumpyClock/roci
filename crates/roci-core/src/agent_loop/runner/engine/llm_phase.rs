use futures::StreamExt;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{self, Duration};
use tokio_util::sync::CancellationToken;

use super::super::super::compaction::estimate_context_usage;
use super::super::control::{
    debug_enabled, process_stream_delta, AgentEventEmitter, RunEventEmitter,
};
use super::super::message_events::{emit_message_end_if_open, emit_message_lifecycle};
use super::super::RunRequest;
use crate::agent::message::AgentMessage;
use crate::error::RociError;
use crate::provider::{self, ProviderRequest, ToolDefinition};
use crate::types::{AgentToolCall, ModelMessage};

pub(super) enum LlmPhaseOutcome {
    Ready {
        iteration_text: String,
        tool_calls: Vec<AgentToolCall>,
    },
    Canceled,
    Failed(String),
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
            return LlmPhaseOutcome::Failed(
                "auto-compaction is enabled but no compaction hook is configured".to_string(),
            );
        };
        let compaction_cancel_token = run_cancel_token.child_token();
        let compaction_future = compact(messages.clone(), compaction_cancel_token.clone());
        tokio::pin!(compaction_future);
        let compaction_result = tokio::select! {
            _ = &mut *abort_rx => {
                run_cancel_token.cancel();
                compaction_cancel_token.cancel();
                return LlmPhaseOutcome::Canceled;
            }
            result = &mut compaction_future => result,
        };
        match compaction_result {
            Ok(Some(compacted)) => {
                *messages = compacted;
            }
            Ok(None) => {}
            Err(err) => {
                return LlmPhaseOutcome::Failed(format!("compaction failed: {err}"));
            }
        }
    }

    let llm_context = if let Some(ref convert) = request.convert_to_llm {
        let agent_messages: Vec<AgentMessage> = messages
            .iter()
            .cloned()
            .map(AgentMessage::from_model)
            .collect();
        convert(agent_messages).await
    } else {
        messages.clone()
    };
    let transformed = if let Some(ref transform) = request.transform_context {
        transform(llm_context).await
    } else {
        llm_context
    };
    let provider_messages =
        provider::sanitize_messages_for_provider(&transformed, provider.provider_name());
    let req = ProviderRequest {
        messages: provider_messages,
        settings: request.settings.clone(),
        tools: tool_defs.clone(),
        response_format: request.settings.response_format.clone(),
        session_id: request.session_id.clone(),
        transport: request.transport.clone(),
    };

    let mut stream = loop {
        match provider.stream_text(&req).await {
            Ok(stream) => break stream,
            Err(RociError::RateLimited { retry_after_ms }) => {
                let retry_after_ms = retry_after_ms.unwrap_or(0);
                if retry_after_ms == 0 {
                    return LlmPhaseOutcome::Failed(
                        "rate limited without retry_after hint".to_string(),
                    );
                }
                if let Some(max_retry_delay_ms) = request.max_retry_delay_ms {
                    if max_retry_delay_ms > 0 && retry_after_ms > max_retry_delay_ms {
                        return LlmPhaseOutcome::Failed(format!(
                            "rate limit retry delay {retry_after_ms}ms exceeds max_retry_delay_ms={max_retry_delay_ms}"
                        ));
                    }
                }
                tokio::select! {
                    _ = &mut *abort_rx => {
                        run_cancel_token.cancel();
                        return LlmPhaseOutcome::Canceled;
                    }
                    _ = time::sleep(Duration::from_millis(retry_after_ms)) => {}
                }
            }
            Err(err) => {
                return LlmPhaseOutcome::Failed(err.to_string());
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
                    return LlmPhaseOutcome::Canceled;
                }
                _ = sleep.as_mut() => {
                    emit_message_end_if_open(
                        agent_emitter,
                        &mut message_open,
                        &iteration_text,
                        &tool_calls,
                    );
                    return LlmPhaseOutcome::Failed("stream idle timeout".to_string());
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
                                return LlmPhaseOutcome::Failed(reason);
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
                            return LlmPhaseOutcome::Failed(err.to_string());
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
                    return LlmPhaseOutcome::Canceled;
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
                                return LlmPhaseOutcome::Failed(reason);
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
                            return LlmPhaseOutcome::Failed(err.to_string());
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
