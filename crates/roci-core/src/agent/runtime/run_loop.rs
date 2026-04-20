use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::types::drain_queue;
use super::AgentRuntime;
use crate::agent_loop::runner::{
    AutoCompactionConfig, BeforeAgentStartHookPayload, BeforeAgentStartHookResult,
    CompactionHandler, FollowUpMessagesFn, RunHooks, SteeringMessagesFn,
};
use crate::agent_loop::{RunHandle, RunRequest, RunResult, RunStatus, Runner};
use crate::error::RociError;
use crate::tools::dynamic::{DynamicToolAdapter, DynamicToolProvider};
use crate::tools::tool::Tool;
use crate::types::ModelMessage;

impl AgentRuntime {
    pub(super) async fn resolve_tools_for_run(&self) -> Result<Vec<Arc<dyn Tool>>, RociError> {
        let static_tools = self.tools.lock().await.clone();
        let providers = self.dynamic_tool_providers.lock().await.clone();
        Self::merge_static_and_dynamic_tools(static_tools, providers).await
    }

    async fn merge_static_and_dynamic_tools(
        mut static_tools: Vec<Arc<dyn Tool>>,
        providers: Vec<Arc<dyn DynamicToolProvider>>,
    ) -> Result<Vec<Arc<dyn Tool>>, RociError> {
        for provider in providers {
            let discovered = provider.list_tools().await?;
            for tool in discovered {
                static_tools.push(Arc::new(DynamicToolAdapter::new(
                    Arc::clone(&provider),
                    tool,
                )));
            }
        }
        Ok(static_tools)
    }

    /// Build a [`RunRequest`], start the loop, wait for the result, then
    /// transition back to Idle.
    pub(super) async fn run_loop(
        &self,
        initial_messages: Vec<ModelMessage>,
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

        let intercepting_sink = self.build_intercepting_sink();

        #[cfg(feature = "agent")]
        let user_input_callback = {
            let coordinator = self.user_input_coordinator.clone();
            let ui_event_sink = intercepting_sink.clone();
            let config_timeout = self.config.user_input_timeout_ms;
            let cb: crate::tools::user_input::RequestUserInputFn =
                Arc::new(move |request: crate::tools::UserInputRequest| {
                    let coordinator = coordinator.clone();
                    let sink = ui_event_sink.clone();
                    Box::pin(async move {
                        let rx = coordinator.create_request(request.clone()).await?;
                        sink(crate::agent_loop::AgentEvent::UserInputRequested {
                            request: request.clone(),
                        });
                        let effective_timeout = request.timeout_ms.or(config_timeout);
                        rx.wait(effective_timeout).await
                    })
                });
            cb
        };

        let model = self.model.lock().await.clone();

        let tools = match self.resolve_tools_for_run().await {
            Ok(tools) => tools,
            Err(err) => {
                self.restore_idle_after_preflight_error().await;
                return Err(err);
            }
        };

        // Freeze session usage into the request before dispatching.
        let session_usage_snapshot = self.session_usage.lock().await.clone();

        let mut request = RunRequest::new(model, initial_messages)
            .with_tools(tools)
            .with_steering_messages(steering_fn)
            .with_follow_up_messages(follow_up_fn)
            .with_agent_event_sink(intercepting_sink)
            .with_prior_session_usage(
                session_usage_snapshot.input_tokens as usize,
                session_usage_snapshot.output_tokens as usize,
            );

        if let Some(ref budget) = self.config.context_budget {
            request = request.with_context_budget(budget.clone());
        }

        #[cfg(feature = "agent")]
        {
            request = request.with_user_input_callback(user_input_callback);
        }

        if let Some(hook) = self.config.before_agent_start.clone() {
            let hook_cancel_token = CancellationToken::new();
            let hook_payload = BeforeAgentStartHookPayload {
                run_id: request.run_id,
                model: request.model.clone(),
                messages: request.messages.clone(),
                cancellation_token: hook_cancel_token.clone(),
            };
            match hook(hook_payload).await {
                Ok(BeforeAgentStartHookResult::Continue) => {}
                Ok(BeforeAgentStartHookResult::ReplaceMessages { messages }) => {
                    request.messages = messages;
                }
                Ok(BeforeAgentStartHookResult::Cancel { .. }) => {
                    self.restore_idle_after_preflight_error().await;
                    return Ok(RunResult::canceled_with_messages(request.messages.clone()));
                }
                Err(err) => {
                    self.restore_idle_after_preflight_error().await;
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
            let run_model = request.model.clone();
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

        request.settings = self.config.settings.clone();

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
        request = request.with_retry_backoff(self.config.retry_backoff);
        if let Some(ref api_key_override) = self.config.api_key_override {
            request = request.with_api_key_override(api_key_override.clone());
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

        let run_result = async {
            let provider_has_config_key = self
                .roci_config
                .get_api_key(request.model.provider_name())
                .is_some();
            if request.api_key_override.is_none() && !provider_has_config_key {
                if let Some(ref get_key) = self.config.get_api_key {
                    let key = get_key().await?;
                    request = request.with_api_key_override(key);
                }
            }

            let mut handle: RunHandle = self.runner.start(request).await?;
            let abort_tx = handle.take_abort_sender();
            *self.active_abort_tx.lock().await = abort_tx;

            Ok::<RunResult, RociError>(handle.wait().await)
        }
        .await;

        self.active_abort_tx.lock().await.take();
        *self.is_streaming.lock().await = false;

        match &run_result {
            Ok(result) => {
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

        run_result
    }
}
