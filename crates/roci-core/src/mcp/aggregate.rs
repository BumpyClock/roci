//! Multi-server MCP aggregation with deterministic routing.

use std::collections::{HashMap, HashSet};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;

use crate::error::RociError;
use crate::tools::arguments::ToolArguments;
use crate::tools::dynamic::{DynamicTool, DynamicToolProvider};
use crate::tools::tool::ToolExecutionContext;
use crate::tools::types::AgentToolParameters;

use super::client::MCPClient;
use super::client_ops::MCPClientOps;
use super::instructions::{MCPInstructionSource, MCPServerMetadata};
use super::server::McpToolIdentity;

/// Tool naming policy used while merging tools across MCP servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MCPCollisionPolicy {
    /// Reject exposed-name collisions.
    #[default]
    DenyOnCollision,
    /// Resolve exposed-name collisions with a deterministic short SHA-256 suffix.
    SuffixOnCollision {
        /// Lower-hex hash length used after the `__h` suffix marker.
        hash_len: usize,
    },
}

/// MCP multi-server initialization behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MCPAggregateInitPolicy {
    /// Stop immediately on first initialize/list failure.
    #[default]
    StrictFailFast,
}

/// Aggregation behavior controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MCPAggregationConfig {
    pub collision_policy: MCPCollisionPolicy,
    pub init_policy: MCPAggregateInitPolicy,
}

impl Default for MCPAggregationConfig {
    fn default() -> Self {
        Self {
            collision_policy: MCPCollisionPolicy::DenyOnCollision,
            init_policy: MCPAggregateInitPolicy::StrictFailFast,
        }
    }
}

/// Registration payload for one MCP server.
pub struct MCPAggregateServer {
    metadata: MCPServerMetadata,
    client: Box<dyn MCPClientOps>,
}

impl MCPAggregateServer {
    /// Register an MCP server using only a server id.
    pub fn new(server_id: impl Into<String>, client: MCPClient) -> Self {
        Self::with_metadata(MCPServerMetadata::new(server_id), client)
    }

    /// Register an MCP server with a display label.
    pub fn new_with_label(
        server_id: impl Into<String>,
        label: impl Into<String>,
        client: MCPClient,
    ) -> Self {
        Self::with_metadata(MCPServerMetadata::with_label(server_id, label), client)
    }

    /// Register an MCP server with full metadata.
    pub fn with_metadata(mut metadata: MCPServerMetadata, client: MCPClient) -> Self {
        metadata.id = metadata.id.trim().to_owned();
        let client = client.with_server_id(metadata.id.clone());
        Self {
            metadata,
            client: Box::new(client),
        }
    }

    #[cfg(test)]
    fn from_client_ops(server_id: impl Into<String>, client: Box<dyn MCPClientOps>) -> Self {
        Self {
            metadata: MCPServerMetadata::new(server_id),
            client,
        }
    }

    #[cfg(test)]
    fn from_client_ops_with_label(
        server_id: impl Into<String>,
        label: impl Into<String>,
        client: Box<dyn MCPClientOps>,
    ) -> Self {
        Self {
            metadata: MCPServerMetadata::with_label(server_id, label),
            client,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MCPToolRoute {
    pub server_id: String,
    pub server_label: Option<String>,
    pub upstream_tool_name: String,
    pub identity: McpToolIdentity,
}

#[derive(Debug, Clone)]
pub struct MCPAggregatedTool {
    pub exposed_name: String,
    pub identity: McpToolIdentity,
    pub server_id: String,
    pub server_label: Option<String>,
    pub upstream_tool_name: String,
    pub description: String,
    pub parameters: AgentToolParameters,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MCPAggregatedResource {
    pub identity: super::instructions::MCPResourceIdentity,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub mime_type: Option<String>,
    pub size: Option<u32>,
    pub server_label: Option<String>,
}

struct MCPServerEntry {
    metadata: MCPServerMetadata,
    client: Mutex<Box<dyn MCPClientOps>>,
}

/// Aggregates multiple MCP servers behind deterministic tool routing.
pub struct MCPToolAggregator {
    servers: Vec<MCPServerEntry>,
    server_index_by_id: HashMap<String, usize>,
    routes: Mutex<HashMap<String, MCPToolRoute>>,
    config: MCPAggregationConfig,
}

impl MCPToolAggregator {
    pub fn new(servers: Vec<MCPAggregateServer>) -> Result<Self, RociError> {
        Self::with_config(servers, MCPAggregationConfig::default())
    }

    pub fn with_config(
        servers: Vec<MCPAggregateServer>,
        config: MCPAggregationConfig,
    ) -> Result<Self, RociError> {
        let mut entries = Vec::with_capacity(servers.len());
        let mut index = HashMap::with_capacity(servers.len());

        for (position, server) in servers.into_iter().enumerate() {
            let mut metadata = server.metadata;
            let normalized_id = metadata.id.trim().to_owned();
            if normalized_id.is_empty() {
                return Err(RociError::Configuration(
                    "MCP server id must not be empty".into(),
                ));
            }
            if index.insert(normalized_id.clone(), position).is_some() {
                return Err(RociError::Configuration(format!(
                    "Duplicate MCP server id '{normalized_id}'"
                )));
            }
            metadata.id = normalized_id;
            entries.push(MCPServerEntry {
                metadata,
                client: Mutex::new(server.client),
            });
        }

        Ok(Self {
            servers: entries,
            server_index_by_id: index,
            routes: Mutex::new(HashMap::new()),
            config,
        })
    }

    pub async fn list_tools_with_origin(&self) -> Result<Vec<MCPAggregatedTool>, RociError> {
        struct PendingTool {
            exposed_name: String,
            route: MCPToolRoute,
            tool: MCPAggregatedTool,
        }

        let mut pending_tools = Vec::new();

        for server in &self.servers {
            let mut client = server.client.lock().await;
            self.initialize_client(&mut **client).await?;
            let tools = client.list_tools().await?;

            for tool in tools {
                let upstream_tool_name = tool.name;
                let exposed_name =
                    Self::base_exposed_name(&server.metadata.id, &upstream_tool_name);
                let identity = McpToolIdentity::Mcp {
                    server_id: server.metadata.id.clone(),
                    tool_name: upstream_tool_name.clone(),
                };
                let route = MCPToolRoute {
                    server_id: server.metadata.id.clone(),
                    server_label: server.metadata.label.clone(),
                    upstream_tool_name: upstream_tool_name.clone(),
                    identity: identity.clone(),
                };
                let tool = MCPAggregatedTool {
                    exposed_name: exposed_name.clone(),
                    identity,
                    server_id: server.metadata.id.clone(),
                    server_label: server.metadata.label.clone(),
                    upstream_tool_name,
                    description: tool.description.unwrap_or_default(),
                    parameters: AgentToolParameters::from_schema(tool.input_schema),
                };
                pending_tools.push(PendingTool {
                    exposed_name,
                    route,
                    tool,
                });
            }
        }

        let mut merged_tools = Vec::with_capacity(pending_tools.len());
        let mut routing_map = HashMap::with_capacity(pending_tools.len());
        let mut used_names = std::collections::HashSet::with_capacity(pending_tools.len());
        for mut pending in pending_tools {
            if used_names.contains(&pending.exposed_name) {
                pending.exposed_name = self.resolve_collision_name(
                    &pending.exposed_name,
                    &pending.route.server_id,
                    &pending.route.upstream_tool_name,
                )?;
                pending.tool.exposed_name = pending.exposed_name.clone();
            }

            if !used_names.insert(pending.exposed_name.clone()) {
                return Err(RociError::InvalidState(format!(
                    "Duplicate aggregated MCP tool name '{}'",
                    pending.exposed_name
                )));
            }

            if routing_map
                .insert(pending.exposed_name.clone(), pending.route)
                .is_some()
            {
                return Err(RociError::InvalidState(format!(
                    "Duplicate aggregated MCP tool name '{}'",
                    pending.exposed_name
                )));
            }
            merged_tools.push(pending.tool);
        }

        let mut routes = self.routes.lock().await;
        *routes = routing_map;
        merged_tools.sort_by(|left, right| left.exposed_name.cmp(&right.exposed_name));
        Ok(merged_tools)
    }

    pub async fn execute_routed_tool(
        &self,
        exposed_tool_name: &str,
        args: &ToolArguments,
        _ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        let route = self.route_for(exposed_tool_name).await.ok_or_else(|| {
            RociError::InvalidArgument(format!("Unknown aggregated MCP tool '{exposed_tool_name}'"))
        })?;

        let server_idx = self
            .server_index_by_id
            .get(&route.server_id)
            .copied()
            .ok_or_else(|| {
                RociError::InvalidState(format!(
                    "Routing points to missing MCP server '{}'",
                    route.server_id
                ))
            })?;

        let mut client = self.servers[server_idx].client.lock().await;
        self.initialize_client(&mut **client).await?;
        let result = client
            .call_tool(&route.upstream_tool_name, args.raw().clone())
            .await?;
        Ok(result.into_value_or_text())
    }

    /// Return server metadata in deterministic order.
    pub fn list_server_metadata(&self) -> Vec<MCPServerMetadata> {
        let mut metadata = self
            .servers
            .iter()
            .map(|entry| entry.metadata.clone())
            .collect::<Vec<_>>();
        metadata.sort_by(|left, right| left.id.cmp(&right.id));
        metadata
    }

    /// Return instruction sources for all servers.
    pub async fn list_instruction_sources(&self) -> Result<Vec<MCPInstructionSource>, RociError> {
        let mut sources = Vec::new();
        for server in &self.servers {
            let mut client = server.client.lock().await;
            self.initialize_client(&mut **client).await?;
            if let Some(instructions) = client.instructions().await? {
                if instructions.trim().is_empty() {
                    continue;
                }
                sources.push(MCPInstructionSource {
                    server: server.metadata.clone(),
                    instructions,
                });
            }
        }
        sources.sort_by(|left, right| left.server.id.cmp(&right.server.id));
        Ok(sources)
    }

    pub async fn route_for(&self, exposed_tool_name: &str) -> Option<MCPToolRoute> {
        self.routes.lock().await.get(exposed_tool_name).cloned()
    }

    pub async fn list_resources(&self) -> Result<Vec<MCPAggregatedResource>, RociError> {
        let mut resources = Vec::new();
        for server in &self.servers {
            let mut client = server.client.lock().await;
            self.initialize_client(&mut **client).await?;
            let mut seen_uris = HashSet::new();
            for resource in client.list_resources().await? {
                if !seen_uris.insert(resource.uri.clone()) {
                    continue;
                }
                resources.push(MCPAggregatedResource {
                    identity: super::instructions::MCPResourceIdentity {
                        server_id: server.metadata.id.clone(),
                        uri: resource.uri,
                    },
                    name: resource.name,
                    title: resource.title,
                    description: resource.description,
                    mime_type: resource.mime_type,
                    size: resource.size,
                    server_label: server.metadata.label.clone(),
                });
            }
        }
        resources.sort_by(|left, right| {
            left.identity
                .server_id
                .cmp(&right.identity.server_id)
                .then_with(|| left.identity.uri.cmp(&right.identity.uri))
        });
        Ok(resources)
    }

    pub async fn read_resource(
        &self,
        identity: &super::instructions::MCPResourceIdentity,
    ) -> Result<super::client::MCPReadResourceResult, RociError> {
        let server_idx = self
            .server_index_by_id
            .get(&identity.server_id)
            .copied()
            .ok_or_else(|| {
                RociError::InvalidArgument(format!("Unknown MCP server '{}'", identity.server_id))
            })?;

        let mut client = self.servers[server_idx].client.lock().await;
        self.initialize_client(&mut **client).await?;
        client.read_resource(&identity.uri).await
    }

    fn base_exposed_name(server_id: &str, tool_name: &str) -> String {
        format!("mcp__{server_id}__{tool_name}")
    }

    fn resolve_collision_name(
        &self,
        base_name: &str,
        server_id: &str,
        tool_name: &str,
    ) -> Result<String, RociError> {
        match self.config.collision_policy {
            MCPCollisionPolicy::DenyOnCollision => Err(RociError::InvalidState(format!(
                "Duplicate aggregated MCP tool name '{base_name}'"
            ))),
            MCPCollisionPolicy::SuffixOnCollision { hash_len } => {
                if hash_len == 0 || hash_len > 64 {
                    return Err(RociError::Configuration(
                        "MCP collision hash length must be between 1 and 64".into(),
                    ));
                }
                let mut hasher = Sha256::new();
                hasher.update(server_id.as_bytes());
                hasher.update([0]);
                hasher.update(tool_name.as_bytes());
                let digest = hasher.finalize();
                let mut hash = String::with_capacity(64);
                for byte in digest {
                    use std::fmt::Write as _;
                    write!(&mut hash, "{byte:02x}").expect("writing to string should not fail");
                }
                Ok(format!("{base_name}__h{}", &hash[..hash_len]))
            }
        }
    }

    async fn initialize_client(&self, client: &mut dyn MCPClientOps) -> Result<(), RociError> {
        match self.config.init_policy {
            MCPAggregateInitPolicy::StrictFailFast => client.initialize().await,
        }
    }
}

#[async_trait]
impl DynamicToolProvider for MCPToolAggregator {
    async fn list_tools(&self) -> Result<Vec<DynamicTool>, RociError> {
        let tools = self.list_tools_with_origin().await?;
        Ok(tools
            .into_iter()
            .map(|tool| DynamicTool {
                name: tool.exposed_name,
                description: tool.description,
                parameters: tool.parameters,
                safety: crate::tools::ToolSafetyPlan::default(),
                safety_summary: crate::tools::ToolSafetySummary::default(),
            })
            .collect())
    }

    async fn execute_tool(
        &self,
        name: &str,
        args: &ToolArguments,
        ctx: &ToolExecutionContext,
    ) -> Result<serde_json::Value, RociError> {
        self.execute_routed_tool(name, args, ctx).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex as StdMutex};

    use async_trait::async_trait;
    use serde_json::json;

    use crate::mcp::client::{MCPReadResourceResult, MCPResourceSchema, MCPToolCallResult};
    use crate::mcp::schema::MCPToolSchema;

    struct MockClientOps {
        initialize_error: Option<String>,
        instructions: Option<String>,
        list_plan: StdMutex<VecDeque<Result<Vec<MCPToolSchema>, String>>>,
        resources: Vec<MCPResourceSchema>,
        resource_reads: HashMap<String, MCPReadResourceResult>,
        resource_read_log: Arc<StdMutex<Vec<String>>>,
        call_results: HashMap<String, serde_json::Value>,
        call_log: Arc<StdMutex<Vec<(String, serde_json::Value)>>>,
        list_calls: Arc<AtomicUsize>,
    }

    type MockCallLog = Arc<StdMutex<Vec<(String, serde_json::Value)>>>;
    type MockResourceReadLog = Arc<StdMutex<Vec<String>>>;
    type MockClientParts = (
        MockClientOps,
        MockCallLog,
        Arc<AtomicUsize>,
        MockResourceReadLog,
    );

    impl MockClientOps {
        fn new(
            list_plan: Vec<Result<Vec<MCPToolSchema>, String>>,
            call_results: HashMap<String, serde_json::Value>,
        ) -> MockClientParts {
            let call_log = Arc::new(StdMutex::new(Vec::new()));
            let resource_read_log = Arc::new(StdMutex::new(Vec::new()));
            let list_calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    initialize_error: None,
                    instructions: None,
                    list_plan: StdMutex::new(list_plan.into()),
                    resources: Vec::new(),
                    resource_reads: HashMap::new(),
                    resource_read_log: Arc::clone(&resource_read_log),
                    call_results,
                    call_log: Arc::clone(&call_log),
                    list_calls: Arc::clone(&list_calls),
                },
                call_log,
                list_calls,
                resource_read_log,
            )
        }

        fn with_instructions(mut self, instructions: impl Into<String>) -> Self {
            self.instructions = Some(instructions.into());
            self
        }

        fn with_resources(
            mut self,
            resources: Vec<MCPResourceSchema>,
            resource_reads: HashMap<String, MCPReadResourceResult>,
        ) -> Self {
            self.resources = resources;
            self.resource_reads = resource_reads;
            self
        }
    }

    #[async_trait]
    impl MCPClientOps for MockClientOps {
        async fn initialize(&mut self) -> Result<(), RociError> {
            match &self.initialize_error {
                Some(message) => Err(RociError::Provider {
                    provider: "mcp".into(),
                    message: message.clone(),
                }),
                None => Ok(()),
            }
        }

        async fn list_tools(&mut self) -> Result<Vec<MCPToolSchema>, RociError> {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            let mut list_plan = self
                .list_plan
                .lock()
                .expect("list_plan lock should not be poisoned");

            match list_plan.pop_front() {
                Some(Ok(tools)) => Ok(tools),
                Some(Err(message)) => Err(RociError::Provider {
                    provider: "mcp".into(),
                    message,
                }),
                None => Ok(Vec::new()),
            }
        }

        async fn instructions(&mut self) -> Result<Option<String>, RociError> {
            Ok(self.instructions.clone())
        }

        async fn list_resources(&mut self) -> Result<Vec<MCPResourceSchema>, RociError> {
            Ok(self.resources.clone())
        }

        async fn read_resource(&mut self, uri: &str) -> Result<MCPReadResourceResult, RociError> {
            self.resource_read_log
                .lock()
                .expect("resource_read_log lock should not be poisoned")
                .push(uri.to_owned());
            self.resource_reads
                .get(uri)
                .cloned()
                .ok_or_else(|| RociError::InvalidArgument(format!("Unknown resource '{uri}'")))
        }

        async fn call_tool(
            &mut self,
            name: &str,
            arguments: serde_json::Value,
        ) -> Result<MCPToolCallResult, RociError> {
            self.call_log
                .lock()
                .expect("call_log lock should not be poisoned")
                .push((name.to_owned(), arguments));

            let result = self
                .call_results
                .get(name)
                .ok_or_else(|| RociError::ToolExecution {
                    tool_name: name.to_owned(),
                    message: "missing mock tool call result".into(),
                })?
                .clone();

            Ok(MCPToolCallResult {
                structured_content: Some(result),
                text_content: None,
                content: Vec::new(),
            })
        }
    }

    fn test_tool(name: &str) -> MCPToolSchema {
        MCPToolSchema {
            name: name.into(),
            description: Some(format!("{name} description")),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "q": { "type": "string" }
                }
            }),
        }
    }

    #[test]
    fn new_rejects_duplicate_server_ids() {
        let (first_client, _first_calls, _first_list_calls, _first_resource_reads) =
            MockClientOps::new(vec![Ok(Vec::new())], HashMap::new());
        let (second_client, _second_calls, _second_list_calls, _second_resource_reads) =
            MockClientOps::new(vec![Ok(Vec::new())], HashMap::new());

        let result = MCPToolAggregator::new(vec![
            MCPAggregateServer::from_client_ops("dup", Box::new(first_client)),
            MCPAggregateServer::from_client_ops("dup", Box::new(second_client)),
        ]);
        assert!(result.is_err());
        let err = result.err().expect("duplicate server ids must fail");

        assert!(matches!(
            err,
            RociError::Configuration(message)
            if message.contains("Duplicate MCP server id")
        ));
    }

    #[tokio::test]
    async fn list_tools_uses_mcp_server_prefix() {
        let (alpha_client, _alpha_calls, _alpha_list_calls, _alpha_resource_reads) =
            MockClientOps::new(
                vec![Ok(vec![test_tool("search")])],
                HashMap::from([(String::from("search"), json!({"server": "alpha"}))]),
            );
        let (beta_client, _beta_calls, _beta_list_calls, _beta_resource_reads) = MockClientOps::new(
            vec![Ok(vec![test_tool("search")])],
            HashMap::from([(String::from("search"), json!({"server": "beta"}))]),
        );

        let aggregator = MCPToolAggregator::new(vec![
            MCPAggregateServer::from_client_ops("alpha", Box::new(alpha_client)),
            MCPAggregateServer::from_client_ops("beta", Box::new(beta_client)),
        ])
        .expect("aggregator should construct");

        let tools = aggregator
            .list_tools_with_origin()
            .await
            .expect("listing should succeed");

        assert_eq!(tools.len(), 2);
        assert!(tools.iter().any(|tool| {
            tool.exposed_name == "mcp__alpha__search"
                && tool.identity
                    == (McpToolIdentity::Mcp {
                        server_id: "alpha".into(),
                        tool_name: "search".into(),
                    })
                && tool.server_id == "alpha"
                && tool.upstream_tool_name == "search"
        }));
        assert!(tools.iter().any(|tool| {
            tool.exposed_name == "mcp__beta__search"
                && tool.server_id == "beta"
                && tool.upstream_tool_name == "search"
        }));
    }

    #[tokio::test]
    async fn list_tools_denies_real_exposed_name_collision_by_default() {
        let (alpha_client, _alpha_calls, _alpha_list_calls, _alpha_resource_reads) =
            MockClientOps::new(vec![Ok(vec![test_tool("beta__search")])], HashMap::new());
        let (beta_client, _beta_calls, _beta_list_calls, _beta_resource_reads) =
            MockClientOps::new(vec![Ok(vec![test_tool("search")])], HashMap::new());

        let aggregator = MCPToolAggregator::new(vec![
            MCPAggregateServer::from_client_ops("alpha", Box::new(alpha_client)),
            MCPAggregateServer::from_client_ops("alpha__beta", Box::new(beta_client)),
        ])
        .expect("aggregator should construct");

        let err = aggregator
            .list_tools_with_origin()
            .await
            .expect_err("base exposed-name collision should fail");

        assert!(matches!(
            err,
            RociError::InvalidState(message)
            if message.contains("mcp__alpha__beta__search")
        ));
    }

    #[tokio::test]
    async fn list_tools_suffix_policy_resolves_real_exposed_name_collision() {
        let (alpha_client, _alpha_calls, _alpha_list_calls, _alpha_resource_reads) =
            MockClientOps::new(
                vec![Ok(vec![test_tool("beta__search")])],
                HashMap::from([(String::from("beta__search"), json!({"server": "alpha"}))]),
            );
        let (beta_client, _beta_calls, _beta_list_calls, _beta_resource_reads) = MockClientOps::new(
            vec![Ok(vec![test_tool("search")])],
            HashMap::from([(String::from("search"), json!({"server": "alpha__beta"}))]),
        );

        let aggregator = MCPToolAggregator::with_config(
            vec![
                MCPAggregateServer::from_client_ops("alpha", Box::new(alpha_client)),
                MCPAggregateServer::from_client_ops("alpha__beta", Box::new(beta_client)),
            ],
            MCPAggregationConfig {
                collision_policy: MCPCollisionPolicy::SuffixOnCollision { hash_len: 8 },
                init_policy: MCPAggregateInitPolicy::StrictFailFast,
            },
        )
        .expect("aggregator should construct");

        let tools = aggregator
            .list_tools_with_origin()
            .await
            .expect("suffix policy should resolve collision");

        assert_eq!(tools.len(), 2);
        assert!(tools
            .iter()
            .any(|tool| tool.exposed_name == "mcp__alpha__beta__search"));
        assert!(tools
            .iter()
            .any(|tool| tool.exposed_name == "mcp__alpha__beta__search__h7ada8d14"));
    }

    #[tokio::test]
    async fn list_tools_suffix_policy_uses_exact_12_hex_hash() {
        let (alpha_client, _alpha_calls, _alpha_list_calls, _alpha_resource_reads) =
            MockClientOps::new(vec![Ok(vec![test_tool("beta__search")])], HashMap::new());
        let (beta_client, _beta_calls, _beta_list_calls, _beta_resource_reads) =
            MockClientOps::new(vec![Ok(vec![test_tool("search")])], HashMap::new());

        let aggregator = MCPToolAggregator::with_config(
            vec![
                MCPAggregateServer::from_client_ops("alpha", Box::new(alpha_client)),
                MCPAggregateServer::from_client_ops("alpha__beta", Box::new(beta_client)),
            ],
            MCPAggregationConfig {
                collision_policy: MCPCollisionPolicy::SuffixOnCollision { hash_len: 12 },
                init_policy: MCPAggregateInitPolicy::StrictFailFast,
            },
        )
        .expect("aggregator should construct");

        let tools = aggregator
            .list_tools_with_origin()
            .await
            .expect("suffix policy should resolve collision");

        assert_eq!(
            tools
                .iter()
                .map(|tool| tool.exposed_name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "mcp__alpha__beta__search",
                "mcp__alpha__beta__search__h7ada8d14725b",
            ]
        );
    }

    #[tokio::test]
    async fn execute_tool_routes_to_correct_server_and_upstream_name() {
        let (alpha_client, alpha_calls, _alpha_list_calls, _alpha_resource_reads) =
            MockClientOps::new(
                vec![Ok(vec![test_tool("search")])],
                HashMap::from([(String::from("search"), json!({"server": "alpha"}))]),
            );
        let (beta_client, beta_calls, _beta_list_calls, _beta_resource_reads) = MockClientOps::new(
            vec![Ok(vec![test_tool("search")])],
            HashMap::from([(String::from("search"), json!({"server": "beta"}))]),
        );

        let aggregator = MCPToolAggregator::new(vec![
            MCPAggregateServer::from_client_ops("alpha", Box::new(alpha_client)),
            MCPAggregateServer::from_client_ops("beta", Box::new(beta_client)),
        ])
        .expect("aggregator should construct");

        aggregator
            .list_tools_with_origin()
            .await
            .expect("listing should populate routes");

        let alpha_result = aggregator
            .execute_routed_tool(
                "mcp__alpha__search",
                &ToolArguments::new(json!({"q":"rust"})),
                &ToolExecutionContext::default(),
            )
            .await
            .expect("alpha route should execute");
        assert_eq!(alpha_result["server"], "alpha");

        let beta_result = aggregator
            .execute_routed_tool(
                "mcp__beta__search",
                &ToolArguments::new(json!({"q":"rust"})),
                &ToolExecutionContext::default(),
            )
            .await
            .expect("beta route should execute");
        assert_eq!(beta_result["server"], "beta");

        let alpha_calls = alpha_calls
            .lock()
            .expect("alpha call log lock should not be poisoned");
        assert_eq!(alpha_calls.len(), 1);
        assert_eq!(alpha_calls[0].0, "search");
        assert_eq!(alpha_calls[0].1, json!({"q":"rust"}));

        let beta_calls = beta_calls
            .lock()
            .expect("beta call log lock should not be poisoned");
        assert_eq!(beta_calls.len(), 1);
        assert_eq!(beta_calls[0].0, "search");
        assert_eq!(beta_calls[0].1, json!({"q":"rust"}));
    }

    #[tokio::test]
    async fn strict_fail_fast_stops_on_first_failure_and_preserves_previous_routes() {
        let (first_client, _first_calls, first_list_calls, _first_resource_reads) =
            MockClientOps::new(
                vec![
                    Ok(vec![test_tool("search")]),
                    Err("first server failed".into()),
                ],
                HashMap::from([(String::from("search"), json!({"server": "first"}))]),
            );
        let (second_client, _second_calls, second_list_calls, _second_resource_reads) =
            MockClientOps::new(
                vec![Ok(vec![test_tool("stats")]), Ok(vec![test_tool("stats")])],
                HashMap::from([(String::from("stats"), json!({"server": "second"}))]),
            );

        let aggregator = MCPToolAggregator::new(vec![
            MCPAggregateServer::from_client_ops("first", Box::new(first_client)),
            MCPAggregateServer::from_client_ops("second", Box::new(second_client)),
        ])
        .expect("aggregator should construct");

        aggregator
            .list_tools_with_origin()
            .await
            .expect("first list should succeed");

        let err = aggregator
            .list_tools_with_origin()
            .await
            .expect_err("second list should fail fast");
        assert!(matches!(
            err,
            RociError::Provider { provider, message }
            if provider == "mcp" && message.contains("first server failed")
        ));

        assert_eq!(first_list_calls.load(Ordering::SeqCst), 2);
        assert_eq!(second_list_calls.load(Ordering::SeqCst), 1);

        let preserved_route = aggregator
            .route_for("mcp__second__stats")
            .await
            .expect("previous route should remain after failed refresh");
        assert_eq!(preserved_route.server_id, "second");
        assert_eq!(preserved_route.upstream_tool_name, "stats");

        let execution_result = aggregator
            .execute_routed_tool(
                "mcp__second__stats",
                &ToolArguments::new(json!({"q":"ok"})),
                &ToolExecutionContext::default(),
            )
            .await
            .expect("execution should still route via preserved state");
        assert_eq!(execution_result["server"], "second");
    }

    #[tokio::test]
    async fn list_instruction_sources_orders_by_server_id_and_preserves_labels() {
        let (alpha_client, _alpha_calls, _alpha_list_calls, _alpha_resource_reads) =
            MockClientOps::new(vec![Ok(Vec::new())], HashMap::new());
        let (beta_client, _beta_calls, _beta_list_calls, _beta_resource_reads) =
            MockClientOps::new(vec![Ok(Vec::new())], HashMap::new());

        let alpha_client = alpha_client.with_instructions("Alpha instructions");
        let beta_client = beta_client.with_instructions("Beta instructions");

        let aggregator = MCPToolAggregator::new(vec![
            MCPAggregateServer::from_client_ops_with_label(
                "beta",
                "Beta MCP",
                Box::new(beta_client),
            ),
            MCPAggregateServer::from_client_ops_with_label(
                "alpha",
                "Alpha MCP",
                Box::new(alpha_client),
            ),
        ])
        .expect("aggregator should construct");

        let sources = aggregator
            .list_instruction_sources()
            .await
            .expect("instruction sources should load");

        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].server.id, "alpha");
        assert_eq!(sources[0].server.label.as_deref(), Some("Alpha MCP"));
        assert_eq!(sources[0].instructions, "Alpha instructions");
        assert_eq!(sources[1].server.id, "beta");
        assert_eq!(sources[1].server.label.as_deref(), Some("Beta MCP"));
        assert_eq!(sources[1].instructions, "Beta instructions");
    }

    #[tokio::test]
    async fn list_resources_preserves_same_uri_with_different_server_identity() {
        let shared_uri = "file:///shared.md";
        let (alpha_client, _alpha_calls, _alpha_list_calls, _alpha_resource_reads) =
            MockClientOps::new(vec![Ok(Vec::new())], HashMap::new());
        let alpha_client = alpha_client.with_resources(
            vec![MCPResourceSchema {
                uri: shared_uri.into(),
                name: "Shared".into(),
                title: Some("Alpha label".into()),
                description: None,
                mime_type: Some("text/plain".into()),
                size: Some(5),
            }],
            HashMap::new(),
        );
        let (beta_client, _beta_calls, _beta_list_calls, _beta_resource_reads) =
            MockClientOps::new(vec![Ok(Vec::new())], HashMap::new());
        let beta_client = beta_client.with_resources(
            vec![MCPResourceSchema {
                uri: shared_uri.into(),
                name: "Shared".into(),
                title: Some("Beta label".into()),
                description: None,
                mime_type: Some("text/plain".into()),
                size: Some(5),
            }],
            HashMap::new(),
        );

        let aggregator = MCPToolAggregator::new(vec![
            MCPAggregateServer::from_client_ops("alpha", Box::new(alpha_client)),
            MCPAggregateServer::from_client_ops("beta", Box::new(beta_client)),
        ])
        .expect("aggregator should construct");

        let resources = aggregator
            .list_resources()
            .await
            .expect("resources should list");

        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].identity.server_id, "alpha");
        assert_eq!(resources[0].identity.uri, shared_uri);
        assert_eq!(resources[1].identity.server_id, "beta");
        assert_eq!(resources[1].identity.uri, shared_uri);
    }

    #[tokio::test]
    async fn list_resources_skips_duplicate_uri_from_same_server() {
        let shared_uri = "file:///shared.md";
        let (client, _calls, _list_calls, _resource_reads) =
            MockClientOps::new(vec![Ok(Vec::new())], HashMap::new());
        let client = client.with_resources(
            vec![
                MCPResourceSchema {
                    uri: shared_uri.into(),
                    name: "Shared".into(),
                    title: Some("First".into()),
                    description: None,
                    mime_type: Some("text/plain".into()),
                    size: Some(5),
                },
                MCPResourceSchema {
                    uri: shared_uri.into(),
                    name: "Shared duplicate".into(),
                    title: Some("Second".into()),
                    description: None,
                    mime_type: Some("text/plain".into()),
                    size: Some(6),
                },
            ],
            HashMap::new(),
        );

        let aggregator = MCPToolAggregator::new(vec![MCPAggregateServer::from_client_ops(
            "alpha",
            Box::new(client),
        )])
        .expect("aggregator should construct");

        let resources = aggregator
            .list_resources()
            .await
            .expect("resources should list");

        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].identity.server_id, "alpha");
        assert_eq!(resources[0].identity.uri, shared_uri);
        assert_eq!(resources[0].title.as_deref(), Some("First"));
    }

    #[tokio::test]
    async fn read_resource_routes_by_identity_not_label() {
        let (client, _calls, _list_calls, resource_reads) =
            MockClientOps::new(vec![Ok(Vec::new())], HashMap::new());
        let client = client.with_resources(
            Vec::new(),
            HashMap::from([(
                "file:///same.md".to_string(),
                MCPReadResourceResult {
                    contents: vec![json!({
                        "uri": "file:///same.md",
                        "mimeType": "text/plain",
                        "text": "alpha"
                    })],
                },
            )]),
        );

        let aggregator =
            MCPToolAggregator::new(vec![MCPAggregateServer::from_client_ops_with_label(
                "alpha",
                "Initial Label",
                Box::new(client),
            )])
            .expect("aggregator should construct");

        let result = aggregator
            .read_resource(&super::super::instructions::MCPResourceIdentity {
                server_id: "alpha".into(),
                uri: "file:///same.md".into(),
            })
            .await
            .expect("resource should read");

        assert_eq!(result.contents[0]["text"], "alpha");
        assert_eq!(
            resource_reads
                .lock()
                .expect("resource read log lock should not be poisoned")
                .as_slice(),
            &["file:///same.md".to_string()]
        );
    }
}
