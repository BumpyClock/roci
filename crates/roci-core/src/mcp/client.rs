//! MCP client for connecting to MCP servers.

use crate::error::RociError;
use rmcp::{
    model::{CallToolRequestParams, CallToolResult, JsonObject, ProtocolVersion},
    service::{ClientInitializeError, ServiceError},
};

use super::error::{map_client_initialize_error, map_service_error};
use super::mapping::{coerce_tool_arguments, map_call_result, map_mcp_tool_schema};
use super::transport::MCPTransport;

pub type MCPRunningService = super::transport::MCPRunningService;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MCPConnectionState {
    Disconnected,
    Connected,
    Initialized,
    Closed,
}

#[derive(Debug, Clone)]
pub struct MCPToolCallResult {
    pub structured_content: Option<serde_json::Value>,
    pub text_content: Option<String>,
    pub content: Vec<serde_json::Value>,
}

impl MCPToolCallResult {
    pub fn into_value_or_text(self) -> serde_json::Value {
        if let Some(structured) = self.structured_content {
            return structured;
        }
        if let Some(text) = self.text_content {
            return serde_json::Value::String(text);
        }
        serde_json::Value::Array(self.content)
    }
}

/// Client for a Model Context Protocol server.
pub struct MCPClient {
    transport: Option<Box<dyn MCPTransport>>,
    session: Option<MCPRunningService>,
    state: MCPConnectionState,
}

impl MCPClient {
    /// Create a new MCP client with the given transport.
    pub fn new(transport: Box<dyn MCPTransport>) -> Self {
        Self {
            transport: Some(transport),
            session: None,
            state: MCPConnectionState::Disconnected,
        }
    }

    /// Create a client from an already-running rmcp service.
    ///
    /// Initialization handshake is already handled by rmcp `serve(...)`.
    pub fn from_running_service(session: MCPRunningService) -> Self {
        Self {
            transport: None,
            session: Some(session),
            state: MCPConnectionState::Connected,
        }
    }

    /// Convert an rmcp initialization result into an MCP client.
    pub fn from_running_service_result(
        result: Result<MCPRunningService, ClientInitializeError>,
    ) -> Result<Self, RociError> {
        result
            .map(Self::from_running_service)
            .map_err(map_client_initialize_error)
    }

    /// Attach/replace the active rmcp session.
    pub fn attach_running_service(&mut self, session: MCPRunningService) {
        self.session = Some(session);
        self.state = MCPConnectionState::Connected;
    }

    pub fn connection_state(&self) -> MCPConnectionState {
        self.state
    }

    pub fn is_initialized(&self) -> bool {
        self.state == MCPConnectionState::Initialized
    }

    /// Return server-provided instructions when available.
    pub fn instructions(&self) -> Result<Option<String>, RociError> {
        self.ensure_initialized()?;
        Ok(self
            .session
            .as_ref()
            .and_then(|session| session.peer_info())
            .and_then(|info| info.instructions.clone()))
    }

    /// Initialize the MCP connection.
    pub async fn initialize(&mut self) -> Result<(), RociError> {
        if let Some(session) = self.session.as_ref() {
            if session.is_closed() {
                self.session = None;
                self.state = MCPConnectionState::Closed;
                if self.transport.is_none() {
                    return Err(RociError::Stream("MCP session is closed".into()));
                }
                self.state = MCPConnectionState::Disconnected;
            } else {
                self.state = MCPConnectionState::Initialized;
                return Ok(());
            }
        }

        if self.session.is_none() {
            let session = self.connect_with_protocol_fallback().await?;
            self.session = Some(session);
        }

        self.state = MCPConnectionState::Initialized;
        Ok(())
    }

    /// List available tools from the MCP server.
    pub async fn list_tools(&mut self) -> Result<Vec<super::schema::MCPToolSchema>, RociError> {
        self.ensure_initialized()?;

        let tools = match self.list_tools_from_active_session().await {
            Ok(tools) => tools,
            Err(error) if Self::should_reconnect_after_service_error(&error) => {
                self.reset_for_reconnect()?;
                self.initialize().await?;
                self.list_tools_from_active_session()
                    .await
                    .map_err(|retry_error| map_service_error("list_tools", retry_error))?
            }
            Err(error) => return Err(map_service_error("list_tools", error)),
        };

        Ok(tools.into_iter().map(map_mcp_tool_schema).collect())
    }

    /// Execute a tool on the MCP server.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<MCPToolCallResult, RociError> {
        self.ensure_initialized()?;
        let arguments = coerce_tool_arguments(arguments)?;

        let result = match self
            .call_tool_from_active_session(name, arguments.clone())
            .await
        {
            Ok(result) => result,
            Err(error) if Self::should_reconnect_after_service_error(&error) => {
                self.reset_for_reconnect()?;
                self.initialize().await?;
                self.call_tool_from_active_session(name, arguments)
                    .await
                    .map_err(|retry_error| map_service_error("call_tool", retry_error))?
            }
            Err(error) => return Err(map_service_error("call_tool", error)),
        };

        map_call_result(name, result)
    }

    fn ensure_initialized(&self) -> Result<(), RociError> {
        match self.state {
            MCPConnectionState::Initialized => Ok(()),
            MCPConnectionState::Closed => Err(RociError::Stream("MCP session is closed".into())),
            _ => Err(RociError::UnsupportedOperation(
                "MCP client must be initialized first".into(),
            )),
        }
    }

    async fn connect_with_protocol_fallback(&mut self) -> Result<MCPRunningService, RociError> {
        let transport = self
            .transport
            .as_mut()
            .ok_or_else(|| RociError::Configuration("Missing MCP session".into()))?;

        let latest_client_info = rmcp::model::ClientInfo {
            protocol_version: ProtocolVersion::LATEST,
            ..Default::default()
        };

        match transport.connect(latest_client_info).await {
            Ok(session) => return Ok(session),
            Err(error) if Self::should_retry_protocol_fallback(&error) => {}
            Err(error) => return Err(map_client_initialize_error(error)),
        }

        let fallback_client_info = rmcp::model::ClientInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            ..Default::default()
        };
        transport
            .connect(fallback_client_info)
            .await
            .map_err(map_client_initialize_error)
    }

    async fn list_tools_from_active_session(
        &mut self,
    ) -> Result<Vec<rmcp::model::Tool>, ServiceError> {
        let session = self.session.as_mut().ok_or(ServiceError::TransportClosed)?;

        match session.list_all_tools().await {
            Ok(tools) => Ok(tools),
            Err(ServiceError::UnexpectedResponse) => {
                session.list_tools(None).await.map(|page| page.tools)
            }
            Err(error) => Err(error),
        }
    }

    async fn call_tool_from_active_session(
        &mut self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, ServiceError> {
        let session = self.session.as_mut().ok_or(ServiceError::TransportClosed)?;

        session
            .call_tool(CallToolRequestParams {
                meta: None,
                name: name.to_owned().into(),
                arguments,
                task: None,
            })
            .await
    }

    fn reset_for_reconnect(&mut self) -> Result<(), RociError> {
        if self.transport.is_none() {
            self.state = MCPConnectionState::Closed;
            return Err(RociError::Stream("MCP session is closed".into()));
        }

        self.session = None;
        self.state = MCPConnectionState::Disconnected;
        Ok(())
    }

    fn should_reconnect_after_service_error(error: &ServiceError) -> bool {
        matches!(
            error,
            ServiceError::TransportClosed
                | ServiceError::TransportSend(_)
                | ServiceError::Cancelled { .. }
        )
    }

    fn should_retry_protocol_fallback(error: &ClientInitializeError) -> bool {
        match error {
            ClientInitializeError::JsonRpcError(error) => {
                let message = error.message.to_ascii_lowercase();
                message.contains("protocol") && message.contains("version")
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use rmcp::{
        model::ServerJsonRpcMessage,
        service::{serve_directly, RoleClient, RxJsonRpcMessage, ServiceExt, TxJsonRpcMessage},
        transport::Transport as RmcpTransport,
    };
    use serde_json::json;
    use std::{
        collections::VecDeque,
        io,
        sync::{Arc, Mutex},
    };
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

    use crate::mcp::transport::{MCPRunningService, MCPTransport};

    enum MockSessionBehavior {
        DisconnectOnListTools,
        DisconnectOnCallTool,
        ListTools { tool_name: String },
        CallTool,
    }

    struct ChannelRmcpTransport {
        outbound: UnboundedSender<TxJsonRpcMessage<RoleClient>>,
        inbound: UnboundedReceiver<RxJsonRpcMessage<RoleClient>>,
    }

    impl ChannelRmcpTransport {
        fn new(
            outbound: UnboundedSender<TxJsonRpcMessage<RoleClient>>,
            inbound: UnboundedReceiver<RxJsonRpcMessage<RoleClient>>,
        ) -> Self {
            Self { outbound, inbound }
        }
    }

    impl RmcpTransport<RoleClient> for ChannelRmcpTransport {
        type Error = io::Error;

        fn send(
            &mut self,
            item: TxJsonRpcMessage<RoleClient>,
        ) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send + 'static {
            let tx = self.outbound.clone();
            async move {
                tx.send(item).map_err(|_| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "mock rmcp channel closed")
                })
            }
        }

        async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
            self.inbound.recv().await
        }

        fn close(&mut self) -> impl std::future::Future<Output = Result<(), Self::Error>> + Send {
            self.inbound.close();
            std::future::ready(Ok(()))
        }
    }

    fn scripted_running_service(behavior: MockSessionBehavior) -> MCPRunningService {
        let (outbound_tx, mut outbound_rx) = unbounded_channel::<TxJsonRpcMessage<RoleClient>>();
        let (inbound_tx, inbound_rx) = unbounded_channel::<RxJsonRpcMessage<RoleClient>>();
        let transport = ChannelRmcpTransport::new(outbound_tx, inbound_rx);

        tokio::spawn(async move {
            while let Some(message) = outbound_rx.recv().await {
                let value = match serde_json::to_value(message) {
                    Ok(value) => value,
                    Err(_) => continue,
                };

                let Some(method) = value.get("method").and_then(|m| m.as_str()) else {
                    continue;
                };

                match (&behavior, method) {
                    (MockSessionBehavior::DisconnectOnListTools, "tools/list")
                    | (MockSessionBehavior::DisconnectOnCallTool, "tools/call") => {
                        return;
                    }
                    (MockSessionBehavior::ListTools { tool_name }, "tools/list") => {
                        let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
                        let response: ServerJsonRpcMessage = serde_json::from_value(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "tools": [
                                    {
                                        "name": tool_name,
                                        "description": "mock tool",
                                        "inputSchema": { "type": "object", "properties": {} }
                                    }
                                ],
                                "nextCursor": null
                            }
                        }))
                        .expect("mock tools/list response should deserialize");
                        let _ = inbound_tx.send(response);
                    }
                    (MockSessionBehavior::CallTool, "tools/call") => {
                        let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
                        let response: ServerJsonRpcMessage = serde_json::from_value(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [
                                    { "type": "text", "text": "tool ok" }
                                ],
                                "structuredContent": { "ok": true },
                                "isError": false
                            }
                        }))
                        .expect("mock tools/call response should deserialize");
                        let _ = inbound_tx.send(response);
                    }
                    _ => {}
                }
            }
        });

        serve_directly(().into_dyn(), transport, None)
    }

    struct MockBootstrapTransport {
        connect_results: VecDeque<Result<MCPRunningService, ClientInitializeError>>,
        attempted_protocols: Arc<Mutex<Vec<ProtocolVersion>>>,
    }

    impl MockBootstrapTransport {
        fn new(connect_results: Vec<Result<MCPRunningService, ClientInitializeError>>) -> Self {
            Self {
                connect_results: connect_results.into(),
                attempted_protocols: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn attempted_protocols(&self) -> Arc<Mutex<Vec<ProtocolVersion>>> {
            Arc::clone(&self.attempted_protocols)
        }
    }

    #[async_trait]
    impl MCPTransport for MockBootstrapTransport {
        #[allow(clippy::result_large_err)]
        async fn connect(
            &mut self,
            client_info: rmcp::model::ClientInfo,
        ) -> Result<MCPRunningService, ClientInitializeError> {
            self.attempted_protocols
                .lock()
                .expect("protocol mutex should lock")
                .push(client_info.protocol_version);

            self.connect_results.pop_front().unwrap_or_else(|| {
                Err(ClientInitializeError::ConnectionClosed(
                    "missing mock connect result".into(),
                ))
            })
        }

        async fn send(&mut self, _message: serde_json::Value) -> Result<(), RociError> {
            Ok(())
        }

        async fn receive(&mut self) -> Result<serde_json::Value, RociError> {
            Ok(serde_json::Value::Null)
        }

        async fn close(&mut self) -> Result<(), RociError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn initialize_bootstraps_session_from_transport() {
        let transport = MockBootstrapTransport::new(vec![Ok(scripted_running_service(
            MockSessionBehavior::ListTools {
                tool_name: "weather".into(),
            },
        ))]);
        let attempted = transport.attempted_protocols();
        let mut client = MCPClient::new(Box::new(transport));

        client
            .initialize()
            .await
            .expect("initialize should bootstrap from transport");

        assert!(client.is_initialized());
        let attempted = attempted.lock().expect("protocol mutex should lock");
        assert_eq!(attempted.as_slice(), &[ProtocolVersion::LATEST]);
    }

    #[tokio::test]
    async fn initialize_with_attached_running_service_sets_initialized_state() {
        let session = scripted_running_service(MockSessionBehavior::ListTools {
            tool_name: "weather".into(),
        });
        let mut client = MCPClient::from_running_service(session);

        client
            .initialize()
            .await
            .expect("initialize should accept attached running service");
        assert!(client.is_initialized());
    }

    #[tokio::test]
    async fn initialize_falls_back_to_legacy_protocol_version() {
        let transport = MockBootstrapTransport::new(vec![
            Err(ClientInitializeError::JsonRpcError(
                rmcp::model::ErrorData::invalid_request("unsupported protocol version", None),
            )),
            Ok(scripted_running_service(MockSessionBehavior::ListTools {
                tool_name: "weather".into(),
            })),
        ]);
        let attempted = transport.attempted_protocols();
        let mut client = MCPClient::new(Box::new(transport));

        client
            .initialize()
            .await
            .expect("initialize should retry with fallback protocol");

        let attempted = attempted.lock().expect("protocol mutex should lock");
        assert_eq!(attempted.len(), 2);
        assert_eq!(attempted[0], ProtocolVersion::LATEST);
        assert_eq!(attempted[1], ProtocolVersion::V_2024_11_05);
    }

    #[tokio::test]
    async fn list_tools_works_after_transport_bootstrap() {
        let transport = MockBootstrapTransport::new(vec![Ok(scripted_running_service(
            MockSessionBehavior::ListTools {
                tool_name: "weather".into(),
            },
        ))]);
        let mut client = MCPClient::new(Box::new(transport));

        client
            .initialize()
            .await
            .expect("initialize should succeed");
        let tools = client
            .list_tools()
            .await
            .expect("list_tools should succeed after initialize");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "weather");
    }

    #[tokio::test]
    async fn call_tool_works_after_transport_bootstrap() {
        let transport = MockBootstrapTransport::new(vec![Ok(scripted_running_service(
            MockSessionBehavior::CallTool,
        ))]);
        let mut client = MCPClient::new(Box::new(transport));

        client
            .initialize()
            .await
            .expect("initialize should succeed");
        let result = client
            .call_tool("echo", json!({"message": "hello"}))
            .await
            .expect("call_tool should succeed after initialize");

        assert_eq!(result.structured_content, Some(json!({"ok": true})));
        assert_eq!(result.text_content.as_deref(), Some("tool ok"));
    }

    #[tokio::test]
    async fn list_tools_reconnects_when_session_disconnects() {
        let transport = MockBootstrapTransport::new(vec![
            Ok(scripted_running_service(
                MockSessionBehavior::DisconnectOnListTools,
            )),
            Ok(scripted_running_service(MockSessionBehavior::ListTools {
                tool_name: "weather".into(),
            })),
        ]);
        let attempted = transport.attempted_protocols();
        let mut client = MCPClient::new(Box::new(transport));

        client
            .initialize()
            .await
            .expect("initialize should succeed");
        let tools = client
            .list_tools()
            .await
            .expect("list_tools should reconnect and retry");

        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "weather");

        let attempted = attempted.lock().expect("protocol mutex should lock");
        assert_eq!(attempted.len(), 2);
    }

    #[tokio::test]
    async fn call_tool_reconnects_when_session_disconnects() {
        let transport = MockBootstrapTransport::new(vec![
            Ok(scripted_running_service(
                MockSessionBehavior::DisconnectOnCallTool,
            )),
            Ok(scripted_running_service(MockSessionBehavior::CallTool)),
        ]);
        let attempted = transport.attempted_protocols();
        let mut client = MCPClient::new(Box::new(transport));

        client
            .initialize()
            .await
            .expect("initialize should succeed");
        let result = client
            .call_tool("echo", json!({"message": "hello"}))
            .await
            .expect("call_tool should reconnect and retry");

        assert_eq!(result.structured_content, Some(json!({"ok": true})));

        let attempted = attempted.lock().expect("protocol mutex should lock");
        assert_eq!(attempted.len(), 2);
    }

    #[test]
    fn reconnect_predicate_treats_cancelled_as_transient() {
        let cancelled = ServiceError::Cancelled {
            reason: Some("transport dropped".into()),
        };
        assert!(MCPClient::should_reconnect_after_service_error(&cancelled));
    }

    #[tokio::test]
    async fn list_tools_requires_initialize() {
        let mut client = MCPClient::new(Box::new(MockBootstrapTransport::new(Vec::new())));
        let err = client
            .list_tools()
            .await
            .expect_err("listing tools should require initialize");
        assert!(matches!(err, RociError::UnsupportedOperation(_)));
    }

    #[test]
    fn from_running_service_result_maps_jsonrpc_initialize_error() {
        let init_error = ClientInitializeError::JsonRpcError(
            rmcp::model::ErrorData::invalid_request("bad initialize payload", None),
        );
        let err = match MCPClient::from_running_service_result(Err(init_error)) {
            Ok(_) => panic!("initialize error should be mapped"),
            Err(err) => err,
        };
        assert!(matches!(
            err,
            RociError::Provider { provider, message }
            if provider == "mcp"
                && message.contains("JSON-RPC error")
                && message.contains("bad initialize payload")
        ));
    }
}
