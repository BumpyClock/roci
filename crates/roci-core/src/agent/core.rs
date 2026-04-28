//! Core Agent struct with execute/stream capabilities.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::stream::BoxStream;
use futures::{Stream, StreamExt};
use tokio::sync::oneshot;

use crate::agent_loop::{
    ApprovalHandler, ApprovalPolicy, LoopRunner, RunEvent, RunEventPayload, RunEventSink,
    RunLifecycle, RunRequest, RunStatus, Runner,
};
use crate::config::RociConfig;
use crate::error::RociError;
use crate::models::LanguageModel;
use crate::provider::ProviderRegistry;
use crate::tools::tool::Tool;
use crate::types::*;

use super::conversation::Conversation;

/// An AI agent that maintains conversation state and can use tools.
pub struct Agent {
    model: LanguageModel,
    config: RociConfig,
    registry: Arc<ProviderRegistry>,
    system_prompt: Option<String>,
    tools: Vec<Arc<dyn Tool>>,
    approval_policy: ApprovalPolicy,
    approval_handler: Option<ApprovalHandler>,
    settings: GenerationSettings,
    conversation: Conversation,
}

impl Agent {
    /// Create a new agent with an explicit provider registry.
    pub fn new(model: LanguageModel, registry: Arc<ProviderRegistry>) -> Self {
        Self {
            model,
            config: RociConfig::from_env(),
            registry,
            system_prompt: None,
            tools: Vec::new(),
            approval_policy: ApprovalPolicy::Ask,
            approval_handler: None,
            settings: GenerationSettings::default(),
            conversation: Conversation::new(),
        }
    }

    /// Set system prompt.
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Set config.
    pub fn with_config(mut self, config: RociConfig) -> Self {
        self.config = config;
        self
    }

    /// Add a tool.
    pub fn with_tool(mut self, tool: Box<dyn Tool>) -> Self {
        self.tools.push(Arc::from(tool));
        self
    }

    /// Add a tool from an existing shared reference.
    pub fn with_tool_ref(mut self, tool: Arc<dyn Tool>) -> Self {
        self.tools.push(tool);
        self
    }

    /// Set the tool approval policy for this agent.
    ///
    /// Defaults to [`ApprovalPolicy::Ask`]: explicitly safe tools run
    /// automatically; mutating/custom tools require an approval handler.
    pub fn with_approval_policy(mut self, policy: ApprovalPolicy) -> Self {
        self.approval_policy = policy;
        self
    }

    /// Set the approval handler used when [`ApprovalPolicy::Ask`] requires a decision.
    pub fn with_approval_handler(mut self, handler: ApprovalHandler) -> Self {
        self.approval_handler = Some(handler);
        self
    }

    /// Set generation settings.
    pub fn with_settings(mut self, settings: GenerationSettings) -> Self {
        self.settings = settings;
        self
    }

    /// Execute a user message and get a response (with tool loop).
    pub async fn execute(&mut self, message: impl Into<String>) -> Result<String, RociError> {
        self.conversation.add_user_message(message);
        let messages = self.messages_for_run();

        if !self.tools.is_empty() {
            let result = self.run_with_tools(messages, None).await?.wait().await;
            return match result.status {
                RunStatus::Completed => {
                    let text = final_assistant_text(&result.messages).unwrap_or_default();
                    self.conversation.add_assistant_message(&text);
                    Ok(text)
                }
                RunStatus::Failed => {
                    Err(RociError::Stream(result.error.unwrap_or_else(|| {
                        "agent run failed without error message".to_string()
                    })))
                }
                RunStatus::Canceled => Err(RociError::Stream("agent run canceled".to_string())),
                RunStatus::Running => Err(RociError::InvalidState(
                    "agent run returned before completion".to_string(),
                )),
            };
        }

        let provider = self.registry.create_provider(
            self.model.provider_name(),
            self.model.model_id(),
            &self.config,
        )?;

        let result = crate::generation::text::generate_text(
            provider.as_ref(),
            messages,
            self.settings.clone(),
            &self.tools,
        )
        .await?;

        self.conversation.add_assistant_message(&result.text);

        Ok(result.text)
    }

    /// Stream a response to a user message.
    pub async fn stream(
        &mut self,
        message: impl Into<String>,
    ) -> Result<BoxStream<'_, Result<TextStreamDelta, RociError>>, RociError> {
        self.conversation.add_user_message(message);
        let messages = self.messages_for_run();

        if !self.tools.is_empty() {
            let (tx, rx) = futures::channel::mpsc::unbounded();
            let tool_calls = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
            let sink: RunEventSink = {
                let tool_calls = tool_calls.clone();
                Arc::new(move |event| {
                    if let Some(item) = run_event_to_stream_item(event, &tool_calls) {
                        let _ = tx.unbounded_send(item);
                    }
                })
            };
            let mut handle = self.run_with_tools(messages, Some(sink)).await?;
            let abort_tx = handle.take_abort_sender();
            let stream = ToolStream {
                rx,
                wait: Box::pin(handle.wait()),
                abort_tx,
                conversation: &mut self.conversation,
                completed: false,
            };
            return Ok(stream.boxed());
        }

        let provider = self.registry.create_provider(
            self.model.provider_name(),
            self.model.model_id(),
            &self.config,
        )?;
        let provider = Arc::from(provider);

        crate::generation::stream::stream_text_with_tools(
            provider,
            messages,
            self.settings.clone(),
            &self.tools,
            Vec::new(),
        )
        .await
    }

    /// Get the conversation history.
    pub fn conversation(&self) -> &Conversation {
        &self.conversation
    }

    /// Clear conversation history.
    pub fn clear_history(&mut self) {
        self.conversation.clear();
    }

    fn messages_for_run(&self) -> Vec<ModelMessage> {
        let mut messages = Vec::new();
        if let Some(ref sys) = self.system_prompt {
            messages.push(ModelMessage::system(sys.clone()));
        }
        messages.extend(self.conversation.messages().iter().cloned());
        messages
    }

    async fn run_with_tools(
        &self,
        messages: Vec<ModelMessage>,
        event_sink: Option<RunEventSink>,
    ) -> Result<crate::agent_loop::RunHandle, RociError> {
        let runner = LoopRunner::with_registry(self.config.clone(), self.registry.clone());
        let mut request = RunRequest::new(self.model.clone(), messages)
            .with_tools(self.tools.clone())
            .with_approval_policy(self.approval_policy);
        if let Some(handler) = &self.approval_handler {
            request = request.with_approval_handler(handler.clone());
        }
        request.settings = self.settings.clone();
        if let Some(sink) = event_sink {
            request = request.with_event_sink(sink);
        }
        runner.start(request).await
    }
}

struct ToolStream<'a> {
    rx: futures::channel::mpsc::UnboundedReceiver<Result<TextStreamDelta, RociError>>,
    wait: Pin<Box<dyn Future<Output = crate::agent_loop::RunResult> + Send>>,
    abort_tx: Option<oneshot::Sender<()>>,
    conversation: &'a mut Conversation,
    completed: bool,
}

impl Stream for ToolStream<'_> {
    type Item = Result<TextStreamDelta, RociError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.completed {
            return Poll::Ready(None);
        }

        match Pin::new(&mut self.rx).poll_next(cx) {
            Poll::Ready(Some(Ok(delta))) => Poll::Ready(Some(Ok(delta))),
            Poll::Ready(Some(Err(error))) => {
                if let Some(tx) = self.abort_tx.take() {
                    let _ = tx.send(());
                }
                self.completed = true;
                Poll::Ready(Some(Err(error)))
            }
            Poll::Pending => Poll::Pending,
            Poll::Ready(None) => match self.wait.as_mut().poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(result) => {
                    self.abort_tx.take();
                    if matches!(result.status, RunStatus::Completed) {
                        if let Some(text) = final_assistant_text(&result.messages) {
                            self.conversation.add_assistant_message(text);
                        }
                    }
                    self.completed = true;
                    Poll::Ready(None)
                }
            },
        }
    }
}

impl Drop for ToolStream<'_> {
    fn drop(&mut self) {
        if !self.completed {
            if let Some(tx) = self.abort_tx.take() {
                let _ = tx.send(());
            }
        }
    }
}

fn final_assistant_text(messages: &[ModelMessage]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|message| matches!(message.role, Role::Assistant))
        .map(ModelMessage::text)
}

fn run_event_to_stream_item(
    event: RunEvent,
    tool_calls: &Arc<std::sync::Mutex<std::collections::HashMap<String, AgentToolCall>>>,
) -> Option<Result<TextStreamDelta, RociError>> {
    match event.payload {
        RunEventPayload::Lifecycle {
            state: RunLifecycle::Started,
        } => Some(Ok(stream_delta(StreamEventType::Start))),
        RunEventPayload::Lifecycle {
            state: RunLifecycle::Completed,
        } => {
            let mut delta = stream_delta(StreamEventType::Done);
            delta.finish_reason = Some(FinishReason::Stop);
            Some(Ok(delta))
        }
        RunEventPayload::Lifecycle {
            state: RunLifecycle::Failed { error },
        } => Some(Err(RociError::Stream(error))),
        RunEventPayload::Lifecycle {
            state: RunLifecycle::Canceled,
        } => Some(Err(RociError::Stream("agent run canceled".to_string()))),
        RunEventPayload::AssistantDelta { text } => Some(Ok(TextStreamDelta {
            text,
            event_type: StreamEventType::TextDelta,
            tool_call: None,
            finish_reason: None,
            usage: None,
            reasoning: None,
            reasoning_signature: None,
            reasoning_type: None,
        })),
        RunEventPayload::ReasoningDelta { text } => Some(Ok(TextStreamDelta {
            text: String::new(),
            event_type: StreamEventType::Reasoning,
            tool_call: None,
            finish_reason: None,
            usage: None,
            reasoning: Some(text),
            reasoning_signature: None,
            reasoning_type: None,
        })),
        RunEventPayload::ToolCallStarted { call } | RunEventPayload::ToolCallCompleted { call } => {
            if let Ok(mut calls) = tool_calls.lock() {
                calls.insert(call.id.clone(), call.clone());
            }
            Some(Ok(tool_call_delta(call)))
        }
        RunEventPayload::ToolCallDelta { call_id, delta } => {
            let call = tool_calls.lock().ok().and_then(|mut calls| {
                let call = calls.get_mut(&call_id)?;
                call.arguments = delta;
                Some(call.clone())
            })?;
            Some(Ok(tool_call_delta(call)))
        }
        RunEventPayload::Error { message } => Some(Err(RociError::Stream(message))),
        RunEventPayload::ToolResult { .. }
        | RunEventPayload::PlanUpdated { .. }
        | RunEventPayload::DiffUpdated { .. }
        | RunEventPayload::ApprovalRequired { .. } => None,
    }
}

fn stream_delta(event_type: StreamEventType) -> TextStreamDelta {
    TextStreamDelta {
        text: String::new(),
        event_type,
        tool_call: None,
        finish_reason: None,
        usage: None,
        reasoning: None,
        reasoning_signature: None,
        reasoning_type: None,
    }
}

fn tool_call_delta(call: AgentToolCall) -> TextStreamDelta {
    TextStreamDelta {
        text: String::new(),
        event_type: StreamEventType::ToolCallDelta,
        tool_call: Some(call),
        finish_reason: None,
        usage: None,
        reasoning: None,
        reasoning_signature: None,
        reasoning_type: None,
    }
}

#[cfg(test)]
#[path = "core_tests.rs"]
mod tests;
