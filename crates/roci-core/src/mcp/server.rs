//! Transport-agnostic MCP server core for native and MCP-backed tools.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    error::RociError,
    tools::{
        arguments::ToolArguments,
        dynamic::DynamicToolProvider,
        tool::{Tool, ToolExecutionContext},
        types::AgentToolParameters,
    },
};

/// Structured identity for tools exposed through MCP surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum McpToolIdentity {
    /// Native roci tool, exposed without MCP prefix.
    Native { name: String },
    /// Tool discovered from an upstream MCP server.
    Mcp {
        server_id: String,
        tool_name: String,
    },
}

impl McpToolIdentity {
    /// Return public exposed name for this identity.
    pub fn exposed_name(&self) -> String {
        match self {
            Self::Native { name } => name.clone(),
            Self::Mcp {
                server_id,
                tool_name,
            } => format!("mcp__{server_id}__{tool_name}"),
        }
    }

    /// Serialize identity for metadata transport.
    pub fn metadata(&self) -> serde_json::Value {
        serde_json::to_value(self).expect("McpToolIdentity serialization should not fail")
    }
}

/// Tool metadata returned by [`McpServerCore::list_tools`].
#[derive(Debug, Clone)]
pub struct McpToolSchema {
    pub exposed_name: String,
    pub identity: McpToolIdentity,
    pub description: String,
    pub parameters: AgentToolParameters,
}

/// MCP call-tool result shape.
#[derive(Debug, Clone, PartialEq)]
pub struct McpCallToolResult {
    pub content: Vec<serde_json::Value>,
    pub structured_content: Option<serde_json::Value>,
    pub is_error: bool,
}

impl McpCallToolResult {
    pub fn success(value: serde_json::Value) -> Self {
        Self {
            content: vec![json!({
                "type": "text",
                "text": value.to_string(),
            })],
            structured_content: Some(value),
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![json!({
                "type": "text",
                "text": message.into(),
            })],
            structured_content: None,
            is_error: true,
        }
    }
}

struct NativeToolEntry {
    tool: Arc<dyn Tool>,
}

struct McpProviderEntry {
    server_id: String,
    provider: Arc<dyn DynamicToolProvider>,
}

/// Tool registry and router that does not depend on any MCP transport.
#[derive(Default)]
pub struct McpServerCore {
    native_tools: Vec<NativeToolEntry>,
    mcp_providers: Vec<McpProviderEntry>,
}

impl McpServerCore {
    /// Create an empty server core.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one native tool. Native tools stay unprefixed.
    #[must_use]
    pub fn with_native_tool(mut self, tool: Arc<dyn Tool>) -> Self {
        self.native_tools.push(NativeToolEntry { tool });
        self
    }

    /// Add one upstream MCP provider. Provider tool names are exposed as
    /// `mcp__<server_id>__<tool_name>`.
    #[must_use]
    pub fn with_mcp_provider(
        mut self,
        server_id: impl Into<String>,
        provider: Arc<dyn DynamicToolProvider>,
    ) -> Self {
        self.mcp_providers.push(McpProviderEntry {
            server_id: server_id.into(),
            provider,
        });
        self
    }

    /// List native and MCP-backed tools in deterministic exposed-name order.
    pub async fn list_tools(&self) -> Result<Vec<McpToolSchema>, RociError> {
        let mut tools = Vec::new();
        let mut server_ids = HashSet::new();

        for entry in &self.native_tools {
            let identity = McpToolIdentity::Native {
                name: entry.tool.name().to_string(),
            };
            tools.push(McpToolSchema {
                exposed_name: identity.exposed_name(),
                identity,
                description: entry.tool.description().to_string(),
                parameters: entry.tool.parameters().clone(),
            });
        }

        for entry in &self.mcp_providers {
            if !server_ids.insert(entry.server_id.as_str()) {
                return Err(RociError::InvalidState(format!(
                    "Duplicate MCP server id '{}'",
                    entry.server_id
                )));
            }
            for tool in entry.provider.list_tools().await? {
                let identity = McpToolIdentity::Mcp {
                    server_id: entry.server_id.clone(),
                    tool_name: tool.name,
                };
                tools.push(McpToolSchema {
                    exposed_name: identity.exposed_name(),
                    identity,
                    description: tool.description,
                    parameters: tool.parameters,
                });
            }
        }

        tools.sort_by(|left, right| left.exposed_name.cmp(&right.exposed_name));
        Self::validate_unique_exposed_names(&tools)?;
        Ok(tools)
    }

    /// Call a tool by structured identity.
    pub async fn call_tool(
        &self,
        identity: &McpToolIdentity,
        args: serde_json::Value,
    ) -> Result<McpCallToolResult, RociError> {
        let args = ToolArguments::new(args);
        let ctx = ToolExecutionContext::default();

        match identity {
            McpToolIdentity::Native { name } => {
                let Some(tool) = self
                    .native_tools
                    .iter()
                    .find(|entry| entry.tool.name() == name)
                else {
                    return Ok(McpCallToolResult::error(format!("Unknown tool '{name}'")));
                };

                Ok(tool
                    .tool
                    .execute(&args, &ctx)
                    .await
                    .map(McpCallToolResult::success)
                    .unwrap_or_else(|error| Self::map_tool_error(name, error)))
            }
            McpToolIdentity::Mcp {
                server_id,
                tool_name,
            } => {
                let Some(provider) = self
                    .mcp_providers
                    .iter()
                    .find(|entry| &entry.server_id == server_id)
                else {
                    return Ok(McpCallToolResult::error(format!(
                        "Unknown MCP server '{server_id}'"
                    )));
                };

                Ok(provider
                    .provider
                    .execute_tool(tool_name, &args, &ctx)
                    .await
                    .map(McpCallToolResult::success)
                    .unwrap_or_else(|error| Self::map_tool_error(tool_name, error)))
            }
        }
    }

    /// Build a route table from exposed name to structured identity.
    pub async fn routes_by_exposed_name(
        &self,
    ) -> Result<HashMap<String, McpToolIdentity>, RociError> {
        let tools = self.list_tools().await?;
        let mut routes = HashMap::with_capacity(tools.len());
        for tool in tools {
            if routes
                .insert(tool.exposed_name.clone(), tool.identity)
                .is_some()
            {
                return Err(Self::duplicate_exposed_name_error(&tool.exposed_name));
            }
        }
        Ok(routes)
    }

    fn validate_unique_exposed_names(tools: &[McpToolSchema]) -> Result<(), RociError> {
        let mut exposed_names = HashSet::with_capacity(tools.len());
        for tool in tools {
            if !exposed_names.insert(tool.exposed_name.as_str()) {
                return Err(Self::duplicate_exposed_name_error(&tool.exposed_name));
            }
        }
        Ok(())
    }

    fn duplicate_exposed_name_error(exposed_name: &str) -> RociError {
        RociError::InvalidState(format!("Duplicate MCP exposed tool name '{exposed_name}'"))
    }

    fn map_tool_error(tool_name: &str, error: RociError) -> McpCallToolResult {
        match error {
            RociError::InvalidArgument(message) => McpCallToolResult::error(message),
            RociError::ToolExecution { message, .. } => {
                McpCallToolResult::error(format!("MCP tool call failed: {tool_name}: {message}"))
            }
            RociError::Stream(message) if message.to_ascii_lowercase().contains("cancel") => {
                McpCallToolResult::error(format!("MCP tool call canceled: {tool_name}: {message}"))
            }
            other => {
                McpCallToolResult::error(format!("MCP tool call failed: {tool_name}: {other}"))
            }
        }
    }
}

/// Back-compat alias for older internal name.
pub type McpServerToolIdentity = McpToolIdentity;
/// Back-compat alias for older internal name.
pub type McpServerListedTool = McpToolSchema;

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use tokio::sync::Mutex;

    use crate::tools::{dynamic::DynamicTool, tool::AgentTool, types::AgentToolParameters};

    struct MockProvider {
        tools: Vec<DynamicTool>,
        calls: Mutex<Vec<(String, serde_json::Value)>>,
        result: serde_json::Value,
    }

    #[async_trait]
    impl DynamicToolProvider for MockProvider {
        async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
            Ok(self.tools.clone())
        }

        async fn execute_tool(
            &self,
            name: &str,
            args: &ToolArguments,
            _ctx: &ToolExecutionContext,
        ) -> Result<serde_json::Value, RociError> {
            self.calls
                .lock()
                .await
                .push((name.to_string(), args.raw().clone()));
            Ok(self.result.clone())
        }
    }

    fn native_tool(name: &str) -> Arc<dyn Tool> {
        Arc::new(AgentTool::new(
            name,
            "native description",
            AgentToolParameters::object()
                .string("q", "query", true)
                .build(),
            |_args, _ctx| async { Ok(json!({"native": true})) },
        ))
    }

    fn dynamic_tool(name: &str) -> DynamicTool {
        DynamicTool::new(
            name,
            "dynamic description",
            AgentToolParameters::object()
                .string("q", "query", true)
                .build(),
        )
    }

    #[tokio::test]
    async fn list_tools_maps_native_and_dynamic_metadata_to_schema() {
        let provider = Arc::new(MockProvider {
            tools: vec![dynamic_tool("search")],
            calls: Mutex::new(Vec::new()),
            result: json!({}),
        });
        let core = McpServerCore::new()
            .with_native_tool(native_tool("read_file"))
            .with_mcp_provider("docs", provider);

        let tools = core.list_tools().await.expect("tools should list");

        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.exposed_name.as_str())
                .collect::<Vec<_>>(),
            vec!["mcp__docs__search", "read_file"]
        );
        assert_eq!(
            tools[0].parameters.schema["properties"]["q"]["type"],
            "string"
        );
        assert!(matches!(
            tools[0].identity,
            McpToolIdentity::Mcp { ref server_id, ref tool_name }
            if server_id == "docs" && tool_name == "search"
        ));
    }

    #[tokio::test]
    async fn routes_by_exposed_name_preserves_structured_identity() {
        let provider = Arc::new(MockProvider {
            tools: vec![dynamic_tool("beta__search")],
            calls: Mutex::new(Vec::new()),
            result: json!({}),
        });
        let core = McpServerCore::new().with_mcp_provider("alpha", provider);

        let routes = core
            .routes_by_exposed_name()
            .await
            .expect("routes should build");

        assert_eq!(
            routes.get("mcp__alpha__beta__search"),
            Some(&McpToolIdentity::Mcp {
                server_id: "alpha".to_string(),
                tool_name: "beta__search".to_string(),
            })
        );
    }

    #[tokio::test]
    async fn list_tools_rejects_native_and_mcp_exposed_name_collision() {
        let provider = Arc::new(MockProvider {
            tools: vec![dynamic_tool("search")],
            calls: Mutex::new(Vec::new()),
            result: json!({}),
        });
        let core = McpServerCore::new()
            .with_native_tool(native_tool("mcp__docs__search"))
            .with_mcp_provider("docs", provider);

        let err = core
            .list_tools()
            .await
            .expect_err("duplicate exposed names should fail");

        assert!(matches!(
            err,
            RociError::InvalidState(message)
            if message.contains("Duplicate MCP exposed tool name")
                && message.contains("mcp__docs__search")
        ));
    }

    #[tokio::test]
    async fn list_tools_rejects_duplicate_mcp_server_ids() {
        let first = Arc::new(MockProvider {
            tools: vec![dynamic_tool("search")],
            calls: Mutex::new(Vec::new()),
            result: json!({}),
        });
        let second = Arc::new(MockProvider {
            tools: vec![dynamic_tool("read")],
            calls: Mutex::new(Vec::new()),
            result: json!({}),
        });
        let core = McpServerCore::new()
            .with_mcp_provider("docs", first)
            .with_mcp_provider("docs", second);

        let err = core
            .routes_by_exposed_name()
            .await
            .expect_err("duplicate MCP server id should fail");

        assert!(matches!(
            err,
            RociError::InvalidState(message)
            if message.contains("Duplicate MCP server id") && message.contains("docs")
        ));
    }

    #[test]
    fn identity_serializer_preserves_structured_contract() {
        let identity = McpToolIdentity::Mcp {
            server_id: "alpha".into(),
            tool_name: "beta__search".into(),
        };

        assert_eq!(identity.exposed_name(), "mcp__alpha__beta__search");
        assert_eq!(
            identity.metadata(),
            json!({
                "kind": "mcp",
                "server_id": "alpha",
                "tool_name": "beta__search"
            })
        );
    }

    #[tokio::test]
    async fn call_tool_routes_by_identity_not_parsed_name() {
        let provider = Arc::new(MockProvider {
            tools: vec![dynamic_tool("beta__search")],
            calls: Mutex::new(Vec::new()),
            result: json!({"ok": true}),
        });
        let core = McpServerCore::new().with_mcp_provider("alpha", provider.clone());

        let result = core
            .call_tool(
                &McpToolIdentity::Mcp {
                    server_id: "alpha".to_string(),
                    tool_name: "beta__search".to_string(),
                },
                json!({"q": "rust"}),
            )
            .await
            .expect("tool should execute");

        assert!(!result.is_error);
        assert_eq!(result.structured_content, Some(json!({"ok": true})));
        assert_eq!(
            provider.calls.lock().await.as_slice(),
            &[("beta__search".to_string(), json!({"q": "rust"}))]
        );
    }

    #[tokio::test]
    async fn call_tool_maps_unknown_validation_runtime_and_cancel_errors() {
        let core = McpServerCore::new().with_native_tool(native_tool("read_file"));
        let unknown = core
            .call_tool(
                &McpToolIdentity::Native {
                    name: "missing".to_string(),
                },
                json!({}),
            )
            .await
            .expect("unknown tool should map to MCP result");
        assert!(unknown.is_error);
        assert!(unknown.content[0]["text"]
            .as_str()
            .expect("text should be present")
            .contains("Unknown tool"));

        let validation_tool: Arc<dyn Tool> = Arc::new(AgentTool::new(
            "validate",
            "validate",
            AgentToolParameters::empty(),
            |_args, _ctx| async { Err(RociError::InvalidArgument("bad args".into())) },
        ));
        let runtime_tool: Arc<dyn Tool> = Arc::new(AgentTool::new(
            "runtime",
            "runtime",
            AgentToolParameters::empty(),
            |_args, _ctx| async {
                Err(RociError::ToolExecution {
                    tool_name: "runtime".into(),
                    message: "failed".into(),
                })
            },
        ));
        let cancel_tool: Arc<dyn Tool> = Arc::new(AgentTool::new(
            "cancel",
            "cancel",
            AgentToolParameters::empty(),
            |_args, _ctx| async { Err(RociError::Stream("cancel requested".into())) },
        ));
        let core = McpServerCore::new()
            .with_native_tool(validation_tool)
            .with_native_tool(runtime_tool)
            .with_native_tool(cancel_tool);

        let validation = core
            .call_tool(
                &McpToolIdentity::Native {
                    name: "validate".into(),
                },
                json!({}),
            )
            .await
            .expect("validation should map");
        assert!(validation.is_error);
        assert_eq!(validation.content[0]["text"], "bad args");

        let runtime = core
            .call_tool(
                &McpToolIdentity::Native {
                    name: "runtime".into(),
                },
                json!({}),
            )
            .await
            .expect("runtime should map");
        assert!(runtime.is_error);
        assert!(runtime.content[0]["text"]
            .as_str()
            .expect("runtime message should be text")
            .contains("failed"));

        let cancel = core
            .call_tool(
                &McpToolIdentity::Native {
                    name: "cancel".into(),
                },
                json!({}),
            )
            .await
            .expect("cancel should map");
        assert!(cancel.is_error);
        assert!(cancel.content[0]["text"]
            .as_str()
            .expect("cancel message should be text")
            .contains("canceled"));
    }
}
