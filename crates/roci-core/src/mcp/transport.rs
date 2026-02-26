//! MCP transport layer.

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use rmcp::{
    model::{ClientInfo, ClientJsonRpcMessage, ServerJsonRpcMessage},
    service::{
        ClientInitializeError, DynService, RoleClient, RunningService, RxJsonRpcMessage,
        ServiceExt, TxJsonRpcMessage,
    },
    transport::{
        common::client_side_sse::ExponentialBackoff,
        streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
        TokioChildProcess, Transport as RmcpTransport,
    },
};
use tokio::process::Command;

use crate::error::RociError;

pub type DynClientService = Box<dyn DynService<RoleClient>>;
pub type MCPRunningService = RunningService<RoleClient, DynClientService>;

/// Transport trait for MCP communication.
#[async_trait]
pub trait MCPTransport: Send {
    /// Create and initialize a new rmcp running service for this transport.
    async fn connect(
        &mut self,
        client_info: ClientInfo,
    ) -> Result<MCPRunningService, ClientInitializeError>;

    /// Send a JSON-RPC message.
    async fn send(&mut self, message: serde_json::Value) -> Result<(), RociError>;

    /// Receive a JSON-RPC message.
    async fn receive(&mut self) -> Result<serde_json::Value, RociError>;

    /// Close the transport.
    async fn close(&mut self) -> Result<(), RociError>;
}

fn map_transport_error(operation: &'static str, error: impl std::fmt::Display) -> RociError {
    RociError::Provider {
        provider: "mcp".into(),
        message: format!("mcp transport {operation} failed: {error}"),
    }
}

#[async_trait]
trait DynRoleClientTransport: Send {
    async fn send(&mut self, message: TxJsonRpcMessage<RoleClient>) -> Result<(), RociError>;
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>>;
    async fn close(&mut self) -> Result<(), RociError>;
}

struct ErasedRoleClientTransport<T>
where
    T: RmcpTransport<RoleClient> + Send,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    inner: T,
    closed: bool,
}

impl<T> ErasedRoleClientTransport<T>
where
    T: RmcpTransport<RoleClient> + Send,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    fn new(inner: T) -> Self {
        Self {
            inner,
            closed: false,
        }
    }
}

#[async_trait]
impl<T> DynRoleClientTransport for ErasedRoleClientTransport<T>
where
    T: RmcpTransport<RoleClient> + Send,
    T::Error: std::error::Error + Send + Sync + 'static,
{
    async fn send(&mut self, message: TxJsonRpcMessage<RoleClient>) -> Result<(), RociError> {
        if self.closed {
            return Err(RociError::Stream("MCP transport closed".into()));
        }

        RmcpTransport::send(&mut self.inner, message)
            .await
            .map_err(|error| map_transport_error("send", error))
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        if self.closed {
            return None;
        }

        RmcpTransport::receive(&mut self.inner).await
    }

    async fn close(&mut self) -> Result<(), RociError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        RmcpTransport::close(&mut self.inner)
            .await
            .map_err(|error| map_transport_error("close", error))
    }
}

/// Stdio-based MCP transport (for local MCP servers).
pub struct StdioTransport {
    command: String,
    args: Vec<String>,
    inner: Option<Box<dyn DynRoleClientTransport>>,
    closed: bool,
}

impl StdioTransport {
    /// Create a stdio transport from command and args.
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            command: command.into(),
            args,
            inner: None,
            closed: false,
        }
    }

    /// Create a stdio transport from command only.
    pub fn from_command(command: impl Into<String>) -> Self {
        Self::new(command, Vec::new())
    }

    pub fn command(&self) -> &str {
        &self.command
    }

    pub fn args(&self) -> &[String] {
        &self.args
    }

    fn ensure_connected(&mut self) -> Result<(), RociError> {
        if self.closed {
            return Err(RociError::Stream("MCP transport closed".into()));
        }
        if self.inner.is_some() {
            return Ok(());
        }

        let mut command = Command::new(&self.command);
        command.args(&self.args);
        let transport = TokioChildProcess::new(command)?;
        self.inner = Some(Box::new(ErasedRoleClientTransport::new(transport)));
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
impl MCPTransport for StdioTransport {
    async fn connect(
        &mut self,
        client_info: ClientInfo,
    ) -> Result<MCPRunningService, ClientInitializeError> {
        if self.closed {
            return Err(ClientInitializeError::ConnectionClosed(
                "MCP transport closed".into(),
            ));
        }

        let mut command = Command::new(&self.command);
        command.args(&self.args);
        let transport = TokioChildProcess::new(command).map_err(|error| {
            ClientInitializeError::transport::<TokioChildProcess>(error, "spawn stdio transport")
        })?;

        client_info.into_dyn().serve(transport).await
    }

    async fn send(&mut self, message: serde_json::Value) -> Result<(), RociError> {
        self.ensure_connected()?;
        let message: ClientJsonRpcMessage = serde_json::from_value(message)?;
        self.inner_mut()?.send(message).await
    }

    async fn receive(&mut self) -> Result<serde_json::Value, RociError> {
        self.ensure_connected()?;
        let message: ServerJsonRpcMessage = self
            .inner_mut()?
            .receive()
            .await
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
}

/// SSE-based MCP transport (for remote MCP servers).
pub struct SSETransport {
    url: String,
    auth_token: Option<String>,
    custom_headers: HashMap<String, String>,
    request_timeout_ms: Option<u64>,
    connect_timeout_ms: Option<u64>,
    retry_max_attempts: Option<usize>,
    retry_base_delay_ms: u64,
    inner: Option<Box<dyn DynRoleClientTransport>>,
    closed: bool,
}

impl SSETransport {
    /// Create SSE transport with only URL.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            auth_token: None,
            custom_headers: HashMap::new(),
            request_timeout_ms: None,
            connect_timeout_ms: None,
            retry_max_attempts: None,
            retry_base_delay_ms: 1_000,
            inner: None,
            closed: false,
        }
    }

    /// Create SSE transport with bearer auth token.
    pub fn with_auth_token(url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self::new(url).auth_token(auth_token)
    }

    /// Create SSE transport with custom headers.
    pub fn with_custom_headers<I, K, V>(url: impl Into<String>, headers: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self::new(url).headers(headers)
    }

    /// Create SSE transport with optional token and custom headers.
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

    pub fn request_timeout_ms_value(&self) -> Option<u64> {
        self.request_timeout_ms
    }

    pub fn connect_timeout_ms_value(&self) -> Option<u64> {
        self.connect_timeout_ms
    }

    pub fn retry_max_attempts(&self) -> Option<usize> {
        self.retry_max_attempts
    }

    pub fn retry_base_delay_ms(&self) -> u64 {
        self.retry_base_delay_ms
    }

    pub fn request_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.request_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn connect_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.connect_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn retry_policy(mut self, max_attempts: Option<usize>, base_delay_ms: u64) -> Self {
        self.retry_max_attempts = max_attempts;
        self.retry_base_delay_ms = base_delay_ms;
        self
    }

    fn build_rmcp_config(&self) -> Result<StreamableHttpClientTransportConfig, RociError> {
        let mut parsed_headers = HashMap::new();
        for (name, value) in &self.custom_headers {
            let header_name =
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                    RociError::Configuration(format!("invalid SSE header name '{name}': {error}"))
                })?;
            let header_value = reqwest::header::HeaderValue::from_str(value).map_err(|error| {
                RociError::Configuration(format!("invalid SSE header value for '{name}': {error}"))
            })?;
            parsed_headers.insert(header_name, header_value);
        }

        let mut config = StreamableHttpClientTransportConfig::with_uri(self.url.clone());
        if let Some(auth_token) = &self.auth_token {
            config = config.auth_header(auth_token.clone());
        }
        if !parsed_headers.is_empty() {
            config = config.custom_headers(parsed_headers);
        }
        config.retry_config = Arc::new(ExponentialBackoff {
            max_times: self.retry_max_attempts,
            base_duration: Duration::from_millis(self.retry_base_delay_ms),
        });

        Ok(config)
    }

    fn ensure_connected(&mut self) -> Result<(), RociError> {
        if self.closed {
            return Err(RociError::Stream("MCP transport closed".into()));
        }
        if self.inner.is_some() {
            return Ok(());
        }

        let config = self.build_rmcp_config()?;
        let transport = StreamableHttpClientTransport::from_config(config);
        self.inner = Some(Box::new(ErasedRoleClientTransport::new(transport)));
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
impl MCPTransport for SSETransport {
    async fn connect(
        &mut self,
        client_info: ClientInfo,
    ) -> Result<MCPRunningService, ClientInitializeError> {
        if self.closed {
            return Err(ClientInitializeError::ConnectionClosed(
                "MCP transport closed".into(),
            ));
        }

        let config = self
            .build_rmcp_config()
            .map_err(|error| ClientInitializeError::ConnectionClosed(error.to_string()))?;
        let transport = StreamableHttpClientTransport::from_config(config);
        client_info.into_dyn().serve(transport).await
    }

    async fn send(&mut self, message: serde_json::Value) -> Result<(), RociError> {
        let operation_timeout_ms = self.request_timeout_ms.or(self.connect_timeout_ms);
        self.ensure_connected()?;
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
        let operation_timeout_ms = self.request_timeout_ms.or(self.connect_timeout_ms);
        self.ensure_connected()?;
        let inner = self.inner_mut()?;
        let maybe_message = match operation_timeout_ms {
            Some(timeout_ms) => {
                tokio::time::timeout(Duration::from_millis(timeout_ms), inner.receive())
                    .await
                    .map_err(|_| RociError::Timeout(timeout_ms))?
            }
            None => inner.receive().await,
        };
        let message: ServerJsonRpcMessage = maybe_message
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{
        collections::VecDeque,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
    };

    struct MockInnerTransport {
        receive_queue: VecDeque<Option<RxJsonRpcMessage<RoleClient>>>,
        send_delay_ms: Option<u64>,
        receive_delay_ms: Option<u64>,
        send_calls: Arc<AtomicUsize>,
        close_calls: Arc<AtomicUsize>,
    }

    impl MockInnerTransport {
        fn new(
            receive_queue: Vec<Option<RxJsonRpcMessage<RoleClient>>>,
        ) -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
            let send_calls = Arc::new(AtomicUsize::new(0));
            let close_calls = Arc::new(AtomicUsize::new(0));
            (
                Self {
                    receive_queue: receive_queue.into(),
                    send_delay_ms: None,
                    receive_delay_ms: None,
                    send_calls: Arc::clone(&send_calls),
                    close_calls: Arc::clone(&close_calls),
                },
                send_calls,
                close_calls,
            )
        }

        fn with_delays(
            receive_queue: Vec<Option<RxJsonRpcMessage<RoleClient>>>,
            send_delay_ms: Option<u64>,
            receive_delay_ms: Option<u64>,
        ) -> (Self, Arc<AtomicUsize>, Arc<AtomicUsize>) {
            let (mut mock, send_calls, close_calls) = Self::new(receive_queue);
            mock.send_delay_ms = send_delay_ms;
            mock.receive_delay_ms = receive_delay_ms;
            (mock, send_calls, close_calls)
        }
    }

    #[async_trait]
    impl DynRoleClientTransport for MockInnerTransport {
        async fn send(&mut self, _message: TxJsonRpcMessage<RoleClient>) -> Result<(), RociError> {
            if let Some(delay_ms) = self.send_delay_ms {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            self.send_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
            if let Some(delay_ms) = self.receive_delay_ms {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
            self.receive_queue.pop_front().unwrap_or(None)
        }

        async fn close(&mut self) -> Result<(), RociError> {
            self.close_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn test_client_request() -> serde_json::Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        })
    }

    fn test_server_response() -> RxJsonRpcMessage<RoleClient> {
        serde_json::from_value(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "tools": []
            }
        }))
        .expect("test server response should deserialize")
    }

    #[test]
    fn stdio_constructor_keeps_command_and_args() {
        let transport = StdioTransport::new("node", vec!["server.js".into(), "--debug".into()]);
        assert_eq!(transport.command(), "node");
        assert_eq!(
            transport.args(),
            &["server.js".to_string(), "--debug".to_string()]
        );
    }

    #[tokio::test]
    async fn stdio_close_is_idempotent_without_connection() {
        let mut transport = StdioTransport::from_command("node");
        assert!(transport.close().await.is_ok());
        assert!(transport.close().await.is_ok());
    }

    #[tokio::test]
    async fn stdio_close_behaves_like_cancel_and_is_idempotent() {
        let mut transport = StdioTransport::from_command("node");
        let (mock, _send_calls, close_calls) = MockInnerTransport::new(Vec::new());
        transport.inner = Some(Box::new(mock));

        assert!(transport.close().await.is_ok());
        assert!(transport.close().await.is_ok());
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);

        let err = transport
            .send(test_client_request())
            .await
            .expect_err("send should fail after close/cancel");
        assert!(matches!(err, RociError::Stream(message) if message.contains("closed")));
    }

    #[tokio::test]
    async fn stdio_receive_closed_peer_maps_to_deterministic_error() {
        let mut transport = StdioTransport::from_command("node");
        let (mock, _send_calls, _close_calls) = MockInnerTransport::new(vec![None]);
        transport.inner = Some(Box::new(mock));

        let err = transport
            .receive()
            .await
            .expect_err("closed peer should map to stream error");
        assert!(matches!(err, RociError::Stream(message) if message.contains("closed by peer")));
    }

    #[tokio::test]
    async fn stdio_send_and_receive_round_trip_via_inner_transport() {
        let mut transport = StdioTransport::from_command("node");
        let (mock, send_calls, _close_calls) =
            MockInnerTransport::new(vec![Some(test_server_response())]);
        transport.inner = Some(Box::new(mock));

        transport
            .send(test_client_request())
            .await
            .expect("send should pass through");
        assert_eq!(send_calls.load(Ordering::SeqCst), 1);

        let received = transport.receive().await.expect("receive should succeed");
        assert_eq!(received["jsonrpc"], "2.0");
        assert_eq!(received["id"], 1);
    }

    #[test]
    fn sse_constructor_defaults_are_empty() {
        let transport = SSETransport::new("http://localhost:3000/mcp");
        assert_eq!(transport.url(), "http://localhost:3000/mcp");
        assert_eq!(transport.auth_token_value(), None);
        assert!(transport.custom_headers().is_empty());
    }

    #[test]
    fn sse_constructor_sets_auth_and_headers() {
        let transport = SSETransport::with_auth_and_headers(
            "http://localhost:3000/mcp",
            Some("test-token"),
            [("x-api-key", "abc123"), ("x-trace-id", "trace-1")],
        );
        assert_eq!(transport.auth_token_value(), Some("test-token"));
        assert_eq!(transport.custom_headers().len(), 2);
        assert_eq!(
            transport.custom_headers().get("x-api-key"),
            Some(&"abc123".into())
        );
        assert_eq!(
            transport.custom_headers().get("x-trace-id"),
            Some(&"trace-1".into())
        );
    }

    #[test]
    fn sse_constructor_sets_timeout_and_retry_controls() {
        let transport = SSETransport::new("http://localhost:3000/mcp")
            .request_timeout_ms(5_000)
            .connect_timeout_ms(750)
            .retry_policy(Some(3), 250);

        assert_eq!(transport.request_timeout_ms_value(), Some(5_000));
        assert_eq!(transport.connect_timeout_ms_value(), Some(750));
        assert_eq!(transport.retry_max_attempts(), Some(3));
        assert_eq!(transport.retry_base_delay_ms(), 250);
    }

    #[test]
    fn sse_build_config_includes_auth_and_headers() {
        let transport = SSETransport::new("http://localhost:3000/mcp")
            .auth_token("test-token")
            .header("x-api-key", "abc123");
        let config = transport.build_rmcp_config().expect("config should build");

        assert_eq!(config.uri.as_ref(), "http://localhost:3000/mcp");
        assert_eq!(config.auth_header.as_deref(), Some("test-token"));
        assert_eq!(config.custom_headers.len(), 1);
        assert_eq!(
            config
                .custom_headers
                .get(&reqwest::header::HeaderName::from_static("x-api-key"))
                .expect("x-api-key header should exist")
                .to_str()
                .expect("header should be utf-8"),
            "abc123"
        );
    }

    #[test]
    fn sse_build_config_applies_retry_policy_controls() {
        let transport = SSETransport::new("http://localhost:3000/mcp").retry_policy(Some(2), 100);
        let config = transport.build_rmcp_config().expect("config should build");

        assert_eq!(
            config.retry_config.retry(0),
            Some(Duration::from_millis(100))
        );
        assert_eq!(
            config.retry_config.retry(1),
            Some(Duration::from_millis(200))
        );
        assert_eq!(config.retry_config.retry(2), None);
    }

    #[test]
    fn sse_build_config_rejects_invalid_header_name() {
        let transport = SSETransport::with_custom_headers(
            "http://localhost:3000/mcp",
            [("invalid header name", "value")],
        );
        let error = transport
            .build_rmcp_config()
            .expect_err("invalid header should fail");
        match error {
            RociError::Configuration(message) => {
                assert!(message.contains("invalid SSE header name"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn sse_close_is_idempotent_without_connection() {
        let mut transport = SSETransport::new("http://localhost:3000/mcp");
        assert!(transport.close().await.is_ok());
        assert!(transport.close().await.is_ok());
    }

    #[tokio::test]
    async fn sse_close_behaves_like_cancel_and_is_idempotent() {
        let mut transport = SSETransport::new("http://localhost:3000/mcp");
        let (mock, _send_calls, close_calls) = MockInnerTransport::new(Vec::new());
        transport.inner = Some(Box::new(mock));

        assert!(transport.close().await.is_ok());
        assert!(transport.close().await.is_ok());
        assert_eq!(close_calls.load(Ordering::SeqCst), 1);

        let err = transport
            .send(test_client_request())
            .await
            .expect_err("send should fail after close/cancel");
        assert!(matches!(err, RociError::Stream(message) if message.contains("closed")));
    }

    #[tokio::test]
    async fn sse_receive_closed_peer_maps_to_deterministic_error() {
        let mut transport = SSETransport::new("http://localhost:3000/mcp");
        let (mock, _send_calls, _close_calls) = MockInnerTransport::new(vec![None]);
        transport.inner = Some(Box::new(mock));

        let err = transport
            .receive()
            .await
            .expect_err("closed peer should map to stream error");
        assert!(matches!(err, RociError::Stream(message) if message.contains("closed by peer")));
    }

    #[tokio::test]
    async fn sse_send_and_receive_round_trip_via_inner_transport() {
        let mut transport = SSETransport::new("http://localhost:3000/mcp");
        let (mock, send_calls, _close_calls) =
            MockInnerTransport::new(vec![Some(test_server_response())]);
        transport.inner = Some(Box::new(mock));

        transport
            .send(test_client_request())
            .await
            .expect("send should pass through");
        assert_eq!(send_calls.load(Ordering::SeqCst), 1);

        let received = transport.receive().await.expect("receive should succeed");
        assert_eq!(received["jsonrpc"], "2.0");
        assert_eq!(received["id"], 1);
    }

    #[tokio::test]
    async fn sse_request_timeout_is_deterministic() {
        let mut transport = SSETransport::new("http://localhost:3000/mcp").request_timeout_ms(1);
        let (mock, _send_calls, _close_calls) =
            MockInnerTransport::with_delays(Vec::new(), Some(25), None);
        transport.inner = Some(Box::new(mock));

        let err = transport
            .send(test_client_request())
            .await
            .expect_err("delayed send should timeout");
        assert!(matches!(err, RociError::Timeout(1)));
    }

    #[tokio::test]
    async fn sse_connect_timeout_applies_on_first_operation() {
        let mut transport = SSETransport::new("http://localhost:3000/mcp").connect_timeout_ms(1);
        let (mock, _send_calls, _close_calls) =
            MockInnerTransport::with_delays(Vec::new(), Some(25), None);
        transport.inner = Some(Box::new(mock));

        let err = transport
            .send(test_client_request())
            .await
            .expect_err("first send should honor connect timeout");
        assert!(matches!(err, RociError::Timeout(1)));
    }
}
