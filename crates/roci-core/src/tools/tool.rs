//! Tool trait and closure-based tool wrapper.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;

use super::arguments::ToolArguments;
use super::types::AgentToolParameters;
use crate::error::RociError;

/// Context available during tool execution.
#[derive(Clone)]
pub struct ToolExecutionContext {
    /// Additional metadata for the tool.
    pub metadata: serde_json::Value,
    /// Tool call id (if provided by the model).
    pub tool_call_id: Option<String>,
    /// Tool name as requested by the model.
    pub tool_name: Option<String>,
    /// Callback to request user input. None if not configured.
    #[cfg(feature = "agent")]
    pub request_user_input: Option<super::user_input::RequestUserInputFn>,
}

impl Default for ToolExecutionContext {
    fn default() -> Self {
        Self {
            metadata: serde_json::Value::Null,
            tool_call_id: None,
            tool_name: None,
            #[cfg(feature = "agent")]
            request_user_input: None,
        }
    }
}

#[cfg(feature = "agent")]
impl std::fmt::Debug for ToolExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolExecutionContext")
            .field("metadata", &self.metadata)
            .field("tool_call_id", &self.tool_call_id)
            .field("tool_name", &self.tool_name)
            .field(
                "request_user_input",
                &self.request_user_input.as_ref().map(|_| "<callback>"),
            )
            .finish()
    }
}

#[cfg(not(feature = "agent"))]
impl std::fmt::Debug for ToolExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolExecutionContext")
            .field("metadata", &self.metadata)
            .field("tool_call_id", &self.tool_call_id)
            .field("tool_name", &self.tool_name)
            .finish()
    }
}

/// Callback for streaming partial tool results during execution.
#[cfg(feature = "agent")]
pub type ToolUpdateCallback =
    Arc<dyn Fn(crate::agent_loop::events::ToolUpdatePayload) + Send + Sync>;

/// Safety metadata used by `ApprovalPolicy::Ask`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSafety {
    /// Reads local or remote state without intentional mutation.
    ReadOnly,
    /// Requests host/user input without modifying external state.
    HostInput,
}

/// Tool-level approval category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolApprovalKind {
    CommandExecution,
    FileChange,
    Other,
}

/// Approval behavior declared by a tool implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolApproval {
    /// Safe to execute without prompting under `ApprovalPolicy::Ask`.
    AutoAccept { safety: ToolSafety },
    /// Prompt before execution under `ApprovalPolicy::Ask`.
    RequiresApproval { kind: ToolApprovalKind },
}

impl ToolApproval {
    pub const fn safe_read_only() -> Self {
        Self::AutoAccept {
            safety: ToolSafety::ReadOnly,
        }
    }

    pub const fn safe_host_input() -> Self {
        Self::AutoAccept {
            safety: ToolSafety::HostInput,
        }
    }

    pub const fn requires_approval(kind: ToolApprovalKind) -> Self {
        Self::RequiresApproval { kind }
    }
}

/// Core tool trait -- implement to create custom tools.
///
/// Existing implementations only need [`execute`]. The agent loop calls
/// [`execute_ext`] which delegates to [`execute`] by default. Override
/// [`execute_ext`] to support cancellation tokens and streaming updates.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must match what the model calls).
    fn name(&self) -> &str;

    /// Human-readable label for UI display. Defaults to [`name`].
    fn label(&self) -> &str {
        self.name()
    }

    /// Human-readable description.
    fn description(&self) -> &str;

    /// JSON Schema parameters.
    fn parameters(&self) -> &AgentToolParameters;

    /// Approval metadata for `ApprovalPolicy::Ask`.
    ///
    /// Custom, dynamic, and unknown tools are approval-required by default.
    fn approval(&self) -> ToolApproval {
        ToolApproval::requires_approval(ToolApprovalKind::Other)
    }

    /// Execute the tool with parsed arguments.
    async fn execute(
        &self,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError>;

    /// Extended execute with cancellation and streaming updates.
    ///
    /// The default implementation ignores `cancel` and `on_update`, delegating
    /// to [`execute`]. Override this for tools that need streaming partial
    /// results or cooperative cancellation.
    #[cfg(feature = "agent")]
    async fn execute_ext(
        &self,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
        _cancel: tokio_util::sync::CancellationToken,
        _on_update: Option<ToolUpdateCallback>,
    ) -> Result<serde_json::Value, RociError> {
        self.execute(args, ctx).await
    }
}

/// Type alias for the tool handler function.
type ToolHandler = dyn Fn(
        ToolArguments,
        ToolExecutionContext,
    ) -> Pin<Box<dyn Future<Output = Result<serde_json::Value, RociError>> + Send>>
    + Send
    + Sync;

/// Closure-based tool for quick tool creation.
pub struct AgentTool {
    name: String,
    description: String,
    parameters: AgentToolParameters,
    approval: ToolApproval,
    handler: Arc<ToolHandler>,
}

impl AgentTool {
    /// Create a tool from a closure.
    pub fn new<F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: AgentToolParameters,
        handler: F,
    ) -> Self
    where
        F: Fn(ToolArguments, ToolExecutionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<serde_json::Value, RociError>> + Send + 'static,
    {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            approval: ToolApproval::requires_approval(ToolApprovalKind::Other),
            handler: Arc::new(move |args, ctx| Box::pin(handler(args, ctx))),
        }
    }

    /// Set approval metadata for `ApprovalPolicy::Ask`.
    pub fn with_approval(mut self, approval: ToolApproval) -> Self {
        self.approval = approval;
        self
    }
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters(&self) -> &AgentToolParameters {
        &self.parameters
    }

    fn approval(&self) -> ToolApproval {
        self.approval
    }

    async fn execute(
        &self,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        (self.handler)(args.clone(), ctx.clone()).await
    }
}

impl std::fmt::Debug for AgentTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentTool")
            .field("name", &self.name)
            .field("description", &self.description)
            .finish()
    }
}
