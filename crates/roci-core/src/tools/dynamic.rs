//! Dynamic tool provider -- runtime-discovered tools (e.g., MCP).

use std::sync::Arc;

use async_trait::async_trait;

use super::arguments::ToolArguments;
use super::tool::{
    Tool, ToolExecutionContext, ToolPromptMetadata, ToolResultSizePolicy, ToolSafetyPlan,
    ToolSafetySummary,
};
use super::types::AgentToolParameters;
use crate::error::RociError;

/// A tool discovered at runtime (e.g., from MCP server).
#[derive(Debug, Clone)]
pub struct DynamicTool {
    pub name: String,
    pub description: String,
    pub aliases: Vec<String>,
    pub prompt: Option<String>,
    pub prompt_metadata: ToolPromptMetadata,
    pub result_policy: ToolResultSizePolicy,
    pub parameters: AgentToolParameters,
    pub safety: ToolSafetyPlan,
    pub safety_summary: ToolSafetySummary,
}

impl DynamicTool {
    /// Create a dynamic tool with fail-closed safety metadata by default.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: AgentToolParameters,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            aliases: Vec::new(),
            prompt: None,
            prompt_metadata: ToolPromptMetadata::default(),
            result_policy: ToolResultSizePolicy::default(),
            parameters,
            safety: ToolSafetyPlan::default(),
            safety_summary: ToolSafetySummary::default(),
        }
    }

    /// Set explicit safety metadata for a dynamic tool.
    pub fn with_safety(mut self, plan: ToolSafetyPlan, summary: ToolSafetySummary) -> Self {
        self.safety = plan;
        self.safety_summary = summary;
        self
    }
}

/// Trait for providers that can discover and execute tools at runtime.
#[async_trait]
pub trait DynamicToolProvider: Send + Sync {
    /// Stable server ids exposed by providers that support scoped discovery.
    fn server_ids(&self) -> Vec<String> {
        Vec::new()
    }

    /// List available tools.
    async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError>;

    /// List tools from selected server ids.
    ///
    /// Providers without server scoping support accept an empty selection as
    /// an unscoped list and reject non-empty selections.
    async fn list_tools_for_servers(
        &self,
        server_ids: &[String],
    ) -> Result<Vec<DynamicTool>, RociError> {
        if server_ids.is_empty() {
            return self.list_tools().await;
        }
        Err(RociError::UnsupportedOperation(
            "dynamic tool provider does not support server-scoped discovery".into(),
        ))
    }

    /// Execute a tool by name.
    async fn execute_tool(
        &self,
        name: &str,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError>;

    /// Execute a tool only if its current route belongs to one of the selected servers.
    async fn execute_tool_for_servers(
        &self,
        server_ids: &[String],
        name: &str,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        if server_ids.is_empty() {
            return self.execute_tool(name, args, ctx).await;
        }
        Err(RociError::UnsupportedOperation(
            "dynamic tool provider does not support server-scoped execution".into(),
        ))
    }
}

/// Restricts dynamic discovery to a fixed set of provider server ids.
pub struct ScopedDynamicToolProvider {
    provider: Arc<dyn DynamicToolProvider>,
    server_ids: Vec<String>,
}

impl ScopedDynamicToolProvider {
    pub fn new(provider: Arc<dyn DynamicToolProvider>, server_ids: Vec<String>) -> Self {
        Self {
            provider,
            server_ids,
        }
    }
}

#[async_trait]
impl DynamicToolProvider for ScopedDynamicToolProvider {
    fn server_ids(&self) -> Vec<String> {
        self.server_ids.clone()
    }

    async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
        if self.server_ids.is_empty() {
            return Ok(Vec::new());
        }
        self.provider.list_tools_for_servers(&self.server_ids).await
    }

    async fn list_tools_for_servers(
        &self,
        server_ids: &[String],
    ) -> Result<Vec<DynamicTool>, RociError> {
        let effective_server_ids = if server_ids.is_empty() {
            &self.server_ids
        } else {
            server_ids
        };
        if effective_server_ids.is_empty() {
            return Ok(Vec::new());
        }
        let requested = effective_server_ids
            .iter()
            .collect::<std::collections::HashSet<_>>();
        if !requested
            .iter()
            .all(|server_id| self.server_ids.contains(server_id))
        {
            return Err(RociError::InvalidArgument(
                "requested server is outside the dynamic provider scope".into(),
            ));
        }
        self.provider
            .list_tools_for_servers(effective_server_ids)
            .await
    }

    async fn execute_tool(
        &self,
        name: &str,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        if self.server_ids.is_empty() {
            return Err(RociError::InvalidArgument(format!(
                "tool '{name}' is outside the dynamic provider scope"
            )));
        }
        self.provider
            .execute_tool_for_servers(&self.server_ids, name, args, ctx)
            .await
    }

    async fn execute_tool_for_servers(
        &self,
        server_ids: &[String],
        name: &str,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        let effective_server_ids = if server_ids.is_empty() {
            &self.server_ids
        } else {
            server_ids
        };
        if effective_server_ids.is_empty() {
            return Err(RociError::InvalidArgument(format!(
                "tool '{name}' is outside the dynamic provider scope"
            )));
        }
        if !effective_server_ids
            .iter()
            .all(|server_id| self.server_ids.contains(server_id))
        {
            return Err(RociError::InvalidArgument(
                "requested server is outside the dynamic provider scope".into(),
            ));
        }
        self.provider
            .execute_tool_for_servers(effective_server_ids, name, args, ctx)
            .await
    }
}

/// Adapter that exposes a [`DynamicTool`] through the core [`Tool`] trait.
pub struct DynamicToolAdapter {
    provider: Arc<dyn DynamicToolProvider>,
    name: String,
    description: String,
    aliases: Vec<String>,
    prompt: Option<String>,
    prompt_metadata: ToolPromptMetadata,
    result_policy: ToolResultSizePolicy,
    parameters: AgentToolParameters,
    safety: ToolSafetyPlan,
    safety_summary: ToolSafetySummary,
}

impl DynamicToolAdapter {
    /// Create a new adapter for a discovered tool.
    pub fn new(provider: Arc<dyn DynamicToolProvider>, tool: DynamicTool) -> Self {
        Self {
            provider,
            name: tool.name,
            description: tool.description,
            aliases: tool.aliases,
            prompt: tool.prompt,
            prompt_metadata: tool.prompt_metadata,
            result_policy: tool.result_policy,
            parameters: tool.parameters,
            safety: tool.safety,
            safety_summary: tool.safety_summary,
        }
    }
}

#[async_trait]
impl Tool for DynamicToolAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn aliases(&self) -> &[String] {
        &self.aliases
    }

    fn prompt(&self) -> &str {
        self.prompt.as_deref().unwrap_or(&self.description)
    }

    fn prompt_metadata(&self) -> ToolPromptMetadata {
        self.prompt_metadata.clone()
    }

    fn result_policy(&self) -> ToolResultSizePolicy {
        self.result_policy
    }

    fn parameters(&self) -> &AgentToolParameters {
        &self.parameters
    }

    fn safety(&self, _args: &ToolArguments) -> ToolSafetyPlan {
        self.safety.clone()
    }

    fn safety_summary(&self) -> ToolSafetySummary {
        self.safety_summary
    }

    async fn execute(
        &self,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        self.provider.execute_tool(&self.name, args, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::super::tool::ToolSafetyKind;
    use super::*;

    struct NoopProvider;

    #[async_trait]
    impl DynamicToolProvider for NoopProvider {
        async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
            Ok(Vec::new())
        }

        async fn execute_tool(
            &self,
            _name: &str,
            _args: &ToolArguments,
            _ctx: &ToolExecutionContext,
        ) -> Result<serde_json::Value, RociError> {
            Ok(serde_json::json!({}))
        }
    }

    #[tokio::test]
    async fn default_provider_rejects_nonempty_server_scope() {
        let err = NoopProvider
            .list_tools_for_servers(&["github".to_string()])
            .await
            .expect_err("unscoped provider must reject server selection");

        assert!(
            matches!(err, RociError::UnsupportedOperation(message) if message.contains("server"))
        );
    }

    struct ServerProvider;

    #[async_trait]
    impl DynamicToolProvider for ServerProvider {
        fn server_ids(&self) -> Vec<String> {
            vec!["github".into(), "linear".into()]
        }

        async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
            Err(RociError::InvalidState(
                "scoped wrapper must not list every server".into(),
            ))
        }

        async fn list_tools_for_servers(
            &self,
            server_ids: &[String],
        ) -> Result<Vec<DynamicTool>, RociError> {
            Ok(server_ids
                .iter()
                .map(|server_id| {
                    DynamicTool::new(
                        format!("mcp__{server_id}__search"),
                        "search",
                        AgentToolParameters::empty(),
                    )
                })
                .collect())
        }

        async fn execute_tool(
            &self,
            _name: &str,
            _args: &ToolArguments,
            _ctx: &ToolExecutionContext,
        ) -> Result<serde_json::Value, RociError> {
            Ok(serde_json::json!({}))
        }

        async fn execute_tool_for_servers(
            &self,
            server_ids: &[String],
            _name: &str,
            _args: &ToolArguments,
            _ctx: &ToolExecutionContext,
        ) -> Result<serde_json::Value, RociError> {
            Ok(serde_json::json!({ "servers": server_ids }))
        }
    }

    #[tokio::test]
    async fn scoped_provider_lists_only_allowed_server_ids() {
        let provider =
            ScopedDynamicToolProvider::new(Arc::new(ServerProvider), vec!["linear".to_string()]);

        assert_eq!(provider.server_ids(), vec!["linear"]);
        let tools = provider
            .list_tools()
            .await
            .expect("scoped list should work");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "mcp__linear__search");

        let tools = provider
            .list_tools_for_servers(&[])
            .await
            .expect("empty nested selection should retain outer scope");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "mcp__linear__search");

        let empty_provider = ScopedDynamicToolProvider::new(Arc::new(ServerProvider), Vec::new());
        assert!(empty_provider
            .list_tools()
            .await
            .expect("empty scope should expose no tools")
            .is_empty());
    }

    #[tokio::test]
    async fn empty_scope_rejects_execution_of_hidden_tools() {
        let provider = ScopedDynamicToolProvider::new(Arc::new(ServerProvider), Vec::new());
        let args = ToolArguments::new(serde_json::json!({}));
        let ctx = ToolExecutionContext::default();

        let error = provider
            .execute_tool("mcp__github__search", &args, &ctx)
            .await
            .expect_err("empty scope must not execute a known hidden tool");
        assert!(
            matches!(error, RociError::InvalidArgument(message) if message.contains("outside"))
        );

        let error = provider
            .execute_tool_for_servers(&[], "mcp__github__search", &args, &ctx)
            .await
            .expect_err("empty nested scope must not execute a known hidden tool");
        assert!(
            matches!(error, RociError::InvalidArgument(message) if message.contains("outside"))
        );
    }

    #[tokio::test]
    async fn nested_scoped_provider_executes_with_intersection_scope() {
        let inner: Arc<dyn DynamicToolProvider> = Arc::new(ScopedDynamicToolProvider::new(
            Arc::new(ServerProvider),
            vec!["github".to_string(), "linear".to_string()],
        ));
        let outer = ScopedDynamicToolProvider::new(inner, vec!["linear".to_string()]);

        let result = outer
            .execute_tool(
                "mcp__linear__search",
                &ToolArguments::new(serde_json::json!({})),
                &ToolExecutionContext::default(),
            )
            .await
            .expect("nested scope should delegate allowed execution");

        assert_eq!(result["servers"], serde_json::json!(["linear"]));
    }

    #[test]
    fn dynamic_tool_defaults_to_fail_closed_safety() {
        let tool = DynamicTool::new("dynamic", "dynamic tool", AgentToolParameters::empty());

        assert_eq!(tool.safety, ToolSafetyPlan::default());
        assert_eq!(tool.safety_summary, ToolSafetySummary::default());
        assert_eq!(tool.safety.approval.kind, ToolSafetyKind::Other);
        assert!(!tool.safety.read_only);
        assert!(!tool.safety.destructive);
        assert!(!tool.safety.concurrency_safe);
    }

    #[test]
    fn dynamic_tool_adapter_uses_declared_safety_metadata() {
        let plan = ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read);
        let summary = ToolSafetySummary {
            read_only_by_default: true,
            destructive_by_default: false,
            concurrency_safe_by_default: true,
            approval_kind: ToolSafetyKind::Read,
        };
        let adapter = DynamicToolAdapter::new(
            Arc::new(NoopProvider),
            DynamicTool::new("dynamic", "dynamic tool", AgentToolParameters::empty())
                .with_safety(plan.clone(), summary),
        );

        assert_eq!(
            adapter.safety(&ToolArguments::new(serde_json::json!({}))),
            plan
        );
        assert_eq!(adapter.safety_summary(), summary);
    }

    #[test]
    fn prompt_metadata_dynamic_tool_adapter_uses_declared_metadata() {
        let metadata = ToolPromptMetadata {
            guidelines: vec!["Use dynamic context carefully.".to_string()],
            search_hint: Some("future-search-only".to_string()),
        };
        let mut tool = DynamicTool::new("dynamic", "UI description", AgentToolParameters::empty());
        tool.prompt = Some("Model prompt".to_string());
        tool.prompt_metadata = metadata.clone();
        let adapter = DynamicToolAdapter::new(Arc::new(NoopProvider), tool);

        assert_eq!(adapter.description(), "UI description");
        assert_eq!(adapter.prompt(), "Model prompt");
        assert_eq!(adapter.prompt_metadata(), metadata);
    }
}
