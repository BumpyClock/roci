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

use crate::mcp::transport::{MCPRemoteReconnectPolicy, MCPRunningService, MCPTransport};

enum MockSessionBehavior {
    DisconnectOnListTools,
    DisconnectOnListResources,
    DisconnectOnCallTool,
    SessionExpiredOnListTools,
    AuthOnListTools,
    ListTools { tool_name: String },
    ListResources { resource_name: String },
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
            tx.send(item)
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "mock rmcp channel closed"))
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
                | (MockSessionBehavior::DisconnectOnListResources, "resources/list")
                | (MockSessionBehavior::DisconnectOnCallTool, "tools/call") => {
                    return;
                }
                (MockSessionBehavior::SessionExpiredOnListTools, "tools/list") => {
                    let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let response: ServerJsonRpcMessage = serde_json::from_value(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32000,
                            "message": "session expired"
                        }
                    }))
                    .expect("mock session expired error should deserialize");
                    let _ = inbound_tx.send(response);
                }
                (MockSessionBehavior::AuthOnListTools, "tools/list") => {
                    let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let response: ServerJsonRpcMessage = serde_json::from_value(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32001,
                            "message": "unauthorized token"
                        }
                    }))
                    .expect("mock auth error should deserialize");
                    let _ = inbound_tx.send(response);
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
                (MockSessionBehavior::ListResources { resource_name }, "resources/list") => {
                    let id = value.get("id").cloned().unwrap_or(serde_json::Value::Null);
                    let response: ServerJsonRpcMessage = serde_json::from_value(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "resources": [
                                {
                                    "uri": format!("file:///{resource_name}.txt"),
                                    "name": resource_name,
                                    "mimeType": "text/plain"
                                }
                            ],
                            "nextCursor": null
                        }
                    }))
                    .expect("mock resources/list response should deserialize");
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
    reconnect_policy: Option<MCPRemoteReconnectPolicy>,
}

impl MockBootstrapTransport {
    fn new(connect_results: Vec<Result<MCPRunningService, ClientInitializeError>>) -> Self {
        Self {
            connect_results: connect_results.into(),
            attempted_protocols: Arc::new(Mutex::new(Vec::new())),
            reconnect_policy: None,
        }
    }

    fn attempted_protocols(&self) -> Arc<Mutex<Vec<ProtocolVersion>>> {
        Arc::clone(&self.attempted_protocols)
    }

    fn with_reconnect_policy(mut self, policy: MCPRemoteReconnectPolicy) -> Self {
        self.reconnect_policy = Some(policy);
        self
    }
}

#[async_trait]
impl MCPTransport for MockBootstrapTransport {
    #[allow(clippy::result_large_err)]
    async fn connect(
        &mut self,
        client_handler: MCPClientHandler,
    ) -> Result<MCPRunningService, ClientInitializeError> {
        self.attempted_protocols
            .lock()
            .expect("protocol mutex should lock")
            .push(client_handler.client_info().protocol_version.clone());

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

    fn remote_reconnect_policy(&self) -> Option<MCPRemoteReconnectPolicy> {
        self.reconnect_policy
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

#[path = "client_reconnect_tests.rs"]
mod reconnect_tests;

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
    let init_error = ClientInitializeError::JsonRpcError(rmcp::model::ErrorData::invalid_request(
        "bad initialize payload",
        None,
    ));
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
