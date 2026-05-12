//! Tool trait and closure-based tool wrapper.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::arguments::ToolArguments;
use super::types::AgentToolParameters;
use crate::error::RociError;
use crate::session::{LogicalPath, SessionFs};

/// Validates sandbox-sensitive tool operations before execution.
#[async_trait]
pub trait SandboxProvider: Send + Sync {
    async fn validate_shell_command(
        &self,
        command: &str,
        cwd: &LogicalPath,
    ) -> Result<(), RociError>;
}

/// Context available during tool execution.
#[derive(Clone)]
pub struct ToolExecutionContext {
    /// Additional metadata for the tool.
    pub metadata: serde_json::Value,
    /// Tool call id (if provided by the model).
    pub tool_call_id: Option<String>,
    /// Tool name as requested by the model.
    pub tool_name: Option<String>,
    /// Session-owned filesystem for tools that operate on durable files.
    pub session_fs: Option<Arc<dyn SessionFs + Send + Sync>>,
    /// Logical current working directory inside the session filesystem.
    pub session_cwd: Option<LogicalPath>,
    /// Optional sandbox validator for command-capable tools.
    pub sandbox_provider: Option<Arc<dyn SandboxProvider>>,
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
            session_fs: None,
            session_cwd: None,
            sandbox_provider: None,
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
                "session_fs",
                &self.session_fs.as_ref().map(|_| "<session_fs>"),
            )
            .field("session_cwd", &self.session_cwd)
            .field(
                "sandbox_provider",
                &self.sandbox_provider.as_ref().map(|_| "<sandbox_provider>"),
            )
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
            .field(
                "session_fs",
                &self.session_fs.as_ref().map(|_| "<session_fs>"),
            )
            .field("session_cwd", &self.session_cwd)
            .field(
                "sandbox_provider",
                &self.sandbox_provider.as_ref().map(|_| "<sandbox_provider>"),
            )
            .finish()
    }
}

/// Callback for streaming partial tool results during execution.
#[cfg(feature = "agent")]
pub type ToolUpdateCallback =
    Arc<dyn Fn(crate::agent_loop::events::ToolUpdatePayload) + Send + Sync>;

/// Safety category used by approval and batching policies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolSafetyKind {
    /// Executes a local or remote command.
    CommandExecution,
    /// Mutates filesystem state.
    FileChange,
    /// Reads local or remote state.
    Read,
    /// Invokes an MCP tool.
    Mcp,
    /// Invokes a custom SDK tool.
    CustomTool,
    /// Tool category is unknown or not yet classified.
    Other,
}

/// Minimum action an approval policy may take for a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolActionFloor {
    /// User approval is required.
    Ask,
    /// Tool call must be denied.
    Deny,
}

/// Approval requirement declared by a tool safety plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolApprovalRequirement {
    /// Approval category.
    pub kind: ToolSafetyKind,
    /// Whether `ApprovalPolicy::ask()` may accept this call without prompting.
    pub auto_accept_under_ask: bool,
    /// Optional policy floor for high-risk calls.
    pub action_floor: Option<ToolActionFloor>,
    /// Human-readable reason for approval handling.
    pub reason: Option<String>,
    /// Whether session-scoped approval may be offered.
    pub allow_session: bool,
}

impl Default for ToolApprovalRequirement {
    fn default() -> Self {
        Self {
            kind: ToolSafetyKind::Other,
            auto_accept_under_ask: false,
            action_floor: None,
            reason: Some("tool requires approval by default".to_string()),
            allow_session: true,
        }
    }
}

/// Filesystem operation a tool call intends to perform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolFilesystemAccess {
    /// Filesystem operation kind.
    pub operation: crate::security::filesystem::PathOperation,
    /// Target path.
    pub path: PathBuf,
}

/// Resource access mode for scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolResourceAccessMode {
    /// Multiple readers can share this resource.
    SharedRead,
    /// Tool requires exclusive access to this resource.
    Exclusive,
}

/// Named resource a tool call intends to access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolResourceAccess {
    /// Stable resource key.
    pub key: String,
    /// Requested access mode.
    pub mode: ToolResourceAccessMode,
}

/// Static safety preview for catalogs and UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSafetySummary {
    /// Tool is read-only unless args prove otherwise.
    pub read_only_by_default: bool,
    /// Tool is destructive unless args prove otherwise.
    pub destructive_by_default: bool,
    /// Tool may run concurrently unless args prove otherwise.
    pub concurrency_safe_by_default: bool,
    /// Default approval category.
    pub approval_kind: ToolSafetyKind,
}

impl Default for ToolSafetySummary {
    fn default() -> Self {
        Self {
            read_only_by_default: false,
            destructive_by_default: false,
            concurrency_safe_by_default: false,
            approval_kind: ToolSafetyKind::Other,
        }
    }
}

/// Input-aware safety plan for a single tool call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSafetyPlan {
    /// Tool call only reads state.
    pub read_only: bool,
    /// Tool call is irreversible or high impact.
    pub destructive: bool,
    /// Tool call can run concurrently with other safe calls.
    pub concurrency_safe: bool,
    /// Approval requirement for this call.
    pub approval: ToolApprovalRequirement,
    /// Command classifier output, when available.
    pub command: Option<crate::security::command::CommandInsight>,
    /// Filesystem access facts, when available.
    pub filesystem: Vec<ToolFilesystemAccess>,
    /// Resource access facts, when available.
    pub resources: Vec<ToolResourceAccess>,
}

impl ToolSafetyPlan {
    /// Build an approval-required plan for the given kind.
    pub fn approval_required(kind: ToolSafetyKind) -> Self {
        Self {
            approval: ToolApprovalRequirement {
                kind,
                ..ToolApprovalRequirement::default()
            },
            ..Self::default()
        }
    }

    /// Build a read-only plan that can auto-accept under ask mode.
    pub fn safe_read_only(kind: ToolSafetyKind) -> Self {
        Self {
            read_only: true,
            destructive: false,
            concurrency_safe: true,
            approval: ToolApprovalRequirement {
                kind,
                auto_accept_under_ask: true,
                action_floor: None,
                reason: None,
                allow_session: true,
            },
            ..Self::default()
        }
    }

    /// Build a plan for host/user-input-only tools.
    pub fn host_input() -> Self {
        Self {
            read_only: false,
            destructive: false,
            concurrency_safe: false,
            approval: ToolApprovalRequirement {
                kind: ToolSafetyKind::Other,
                auto_accept_under_ask: true,
                action_floor: None,
                reason: None,
                allow_session: false,
            },
            ..Self::default()
        }
    }

    /// Build a plan with one filesystem access fact.
    pub fn file_access(
        kind: ToolSafetyKind,
        operation: crate::security::filesystem::PathOperation,
        path: impl Into<PathBuf>,
    ) -> Self {
        use crate::security::filesystem::PathOperation;

        let read_only = matches!(
            operation,
            PathOperation::Read | PathOperation::List | PathOperation::Search
        );
        let mut plan = if read_only {
            Self::safe_read_only(kind)
        } else {
            Self::approval_required(kind)
        };
        plan.read_only = read_only;
        plan.concurrency_safe = read_only;
        plan.destructive = matches!(operation, PathOperation::Delete);
        plan.filesystem.push(ToolFilesystemAccess {
            operation,
            path: path.into(),
        });
        plan
    }

    /// Build a read-file plan.
    pub fn file_read(path: impl Into<PathBuf>) -> Self {
        Self::file_access(
            ToolSafetyKind::Read,
            crate::security::filesystem::PathOperation::Read,
            path,
        )
    }

    /// Build a list-directory plan.
    pub fn file_list(path: impl Into<PathBuf>) -> Self {
        Self::file_access(
            ToolSafetyKind::Read,
            crate::security::filesystem::PathOperation::List,
            path,
        )
    }

    /// Build a file-search plan.
    pub fn file_search(path: impl Into<PathBuf>) -> Self {
        Self::file_access(
            ToolSafetyKind::Read,
            crate::security::filesystem::PathOperation::Search,
            path,
        )
    }

    /// Build a write-file plan.
    pub fn file_write(path: impl Into<PathBuf>) -> Self {
        Self::file_access(
            ToolSafetyKind::FileChange,
            crate::security::filesystem::PathOperation::Write,
            path,
        )
    }

    /// Build a create-file plan.
    pub fn file_create(path: impl Into<PathBuf>) -> Self {
        Self::file_access(
            ToolSafetyKind::FileChange,
            crate::security::filesystem::PathOperation::Create,
            path,
        )
    }

    /// Build a delete-file plan.
    pub fn file_delete(path: impl Into<PathBuf>) -> Self {
        Self::file_access(
            ToolSafetyKind::FileChange,
            crate::security::filesystem::PathOperation::Delete,
            path,
        )
    }

    /// Build a plan from command-classifier output.
    pub fn from_command_insight(insight: crate::security::command::CommandInsight) -> Self {
        use crate::security::command::CommandCategory;

        let read_only = !insight.categories.is_empty()
            && insight
                .categories
                .iter()
                .all(|category| matches!(category, CommandCategory::ReadOnly));
        let destructive = insight
            .categories
            .contains(&CommandCategory::DestructiveDelete);

        let mut plan = if read_only {
            Self::safe_read_only(ToolSafetyKind::CommandExecution)
        } else {
            Self::approval_required(ToolSafetyKind::CommandExecution)
        };
        plan.read_only = read_only;
        plan.concurrency_safe = read_only;
        plan.destructive = destructive;
        plan.command = Some(insight);
        if destructive {
            plan.approval.action_floor = Some(ToolActionFloor::Ask);
            plan.approval.reason = Some("destructive command requires approval".to_string());
        }
        plan
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

    /// Input-aware safety plan for this tool call.
    ///
    /// Custom, dynamic, and unknown tools fail closed by default.
    fn safety(&self, _args: &ToolArguments) -> ToolSafetyPlan {
        ToolSafetyPlan::default()
    }

    /// Static safety preview for catalogs and UI.
    fn safety_summary(&self) -> ToolSafetySummary {
        ToolSafetySummary::default()
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

/// Type alias for the tool safety handler function.
type ToolSafetyHandler = dyn Fn(&ToolArguments) -> ToolSafetyPlan + Send + Sync;

/// Closure-based tool for quick tool creation.
pub struct AgentTool {
    name: String,
    description: String,
    parameters: AgentToolParameters,
    safety_summary: ToolSafetySummary,
    safety_handler: Arc<ToolSafetyHandler>,
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
            safety_summary: ToolSafetySummary::default(),
            safety_handler: Arc::new(|_args| ToolSafetyPlan::default()),
            handler: Arc::new(move |args, ctx| Box::pin(handler(args, ctx))),
        }
    }

    /// Set a static safety plan.
    pub fn with_static_safety(mut self, plan: ToolSafetyPlan, summary: ToolSafetySummary) -> Self {
        let plan_for_handler = plan.clone();
        self.safety_summary = summary;
        self.safety_handler = Arc::new(move |_args| plan_for_handler.clone());
        self
    }

    /// Set an input-aware safety handler.
    pub fn with_safety<F>(mut self, summary: ToolSafetySummary, safety: F) -> Self
    where
        F: Fn(&ToolArguments) -> ToolSafetyPlan + Send + Sync + 'static,
    {
        self.safety_summary = summary;
        self.safety_handler = Arc::new(safety);
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

    fn safety(&self, args: &ToolArguments) -> ToolSafetyPlan {
        (self.safety_handler)(args)
    }

    fn safety_summary(&self) -> ToolSafetySummary {
        self.safety_summary
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
