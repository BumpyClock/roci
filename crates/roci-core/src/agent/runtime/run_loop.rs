use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::types::drain_queue;
use super::{AgentRuntime, AgentRuntimeError, CollaborationMode, ThreadId, TurnId, TurnStatus};
use crate::agent_loop::events::RunEventPayload;
use crate::agent_loop::runner::{
    AutoCompactionConfig, BeforeAgentStartHookPayload, BeforeAgentStartHookResult,
    CompactionHandler, FollowUpMessagesFn, RunEventSink, RunHooks, SteeringMessagesFn,
};
use crate::agent_loop::ApprovalPolicy;
use crate::agent_loop::{RunHandle, RunRequest, RunResult, RunStatus, Runner};
use crate::error::RociError;
use crate::models::{ModelCandidates, ModelHealthTracker};
use crate::tools::catalog::{ToolCatalog, ToolOrigin};
use crate::tools::dynamic::{DynamicToolAdapter, DynamicToolProvider};
use crate::tools::tool::Tool;
use crate::types::{
    GenerationSettings, ModelMessage, OpenAiResponsesOptions, ResponseFormat, Role,
};

#[derive(Debug, Clone)]
pub(super) struct TurnRunOptions {
    pub settings: GenerationSettings,
    pub approval_policy: ApprovalPolicy,
    pub collaboration_mode: CollaborationMode,
}

impl AgentRuntime {
    async fn complete_chat_turn(&self, turn_id: TurnId) -> Result<(), RociError> {
        self.terminal_chat_turn(turn_id, TurnStatus::Completed, None)
            .await
    }

    async fn fail_chat_turn(&self, turn_id: TurnId, error: String) -> Result<(), RociError> {
        self.terminal_chat_turn(turn_id, TurnStatus::Failed, Some(error))
            .await
    }

    async fn cancel_chat_turn(&self, turn_id: TurnId) -> Result<(), RociError> {
        self.terminal_chat_turn(turn_id, TurnStatus::Canceled, None)
            .await
    }

    async fn terminal_chat_turn(
        &self,
        turn_id: TurnId,
        status: TurnStatus,
        error: Option<String>,
    ) -> Result<(), RociError> {
        let events: Result<Vec<_>, AgentRuntimeError> = (|| {
            let mut projector =
                self.chat_projector
                    .lock()
                    .map_err(|_| AgentRuntimeError::ProjectionFailed {
                        message: "chat projector lock poisoned".into(),
                    })?;
            let mut events = if status == TurnStatus::Canceled {
                projector.cancel_pending_approvals(turn_id)
            } else {
                Ok(Vec::new())
            }?;
            if status == TurnStatus::Canceled {
                events.extend(projector.cancel_pending_human_interactions(turn_id)?);
            }
            let event = match status {
                TurnStatus::Completed => projector.complete_turn(turn_id),
                TurnStatus::Failed => projector.fail_turn(
                    turn_id,
                    error.expect("failed terminal projection carries error"),
                ),
                TurnStatus::Canceled => projector.cancel_turn(turn_id),
                TurnStatus::Queued | TurnStatus::Running => {
                    Err(AgentRuntimeError::ProjectionFailed {
                        message: format!("non-terminal status requested: {status:?}"),
                    })
                }
            };
            events.push(event?);
            Ok::<_, AgentRuntimeError>(events)
        })();

        let events = match events {
            Ok(events) => events,
            Err(AgentRuntimeError::AlreadyTerminal {
                status: terminal_status,
                ..
            }) if terminal_status == status => return Ok(()),
            Err(err) => return Err(Self::map_chat_projection_error(err)),
        };

        self.publish_runtime_events(events)
            .await
            .map(|_| ())
            .map_err(Self::map_chat_projection_error)
    }

    pub(super) fn chat_turn_status(&self, turn_id: TurnId) -> Result<TurnStatus, RociError> {
        self.chat_projector
            .lock()
            .map_err(|_| RociError::InvalidState("chat projector lock poisoned".into()))?
            .turn_snapshot(turn_id)
            .map(|turn| turn.status)
            .map_err(Self::map_chat_projection_error)
    }

    pub(super) async fn resolve_tools_for_run(&self) -> Result<Vec<Arc<dyn Tool>>, RociError> {
        let static_tools = self.tools.lock().await.clone();
        let providers = self.dynamic_tool_providers.lock().await.clone();
        let catalog = Self::merge_static_and_dynamic_tools(static_tools, providers).await?;
        Ok(catalog.resolve(&self.config.tool_visibility_policy))
    }

    async fn merge_static_and_dynamic_tools(
        static_tools: Vec<Arc<dyn Tool>>,
        providers: Vec<Arc<dyn DynamicToolProvider>>,
    ) -> Result<ToolCatalog, RociError> {
        let mut catalog = ToolCatalog::from_tools(static_tools, ToolOrigin::Custom);
        for provider in providers {
            let discovered = provider.list_tools().await?;
            for tool in discovered {
                catalog.insert_first_wins(
                    Arc::new(DynamicToolAdapter::new(Arc::clone(&provider), tool)),
                    ToolOrigin::Dynamic,
                );
            }
        }
        Ok(catalog)
    }

    pub(super) async fn current_turn_options(
        &self,
        generation_settings: Option<GenerationSettings>,
        approval_policy: Option<ApprovalPolicy>,
        collaboration_mode: Option<CollaborationMode>,
    ) -> TurnRunOptions {
        let mode = collaboration_mode.unwrap_or(CollaborationMode::Code);
        let mut settings = match generation_settings {
            Some(settings) => settings,
            None => self.generation_settings.lock().await.clone(),
        };
        if mode == CollaborationMode::Plan {
            settings = plan_mode_settings(settings);
        }
        let default_approval_policy = *self.approval_policy.lock().await;
        TurnRunOptions {
            settings,
            approval_policy: approval_policy.unwrap_or(default_approval_policy),
            collaboration_mode: mode,
        }
    }

    /// Build a [`RunRequest`], start the loop, wait for the result, then
    /// transition back to Idle.
    pub(super) async fn run_loop(
        &self,
        initial_messages: Vec<ModelMessage>,
        turn_id: TurnId,
        options: TurnRunOptions,
    ) -> Result<RunResult, RociError> {
        *self.is_streaming.lock().await = true;
        *self.last_error.lock().await = None;
        self.broadcast_snapshot().await;

        let steering_queue = self.steering_queue.clone();
        let follow_up_queue = self.follow_up_queue.clone();
        let steering_mode = self.config.steering_mode;
        let follow_up_mode = self.config.follow_up_mode;

        let steering_fn: SteeringMessagesFn = Arc::new(move || {
            let queue = steering_queue.clone();
            Box::pin(async move {
                let mut queue = queue.lock().await;
                drain_queue(&mut queue, steering_mode)
            })
        });

        let follow_up_fn: FollowUpMessagesFn = {
            let queue = follow_up_queue.clone();
            Arc::new(move || {
                let queue = queue.clone();
                Box::pin(async move {
                    let mut queue = queue.lock().await;
                    drain_queue(&mut queue, follow_up_mode)
                })
            })
        };

        let (intercepting_sink, chat_projection_error) = self.build_intercepting_sink(
            turn_id,
            initial_messages.len(),
            options.collaboration_mode,
        );
        let retry_event_sink = self.build_retry_event_sink(turn_id, chat_projection_error.clone());

        if self.chat_turn_status(turn_id)? == TurnStatus::Canceled {
            *self.is_streaming.lock().await = false;
            self.restore_idle_after_preflight_error().await;
            return Ok(RunResult::canceled_with_messages(initial_messages));
        }

        #[cfg(feature = "agent")]
        let user_input_callback = {
            let coordinator = self.human_interaction_coordinator.clone();
            let ui_event_sink = intercepting_sink.clone();
            let config_timeout = self.config.user_input_timeout_ms;
            let cb: crate::tools::user_input::RequestUserInputFn = Arc::new(
                move |request: crate::tools::UserInputRequest| {
                    let coordinator = coordinator.clone();
                    let sink = ui_event_sink.clone();
                    Box::pin(async move {
                        let human_request =
                            crate::human_interaction::HumanInteractionRequest::from_user_input(
                                request.clone(),
                            );
                        let rx = coordinator
                            .create_request(human_request.clone())
                            .await
                            .map_err(crate::tools::UserInputError::from)?;
                        sink(crate::agent_loop::AgentEvent::HumanInteractionRequested {
                            request: human_request,
                        });
                        let effective_timeout = request.timeout_ms.or(config_timeout);
                        match rx.wait_user_input(effective_timeout).await {
                            Ok(response) => {
                                sink(crate::agent_loop::AgentEvent::HumanInteractionResolved {
                                    response:
                                        crate::human_interaction::HumanInteractionResponse::from_user_input(
                                            response.clone(),
                                        ),
                                });
                                Ok(response)
                            }
                            Err(error) => {
                                sink(crate::agent_loop::AgentEvent::HumanInteractionCanceled {
                                    request_id: request.request_id,
                                    reason: Some(error.to_string()),
                                });
                                Err(error)
                            }
                        }
                    })
                },
            );
            cb
        };

        let candidates = self.candidates.lock().await.clone();
        let candidates = ModelCandidates::new(candidates)?;
        let primary_model = candidates.primary().clone();

        let tools = match self.resolve_tools_for_run().await {
            Ok(tools) => tools,
            Err(err) => {
                let projection_result = self.fail_chat_turn(turn_id, err.to_string()).await;
                self.restore_idle_after_preflight_error().await;
                projection_result?;
                return Err(err);
            }
        };

        // Freeze session usage into the request before dispatching.
        let session_usage_snapshot = self.session_usage.lock().await.clone();

        let mut request = RunRequest::with_candidates(candidates.into_vec(), initial_messages)?
            .with_tools(tools)
            .with_steering_messages(steering_fn)
            .with_follow_up_messages(follow_up_fn)
            .with_approval_policy(options.approval_policy)
            .with_agent_event_sink(intercepting_sink)
            .with_model_health_tracker(ModelHealthTracker::new_session(
                self.config.model_health.clone(),
            ))
            .with_prior_session_usage(
                session_usage_snapshot.input_tokens as usize,
                session_usage_snapshot.output_tokens as usize,
            );
        if let Some(ref approval_handler) = self.config.approval_handler {
            request = request.with_approval_handler(approval_handler.clone());
        }
        if let (Some(session_config), Some(session_fs)) = (&self.session_config, &self.session_fs) {
            let session_fs: Arc<dyn crate::session::SessionFs + Send + Sync> = session_fs.clone();
            request = request.with_session_context(session_fs, session_config.cwd.clone());
        }
        if let Some(sandbox_provider) = &self.sandbox_provider {
            request = request.with_sandbox_provider(sandbox_provider.clone());
        }

        if let Some(ref budget) = self.config.context_budget {
            request = request.with_context_budget(budget.clone());
        }

        #[cfg(feature = "agent")]
        {
            request = request
                .with_user_input_callback(user_input_callback)
                .with_human_interaction_coordinator(self.human_interaction_coordinator.clone())
                .with_tool_permission_session_approvals(
                    self.tool_permission_session_approvals.clone(),
                );
        }

        if let Some(hook) = self.config.before_agent_start.clone() {
            let hook_cancel_token = CancellationToken::new();
            let hook_payload = BeforeAgentStartHookPayload {
                run_id: request.run_id,
                model: request.active_model().clone(),
                messages: request.messages.clone(),
                cancellation_token: hook_cancel_token.clone(),
            };
            match hook(hook_payload).await {
                Ok(BeforeAgentStartHookResult::Continue) => {}
                Ok(BeforeAgentStartHookResult::ReplaceMessages { messages }) => {
                    request.messages = messages;
                }
                Ok(BeforeAgentStartHookResult::Cancel { .. }) => {
                    let projection_result = self.cancel_chat_turn(turn_id).await;
                    self.restore_idle_after_preflight_error().await;
                    projection_result?;
                    return Ok(RunResult::canceled_with_messages(request.messages.clone()));
                }
                Err(err) => {
                    let projection_result = self.fail_chat_turn(turn_id, err.to_string()).await;
                    self.restore_idle_after_preflight_error().await;
                    projection_result?;
                    return Err(RociError::InvalidState(format!(
                        "before_agent_start hook failed: {err}"
                    )));
                }
            }
            hook_cancel_token.cancel();
        }

        let mut run_hooks = RunHooks {
            compaction: None,
            pre_tool_use: self.config.pre_tool_use.clone(),
            post_tool_use: self.config.post_tool_use.clone(),
        };

        if self.config.compaction.enabled {
            let compaction_settings = self.config.compaction.clone();
            let session_before_compact = self.config.session_before_compact.clone();
            let registry = self.registry.clone();
            let roci_config = self.roci_config.clone();
            let run_model = primary_model.clone();
            let compaction_hook: CompactionHandler = Arc::new(move |messages, _cancel| {
                let compaction_settings = compaction_settings.clone();
                let session_before_compact = session_before_compact.clone();
                let registry = registry.clone();
                let roci_config = roci_config.clone();
                let run_model = run_model.clone();
                Box::pin(async move {
                    AgentRuntime::compact_messages_with_model(
                        messages,
                        &run_model,
                        &compaction_settings,
                        session_before_compact.as_ref(),
                        &registry,
                        &roci_config,
                    )
                    .await
                })
            });
            run_hooks.compaction = Some(compaction_hook);
            request = request.with_auto_compaction(AutoCompactionConfig {
                reserve_tokens: self.config.compaction.reserve_tokens,
            });
        }
        request = request.with_hooks(run_hooks);

        request.settings = options.settings.clone();

        if let Some(ref transform) = self.config.transform_context {
            request = request.with_transform_context(transform.clone());
        }
        if let Some(ref convert) = self.config.convert_to_llm {
            request = request.with_convert_to_llm(convert.clone());
        }
        if let Some(ref id) = self.config.session_id {
            request = request.with_session_id(id.clone());
        }
        if let Some(ref transport) = self.config.transport {
            request = request.with_transport(transport.clone());
        }
        if let Some(max_retry_delay_ms) = self.config.max_retry_delay_ms {
            request = request.with_max_retry_delay_ms(max_retry_delay_ms);
        }
        request = request
            .with_retry_backoff(self.config.retry_backoff)
            .with_retry_mode(self.config.effective_retry_mode()?);
        request = request.with_event_sink(retry_event_sink);
        if let Some(ref api_key_override) = self.config.api_key_override {
            request = request.with_api_key_override(api_key_override.clone());
        }
        if let Some(ref get_api_key) = self.config.get_api_key {
            request = request.with_get_api_key(get_api_key.clone());
        }
        if !self.config.provider_headers.is_empty() {
            request = request.with_provider_headers(self.config.provider_headers.clone());
        }
        if !self.config.provider_metadata.is_empty() {
            request = request.with_provider_metadata(self.config.provider_metadata.clone());
        }
        if let Some(ref callback) = self.config.provider_payload_callback {
            request = request.with_provider_payload_callback(callback.clone());
        }

        if self.chat_turn_status(turn_id)? == TurnStatus::Canceled {
            *self.is_streaming.lock().await = false;
            self.restore_idle_after_preflight_error().await;
            return Ok(RunResult::canceled_with_messages(request.messages.clone()));
        }

        let run_result = async {
            let active_model = request.active_model().clone();
            let active_provider = active_model.provider_name().to_string();
            if request.active_api_key_override().is_none()
                && self.roci_config.get_api_key(&active_provider).is_none()
            {
                if let Some(ref get_key) = request.get_api_key {
                    let key = get_key(active_model).await?;
                    request.api_key_overrides.insert(active_provider, key);
                }
            }

            if self.chat_turn_status(turn_id)? == TurnStatus::Canceled {
                return Ok(RunResult::canceled_with_messages(request.messages.clone()));
            }

            let mut handle: RunHandle = self.runner.start(request).await?;
            let abort_tx = handle.take_abort_sender();
            *self.active_abort_tx.lock().await = abort_tx;
            if self.chat_turn_status(turn_id)? == TurnStatus::Canceled {
                self.abort_active_provider_call().await;
            }

            Ok::<RunResult, RociError>(handle.wait().await)
        }
        .await;

        self.active_abort_tx.lock().await.take();
        *self.is_streaming.lock().await = false;

        let mut plan_contract_error = None;
        let projection_result = match &run_result {
            Ok(result) => match result.status {
                RunStatus::Completed => {
                    if options.collaboration_mode == CollaborationMode::Plan {
                        match self
                            .project_structured_plan(turn_id, &result.messages)
                            .await
                        {
                            Ok(()) => {
                                match self
                                    .persist_provider_ledger_messages(
                                        turn_id.thread_id(),
                                        &result.messages,
                                    )
                                    .await
                                {
                                    Ok(()) => self.complete_chat_turn(turn_id).await,
                                    Err(err) => Err(err),
                                }
                            }
                            Err(err) => {
                                let message = err.to_string();
                                plan_contract_error = Some(message.clone());
                                self.fail_chat_turn(turn_id, message).await
                            }
                        }
                    } else {
                        match self
                            .persist_provider_ledger_messages(turn_id.thread_id(), &result.messages)
                            .await
                        {
                            Ok(()) => self.complete_chat_turn(turn_id).await,
                            Err(err) => Err(err),
                        }
                    }
                }
                RunStatus::Failed => {
                    self.fail_chat_turn(
                        turn_id,
                        result
                            .error
                            .clone()
                            .unwrap_or_else(|| "run failed".to_string()),
                    )
                    .await
                }
                RunStatus::Canceled => self.cancel_chat_turn(turn_id).await,
                RunStatus::Running => Ok(()),
            },
            Err(err) => self.fail_chat_turn(turn_id, err.to_string()).await,
        };

        let projection_succeeded = projection_result.is_ok();
        match &run_result {
            Ok(result)
                if projection_succeeded
                    && plan_contract_error.is_none()
                    && result.status != RunStatus::Canceled =>
            {
                *self.messages.lock().await = result.messages.clone();
                if result.status == RunStatus::Failed {
                    *self.last_error.lock().await = result.error.clone();
                } else {
                    *self.last_error.lock().await = None;
                }
                // Merge run usage delta into persistent session ledger.
                if let Some(ref delta) = result.usage_delta {
                    self.session_usage.lock().await.merge(delta);
                }
            }
            Ok(result) if plan_contract_error.is_none() && result.status == RunStatus::Canceled => {
                *self.last_error.lock().await = None;
            }
            Ok(_) if projection_succeeded => {
                *self.last_error.lock().await = plan_contract_error.clone();
            }
            Ok(_) => {
                *self.last_error.lock().await =
                    Some("run finished after semantic turn was already terminal".to_string());
            }
            Err(err) => {
                *self.last_error.lock().await = Some(err.to_string());
            }
        }

        let mut state = self.state.lock().await;
        *state = super::AgentState::Idle;
        let _ = self.state_tx.send(super::AgentState::Idle);
        drop(state);
        self.broadcast_snapshot().await;
        self.idle_notify.notify_waiters();

        projection_result?;
        if let Some(error) = plan_contract_error {
            return Err(RociError::InvalidState(error));
        }
        if let Some(err) = chat_projection_error
            .lock()
            .map_err(|_| RociError::InvalidState("chat projection lock poisoned".into()))?
            .take()
        {
            match err {
                AgentRuntimeError::AlreadyTerminal {
                    turn_id: terminal_turn_id,
                    status: TurnStatus::Canceled,
                } if terminal_turn_id == turn_id => {}
                err => return Err(Self::map_chat_projection_error(err)),
            }
        }
        run_result
    }

    fn build_retry_event_sink(
        &self,
        turn_id: TurnId,
        projection_error: std::sync::Arc<std::sync::Mutex<Option<AgentRuntimeError>>>,
    ) -> RunEventSink {
        let chat_projector = self.chat_projector.clone();
        let runtime_event_publish_tx = self.runtime_event_publish_tx.clone();
        let runtime_event_send_lock = self.runtime_event_send_lock.clone();
        std::sync::Arc::new(move |event| {
            let RunEventPayload::Retry { event } = event.payload else {
                return;
            };
            let projection_result = chat_projector
                .lock()
                .map_err(|_| AgentRuntimeError::ProjectionFailed {
                    message: "chat projector lock poisoned".into(),
                })
                .and_then(|mut projector| projector.record_retry(turn_id, event))
                .and_then(|event| {
                    AgentRuntime::queue_runtime_event_to(
                        &runtime_event_publish_tx,
                        &runtime_event_send_lock,
                        event,
                        projection_error.clone(),
                    )
                });
            if let Err(err) = projection_result {
                if let Ok(mut stored_error) = projection_error.lock() {
                    if stored_error.is_none() {
                        *stored_error = Some(err);
                    }
                }
            }
        })
    }

    async fn project_structured_plan(
        &self,
        turn_id: TurnId,
        messages: &[ModelMessage],
    ) -> Result<(), RociError> {
        let Some(plan) = messages
            .iter()
            .rev()
            .find(|message| message.role == Role::Assistant)
            .and_then(|message| parse_structured_plan(&message.text()))
        else {
            return Err(RociError::InvalidState(
                "plan mode response did not match structured plan contract".into(),
            ));
        };

        let events = {
            let mut projector = self
                .chat_projector
                .lock()
                .map_err(|_| RociError::InvalidState("chat projector lock poisoned".into()))?;
            super::events::project_plan_update_and_mirror(
                &mut projector,
                turn_id,
                plan,
                self.session_resources.as_deref(),
            )
        }
        .map_err(Self::map_chat_projection_error)?;

        self.publish_runtime_events(events)
            .await
            .map(|_| ())
            .map_err(Self::map_chat_projection_error)
    }

    async fn persist_provider_ledger_messages(
        &self,
        thread_id: ThreadId,
        messages: &[ModelMessage],
    ) -> Result<(), RociError> {
        let Some(ledger) = &self.provider_ledger else {
            return Ok(());
        };
        let mut persisted = self.persisted_provider_message_count.lock().await;
        let needs_compaction = *persisted > messages.len()
            || self
                .messages
                .lock()
                .await
                .get(..*persisted)
                .is_some_and(|current| current != messages.get(..*persisted).unwrap_or(&[]));
        if needs_compaction {
            ledger
                .append_compacted(thread_id, messages.to_vec())
                .map_err(|err| RociError::InvalidState(err.to_string()))?;
            *persisted = messages.len();
            return Ok(());
        }
        for message in &messages[*persisted..] {
            ledger
                .append_message(thread_id, message.clone())
                .map_err(|err| RociError::InvalidState(err.to_string()))?;
        }
        *persisted = messages.len();
        Ok(())
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StructuredPlan {
    plan: Option<String>,
    steps: Option<Vec<String>>,
}

fn parse_structured_plan(text: &str) -> Option<String> {
    let structured = serde_json::from_str::<StructuredPlan>(text).ok()?;
    if structured
        .plan
        .as_ref()
        .is_some_and(|plan| plan.trim().is_empty())
    {
        return None;
    }
    if structured
        .steps
        .as_ref()
        .is_some_and(|steps| steps.is_empty() || steps.iter().any(|step| step.trim().is_empty()))
    {
        return None;
    }
    if let Some(plan) = structured.plan {
        return Some(plan);
    }
    let steps = structured.steps?;
    Some(
        steps
            .iter()
            .enumerate()
            .map(|(index, step)| format!("{}. {}", index + 1, step))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn plan_mode_settings(mut settings: GenerationSettings) -> GenerationSettings {
    settings.response_format = Some(ResponseFormat::JsonSchema {
        name: "roci_plan".to_string(),
        schema: serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "plan": {
                    "type": "string",
                    "minLength": 1,
                    "pattern": "\\S"
                },
                "steps": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "string",
                        "minLength": 1,
                        "pattern": "\\S"
                    }
                }
            },
            "anyOf": [
                { "required": ["plan"] },
                { "required": ["steps"] }
            ]
        }),
    });
    let mut openai = settings.openai_responses.unwrap_or_default();
    let instruction = "Return only JSON matching the roci_plan schema. Do not include prose outside the JSON object.";
    openai.instructions = Some(match openai.instructions {
        Some(existing) if !existing.is_empty() => format!("{existing}\n\n{instruction}"),
        _ => instruction.to_string(),
    });
    settings.openai_responses = Some(OpenAiResponsesOptions {
        instructions: openai.instructions,
        ..openai
    });
    settings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_plan_rejects_unknown_fields() {
        assert!(parse_structured_plan(r#"{"plan":"inspect","extra":"ignored?"}"#).is_none());
    }

    #[test]
    fn structured_plan_rejects_blank_or_empty_values() {
        assert!(parse_structured_plan(r#"{"plan":"   "}"#).is_none());
        assert!(parse_structured_plan(r#"{"steps":[]}"#).is_none());
        assert!(parse_structured_plan(r#"{"steps":["inspect","  "]}"#).is_none());
    }

    #[test]
    fn plan_mode_schema_matches_parser_constraints() {
        let settings = plan_mode_settings(GenerationSettings::default());
        let Some(ResponseFormat::JsonSchema { schema, .. }) = settings.response_format else {
            panic!("plan mode should request JSON schema output");
        };

        assert_eq!(schema["additionalProperties"], false);
        assert_eq!(schema["properties"]["plan"]["minLength"], 1);
        assert_eq!(schema["properties"]["plan"]["pattern"], "\\S");
        assert_eq!(schema["properties"]["steps"]["minItems"], 1);
        assert_eq!(schema["properties"]["steps"]["items"]["minLength"], 1);
        assert_eq!(schema["properties"]["steps"]["items"]["pattern"], "\\S");
    }
}
