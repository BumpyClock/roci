use std::{collections::HashMap, future::Future, sync::Arc, time::Duration};

use async_trait::async_trait;
use futures::{stream::SplitStream, SinkExt, StreamExt};
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    service::{ClientInitializeError, RoleClient, RxJsonRpcMessage, ServiceExt, TxJsonRpcMessage},
    transport::Transport as RmcpTransport,
};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{
        client::IntoClientRequest,
        http::{HeaderName, HeaderValue},
        Message,
    },
    MaybeTlsStream, WebSocketStream,
};

use super::common::DynRoleClientTransport;
use super::{MCPRemoteReconnectPolicy, MCPRunningService, MCPTransport};
use crate::error::RociError;
use crate::mcp::elicitation::MCPClientHandler;

pub type WebSocketAuthHeaderProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Public WebSocket transport configuration.
#[derive(Clone, Default)]
pub struct WebSocketTransportConfig {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub auth_token: Option<String>,
    pub auth_header_provider: Option<WebSocketAuthHeaderProvider>,
    pub request_timeout_ms: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
    pub reconnect_policy: MCPRemoteReconnectPolicy,
}

type ClientWebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;
type ClientWebSocketSink = futures::stream::SplitSink<ClientWebSocket, Message>;

/// WebSocket MCP transport (for remote MCP servers).
pub struct WebSocketTransport {
    url: String,
    auth_token: Option<String>,
    custom_headers: HashMap<String, String>,
    request_timeout_ms: Option<u64>,
    connect_timeout_ms: Option<u64>,
    reconnect_policy: MCPRemoteReconnectPolicy,
    inner: Option<Box<dyn DynRoleClientTransport>>,
    closed: bool,
}

impl WebSocketTransport {
    /// Create WebSocket transport with only URL.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth_token: None,
            custom_headers: HashMap::new(),
            request_timeout_ms: None,
            connect_timeout_ms: None,
            reconnect_policy: MCPRemoteReconnectPolicy::default(),
            inner: None,
            closed: false,
        }
    }

    /// Create WebSocket transport from public config.
    pub fn from_config(config: WebSocketTransportConfig) -> Self {
        let auth_token = config
            .auth_header_provider
            .as_ref()
            .and_then(|provider| provider())
            .or(config.auth_token);

        Self {
            url: config.url,
            auth_token,
            custom_headers: config.headers,
            request_timeout_ms: config.request_timeout_ms,
            connect_timeout_ms: config.connect_timeout_ms,
            reconnect_policy: config.reconnect_policy,
            inner: None,
            closed: false,
        }
    }

    /// Create WebSocket transport with bearer auth token.
    pub fn with_auth_token(url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self::new(url).auth_token(auth_token)
    }

    /// Create WebSocket transport with custom headers.
    pub fn with_custom_headers<I, K, V>(url: impl Into<String>, headers: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self::new(url).headers(headers)
    }

    /// Create WebSocket transport with optional token and custom headers.
    pub fn with_auth_and_headers<I, K, V, T>(
        url: impl Into<String>,
        auth_token: Option<T>,
        headers: I,
    ) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
        T: Into<String>,
    {
        let transport = Self::with_custom_headers(url, headers);
        match auth_token {
            Some(token) => transport.auth_token(token),
            None => transport,
        }
    }

    /// Set bearer token (without "Bearer " prefix).
    pub fn auth_token(mut self, auth_token: impl Into<String>) -> Self {
        self.auth_token = Some(auth_token.into());
        self
    }

    /// Add one custom header.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.custom_headers.insert(name.into(), value.into());
        self
    }

    /// Add many custom headers.
    pub fn headers<I, K, V>(mut self, headers: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.custom_headers.extend(
            headers
                .into_iter()
                .map(|(name, value)| (name.into(), value.into())),
        );
        self
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn auth_token_value(&self) -> Option<&str> {
        self.auth_token.as_deref()
    }

    pub fn custom_headers(&self) -> &HashMap<String, String> {
        &self.custom_headers
    }

    pub fn request_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.request_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn connect_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.connect_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn reconnect_policy(mut self, policy: MCPRemoteReconnectPolicy) -> Self {
        self.reconnect_policy = policy;
        self
    }

    /// Total configured reconnect attempts, including the immediate first try.
    pub fn retry_max_attempts(&self) -> Option<usize> {
        Some(self.reconnect_policy.max_attempts)
    }

    /// First retry sleep in milliseconds after the immediate reconnect try fails.
    pub fn retry_initial_delay_ms(&self) -> u64 {
        self.reconnect_policy.initial_backoff_ms
    }

    /// Maximum retry sleep in milliseconds after multiplier and jitter apply.
    pub fn retry_max_delay_ms(&self) -> u64 {
        self.reconnect_policy.max_backoff_ms
    }

    /// Backoff multiplier between retry sleeps.
    pub fn retry_multiplier(&self) -> f64 {
        self.reconnect_policy.backoff_multiplier
    }

    /// Symmetric jitter ratio applied to retry sleeps.
    pub fn retry_jitter_ratio(&self) -> f64 {
        self.reconnect_policy.jitter_ratio
    }

    /// Idle milliseconds after which the next request reconnects first.
    pub fn idle_timeout_ms_value(&self) -> Option<u64> {
        self.reconnect_policy.idle_timeout_ms
    }

    /// Session-age milliseconds after which the next request reconnects first.
    pub fn periodic_reconnect_ms_value(&self) -> Option<u64> {
        self.reconnect_policy.periodic_reconnect_ms
    }

    fn build_request(
        &self,
    ) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, RociError> {
        Self::build_request_from(&self.url, self.auth_token.as_deref(), &self.custom_headers)
    }

    fn build_request_from(
        url: &str,
        auth_token: Option<&str>,
        custom_headers: &HashMap<String, String>,
    ) -> Result<tokio_tungstenite::tungstenite::handshake::client::Request, RociError> {
        let mut request = url
            .into_client_request()
            .map_err(|error| RociError::Configuration(format!("invalid WebSocket URL: {error}")))?;

        if let Some(auth_token) = auth_token {
            let value =
                HeaderValue::from_str(&format!("Bearer {auth_token}")).map_err(|error| {
                    RociError::Configuration(format!(
                        "invalid WebSocket authorization header: {error}"
                    ))
                })?;
            request
                .headers_mut()
                .insert(HeaderName::from_static("authorization"), value);
        }

        for (name, value) in custom_headers {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                RociError::Configuration(format!("invalid WebSocket header name '{name}': {error}"))
            })?;
            let header_value = HeaderValue::from_str(value).map_err(|error| {
                RociError::Configuration(format!(
                    "invalid WebSocket header value for '{name}': {error}"
                ))
            })?;
            request.headers_mut().insert(header_name, header_value);
        }

        Ok(request)
    }

    async fn connect_request(
        request: tokio_tungstenite::tungstenite::handshake::client::Request,
        connect_timeout_ms: Option<u64>,
    ) -> Result<WebSocketRmcpTransport, RociError> {
        let connect = connect_async(request);
        let (stream, _response) = match connect_timeout_ms {
            Some(timeout_ms) => tokio::time::timeout(Duration::from_millis(timeout_ms), connect)
                .await
                .map_err(|_| RociError::Timeout(timeout_ms))?
                .map_err(|error| RociError::Stream(format!("WebSocket connect failed: {error}")))?,
            None => connect
                .await
                .map_err(|error| RociError::Stream(format!("WebSocket connect failed: {error}")))?,
        };
        Ok(WebSocketRmcpTransport::new(stream))
    }

    async fn ensure_connected(&mut self) -> Result<(), RociError> {
        if self.closed {
            return Err(RociError::Stream("MCP transport closed".into()));
        }
        if self.inner.is_some() {
            return Ok(());
        }

        let request = self.build_request()?;
        let transport = Self::connect_request(request, self.connect_timeout_ms).await?;
        self.inner = Some(Box::new(transport));
        Ok(())
    }

    fn inner_mut(&mut self) -> Result<&mut dyn DynRoleClientTransport, RociError> {
        match self.inner.as_mut() {
            Some(inner) => Ok(inner.as_mut()),
            None => Err(RociError::Stream("MCP transport unavailable".into())),
        }
    }
}

#[async_trait]
impl MCPTransport for WebSocketTransport {
    async fn connect(
        &mut self,
        client_handler: MCPClientHandler,
    ) -> Result<MCPRunningService, ClientInitializeError> {
        if self.closed {
            return Err(ClientInitializeError::ConnectionClosed(
                "MCP transport closed".into(),
            ));
        }

        let request = self
            .build_request()
            .map_err(|error| ClientInitializeError::ConnectionClosed(error.to_string()))?;
        let transport = Self::connect_request(request, self.connect_timeout_ms)
            .await
            .map_err(|error| ClientInitializeError::ConnectionClosed(error.to_string()))?;
        client_handler.into_dyn().serve(transport).await
    }

    async fn send(&mut self, message: serde_json::Value) -> Result<(), RociError> {
        let operation_timeout_ms = self.request_timeout_ms;
        self.ensure_connected().await?;
        let message: ClientJsonRpcMessage = serde_json::from_value(message)?;
        let inner = self.inner_mut()?;

        match operation_timeout_ms {
            Some(timeout_ms) => {
                tokio::time::timeout(Duration::from_millis(timeout_ms), inner.send(message))
                    .await
                    .map_err(|_| RociError::Timeout(timeout_ms))?
            }
            None => inner.send(message).await,
        }
    }

    async fn receive(&mut self) -> Result<serde_json::Value, RociError> {
        let operation_timeout_ms = self.request_timeout_ms;
        self.ensure_connected().await?;
        let inner = self.inner_mut()?;
        let maybe_message = match operation_timeout_ms {
            Some(timeout_ms) => {
                tokio::time::timeout(Duration::from_millis(timeout_ms), inner.receive())
                    .await
                    .map_err(|_| RociError::Timeout(timeout_ms))?
            }
            None => inner.receive().await,
        };
        let message: ServerJsonRpcMessage = maybe_message?
            .ok_or_else(|| RociError::Stream("MCP transport closed by peer".into()))?;
        Ok(serde_json::to_value(message)?)
    }

    async fn close(&mut self) -> Result<(), RociError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        if let Some(mut inner) = self.inner.take() {
            inner.close().await?;
        }
        Ok(())
    }

    fn remote_reconnect_policy(&self) -> Option<MCPRemoteReconnectPolicy> {
        Some(self.reconnect_policy)
    }
}

struct WebSocketRmcpTransport {
    sink: Arc<Mutex<ClientWebSocketSink>>,
    stream: SplitStream<ClientWebSocket>,
    closed: bool,
}

impl WebSocketRmcpTransport {
    fn new(stream: ClientWebSocket) -> Self {
        let (sink, stream) = stream.split();
        Self {
            sink: Arc::new(Mutex::new(sink)),
            stream,
            closed: false,
        }
    }
}

impl RmcpTransport<RoleClient> for WebSocketRmcpTransport {
    type Error = RociError;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let sink = Arc::clone(&self.sink);
        async move {
            let text = serde_json::to_string(&item)?;
            let mut sink = sink.lock().await;
            sink.send(Message::Text(text))
                .await
                .map_err(|error| RociError::Stream(format!("WebSocket send failed: {error}")))
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        match self.receive_value().await {
            Ok(message) => message,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "RmcpTransport::receive failed while reading WebSocket receive_value"
                );
                None
            }
        }
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        let mut sink = self.sink.lock().await;
        sink.close()
            .await
            .map_err(|error| RociError::Stream(format!("WebSocket close failed: {error}")))
    }
}

impl WebSocketRmcpTransport {
    async fn receive_value(&mut self) -> Result<Option<RxJsonRpcMessage<RoleClient>>, RociError> {
        if self.closed {
            return Ok(None);
        }

        while let Some(frame) = self.stream.next().await {
            let frame = frame
                .map_err(|error| RociError::Stream(format!("WebSocket receive failed: {error}")))?;
            match frame {
                Message::Text(text) => {
                    return serde_json::from_str(&text)
                        .map(Some)
                        .map_err(RociError::Serialization);
                }
                Message::Binary(bytes) => {
                    return serde_json::from_slice(&bytes)
                        .map(Some)
                        .map_err(RociError::Serialization);
                }
                Message::Close(_) => {
                    self.closed = true;
                    return Ok(None);
                }
                Message::Ping(_) | Message::Pong(_) | Message::Frame(_) => {}
            }
        }
        self.closed = true;
        Ok(None)
    }
}

#[async_trait]
impl DynRoleClientTransport for WebSocketRmcpTransport {
    async fn send(&mut self, message: TxJsonRpcMessage<RoleClient>) -> Result<(), RociError> {
        RmcpTransport::send(self, message).await
    }

    async fn receive(&mut self) -> Result<Option<RxJsonRpcMessage<RoleClient>>, RociError> {
        self.receive_value().await
    }

    async fn close(&mut self) -> Result<(), RociError> {
        RmcpTransport::close(self).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::client::MCPClient;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;
    use tokio_tungstenite::{
        accept_async, accept_hdr_async,
        tungstenite::{
            handshake::server::{Request as ServerRequest, Response as ServerResponse},
            Message,
        },
    };

    #[test]
    fn websocket_constructor_sets_auth_and_headers() {
        let transport = WebSocketTransport::with_auth_and_headers(
            "ws://localhost:3000/mcp",
            Some("test-token"),
            [("x-api-key", "abc123")],
        );

        assert_eq!(transport.url(), "ws://localhost:3000/mcp");
        assert_eq!(transport.auth_token_value(), Some("test-token"));
        assert_eq!(
            transport.custom_headers().get("x-api-key"),
            Some(&"abc123".to_string())
        );
    }

    #[test]
    fn websocket_build_request_includes_auth_and_headers() {
        let transport = WebSocketTransport::new("ws://localhost:3000/mcp")
            .auth_token("test-token")
            .header("x-api-key", "abc123");

        let request = transport
            .build_request()
            .expect("websocket request should build");

        assert_eq!(
            request
                .headers()
                .get("authorization")
                .and_then(|value| value.to_str().ok()),
            Some("Bearer test-token")
        );
        assert_eq!(
            request
                .headers()
                .get("x-api-key")
                .and_then(|value| value.to_str().ok()),
            Some("abc123")
        );
    }

    #[test]
    fn websocket_build_request_rejects_invalid_header_name() {
        let transport =
            WebSocketTransport::with_custom_headers("ws://localhost:3000/mcp", [("bad name", "x")]);

        let error = transport
            .build_request()
            .expect_err("invalid header should fail");

        assert!(matches!(
            error,
            RociError::Configuration(message)
            if message.contains("invalid WebSocket header name")
        ));
    }

    #[test]
    fn websocket_constructor_sets_reconnect_policy_controls() {
        let transport = WebSocketTransport::new("ws://localhost:3000/mcp").reconnect_policy(
            MCPRemoteReconnectPolicy {
                max_attempts: 4,
                initial_backoff_ms: 50,
                max_backoff_ms: 500,
                backoff_multiplier: 1.5,
                jitter_ratio: 0.1,
                idle_timeout_ms: Some(1_000),
                periodic_reconnect_ms: Some(5_000),
            },
        );

        assert_eq!(transport.retry_max_attempts(), Some(4));
        assert_eq!(transport.retry_initial_delay_ms(), 50);
        assert_eq!(transport.retry_max_delay_ms(), 500);
        assert_eq!(transport.retry_multiplier(), 1.5);
        assert_eq!(transport.retry_jitter_ratio(), 0.1);
        assert_eq!(transport.idle_timeout_ms_value(), Some(1_000));
        assert_eq!(transport.periodic_reconnect_ms_value(), Some(5_000));
    }

    #[test]
    fn websocket_from_config_applies_auth_hook_headers_and_timeouts() {
        let transport = WebSocketTransport::from_config(WebSocketTransportConfig {
            url: "ws://localhost:3000/mcp".into(),
            headers: HashMap::from([("x-api-key".into(), "abc123".into())]),
            auth_token: Some("fallback".into()),
            auth_header_provider: Some(Arc::new(|| Some("hook-token".into()))),
            request_timeout_ms: Some(250),
            connect_timeout_ms: Some(100),
            reconnect_policy: MCPRemoteReconnectPolicy {
                max_attempts: 2,
                initial_backoff_ms: 25,
                max_backoff_ms: 250,
                backoff_multiplier: 2.0,
                jitter_ratio: 0.0,
                idle_timeout_ms: Some(500),
                periodic_reconnect_ms: Some(1_000),
            },
        });

        assert_eq!(transport.url(), "ws://localhost:3000/mcp");
        assert_eq!(transport.auth_token_value(), Some("hook-token"));
        assert_eq!(
            transport.custom_headers().get("x-api-key"),
            Some(&"abc123".to_string())
        );
        assert_eq!(transport.retry_max_attempts(), Some(2));
        assert_eq!(transport.idle_timeout_ms_value(), Some(500));
    }

    #[tokio::test]
    async fn websocket_close_is_idempotent_without_connection() {
        let mut transport = WebSocketTransport::new("ws://localhost:3000/mcp");

        assert!(transport.close().await.is_ok());
        assert!(transport.close().await.is_ok());
    }

    fn mcp_result_for(method: &str, id: serde_json::Value) -> serde_json::Value {
        match method {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "ws-fixture", "version": "0.1.0" }
                }
            }),
            "tools/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "tools": [{
                        "name": "search",
                        "description": "Search docs",
                        "inputSchema": { "type": "object", "properties": {} }
                    }]
                }
            }),
            "tools/call" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{ "type": "text", "text": "done" }],
                    "structuredContent": { "ok": true },
                    "isError": false
                }
            }),
            other => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("unknown method {other}") }
            }),
        }
    }

    async fn spawn_websocket_fixture() -> String {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener addr should exist");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("fixture should accept");
            let mut websocket = accept_async(stream).await.expect("websocket should accept");
            while let Some(frame) = websocket.next().await {
                let frame = frame.expect("frame should read");
                if !frame.is_text() {
                    continue;
                }
                let payload: serde_json::Value =
                    serde_json::from_str(frame.to_text().expect("text frame should decode"))
                        .expect("fixture request should be JSON");
                let Some(id) = payload.get("id").cloned() else {
                    continue;
                };
                let method = payload
                    .get("method")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                websocket
                    .send(Message::Text(mcp_result_for(method, id).to_string()))
                    .await
                    .expect("fixture response should send");
            }
        });
        format!("ws://{addr}/mcp")
    }

    #[tokio::test]
    async fn websocket_mcp_client_initialize_list_call() {
        let url = spawn_websocket_fixture().await;
        let mut client = MCPClient::new(Box::new(WebSocketTransport::new(url)));

        client.initialize().await.expect("initialize should work");
        let tools = client.list_tools().await.expect("tools should list");
        let call = client
            .call_tool("search", serde_json::json!({}))
            .await
            .expect("tool should call");

        assert_eq!(tools[0].name, "search");
        assert_eq!(
            call.structured_content,
            Some(serde_json::json!({"ok": true}))
        );
    }

    #[tokio::test]
    async fn websocket_receive_timeout_is_deterministic() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener addr should exist");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("fixture should accept");
            let _websocket = accept_async(stream).await.expect("websocket should accept");
            tokio::time::sleep(Duration::from_millis(200)).await;
        });
        let mut transport =
            WebSocketTransport::new(format!("ws://{addr}/mcp")).request_timeout_ms(10);

        let err = transport
            .receive()
            .await
            .expect_err("receive should timeout");
        assert!(matches!(err, RociError::Timeout(10)));
    }

    #[tokio::test]
    async fn websocket_malformed_peer_maps_to_serialization_error() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener addr should exist");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("fixture should accept");
            let mut websocket = accept_async(stream).await.expect("websocket should accept");
            websocket
                .send(Message::Text("not-json".into()))
                .await
                .expect("malformed frame should send");
        });
        let mut transport = WebSocketTransport::new(format!("ws://{addr}/mcp"));

        let err = transport
            .receive()
            .await
            .expect_err("malformed peer should fail");
        assert!(matches!(err, RociError::Serialization(_)));
    }

    #[tokio::test]
    async fn websocket_closed_peer_maps_to_stream_error() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener addr should exist");
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("fixture should accept");
            let mut websocket = accept_async(stream).await.expect("websocket should accept");
            websocket.close(None).await.expect("close should send");
        });
        let mut transport = WebSocketTransport::new(format!("ws://{addr}/mcp"));

        let err = transport
            .receive()
            .await
            .expect_err("closed peer should fail");
        assert!(matches!(err, RociError::Stream(message) if message.contains("closed by peer")));
    }

    #[tokio::test]
    #[allow(clippy::result_large_err)]
    async fn websocket_connect_sends_auth_and_headers() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let addr = listener.local_addr().expect("listener addr should exist");
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("fixture should accept");
            let tx = Arc::clone(&tx);
            let mut websocket = accept_hdr_async(
                stream,
                move |request: &ServerRequest, response: ServerResponse| {
                    let auth = request
                        .headers()
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    let trace = request
                        .headers()
                        .get("x-trace-id")
                        .and_then(|value| value.to_str().ok())
                        .map(str::to_owned);
                    if let Some(tx) = tx.lock().expect("sender lock should work").take() {
                        let _ = tx.send((auth, trace));
                    }
                    Ok(response)
                },
            )
            .await
            .expect("websocket should accept");
            websocket.close(None).await.expect("close should send");
        });
        let mut transport = WebSocketTransport::from_config(WebSocketTransportConfig {
            url: format!("ws://{addr}/mcp"),
            headers: HashMap::from([("x-trace-id".into(), "trace-1".into())]),
            auth_token: Some("secret".into()),
            auth_header_provider: None,
            request_timeout_ms: Some(100),
            connect_timeout_ms: Some(100),
            reconnect_policy: MCPRemoteReconnectPolicy::default(),
        });

        let _ = transport.receive().await;
        let (auth, trace) = rx.await.expect("headers should be captured");
        assert_eq!(auth.as_deref(), Some("Bearer secret"));
        assert_eq!(trace.as_deref(), Some("trace-1"));
    }
}
