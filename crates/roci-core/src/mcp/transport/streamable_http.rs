use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::service::{ClientInitializeError, ServiceExt};
use rmcp::transport::{
    common::client_side_sse::SseRetryPolicy,
    streamable_http_client::StreamableHttpClientTransportConfig, StreamableHttpClientTransport,
};

use super::common::{DynRoleClientTransport, ErasedRoleClientTransport};
use crate::error::RociError;
use crate::mcp::elicitation::MCPClientHandler;

use super::{MCPRemoteReconnectPolicy, MCPRunningService, MCPTransport};

pub type StreamableHttpAuthHeaderProvider = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Public Streamable HTTP transport configuration.
#[derive(Clone, Default)]
pub struct StreamableHttpTransportConfig {
    pub url: String,
    pub headers: HashMap<String, String>,
    pub auth_token: Option<String>,
    pub auth_header_provider: Option<StreamableHttpAuthHeaderProvider>,
    pub request_timeout_ms: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
    pub reconnect_policy: MCPRemoteReconnectPolicy,
}

/// Streamable HTTP MCP transport (for remote MCP servers).
pub struct StreamableHttpTransport {
    url: String,
    auth_token: Option<String>,
    custom_headers: HashMap<String, String>,
    request_timeout_ms: Option<u64>,
    connect_timeout_ms: Option<u64>,
    reconnect_policy: MCPRemoteReconnectPolicy,
    inner: Option<Box<dyn DynRoleClientTransport>>,
    closed: bool,
}

impl StreamableHttpTransport {
    /// Create streamable HTTP transport with only URL.
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

    /// Create streamable HTTP transport from public config.
    pub fn from_config(config: StreamableHttpTransportConfig) -> Self {
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

    /// Create streamable HTTP transport with bearer auth token.
    pub fn with_auth_token(url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        Self::new(url).auth_token(auth_token)
    }

    /// Create streamable HTTP transport with custom headers.
    pub fn with_custom_headers<I, K, V>(url: impl Into<String>, headers: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Self::new(url).headers(headers)
    }

    /// Create streamable HTTP transport with optional token and custom headers.
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

    pub fn request_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.request_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn connect_timeout_ms(mut self, timeout_ms: u64) -> Self {
        self.connect_timeout_ms = Some(timeout_ms);
        self
    }

    pub fn retry_policy(mut self, max_attempts: Option<usize>, base_delay_ms: u64) -> Self {
        self.reconnect_policy.max_attempts =
            max_attempts.unwrap_or(MCPRemoteReconnectPolicy::DEFAULT_MAX_ATTEMPTS);
        self.reconnect_policy.initial_backoff_ms = base_delay_ms;
        self
    }

    pub fn reconnect_policy(mut self, policy: MCPRemoteReconnectPolicy) -> Self {
        self.reconnect_policy = policy;
        self
    }

    pub(crate) fn build_rmcp_config(
        &self,
    ) -> Result<StreamableHttpClientTransportConfig, RociError> {
        let mut parsed_headers = HashMap::new();
        for (name, value) in &self.custom_headers {
            let header_name =
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).map_err(|error| {
                    RociError::Configuration(format!(
                        "invalid streamable HTTP header name '{name}': {error}"
                    ))
                })?;
            let header_value = reqwest::header::HeaderValue::from_str(value).map_err(|error| {
                RociError::Configuration(format!(
                    "invalid streamable HTTP header value for '{name}': {error}"
                ))
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
        config.retry_config = Arc::new(JitteredExponentialBackoff::new(self.reconnect_policy));

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

#[derive(Debug, Clone)]
struct JitteredExponentialBackoff {
    policy: MCPRemoteReconnectPolicy,
}

impl JitteredExponentialBackoff {
    fn new(policy: MCPRemoteReconnectPolicy) -> Self {
        Self { policy }
    }
}

impl SseRetryPolicy for JitteredExponentialBackoff {
    fn retry(&self, current_times: usize) -> Option<Duration> {
        self.policy.backoff_delay(current_times)
    }
}

#[async_trait]
impl MCPTransport for StreamableHttpTransport {
    async fn connect(
        &mut self,
        client_handler: MCPClientHandler,
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
        client_handler.into_dyn().serve(transport).await
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
            match self.request_timeout_ms.or(self.connect_timeout_ms) {
                Some(timeout_ms) => {
                    tokio::time::timeout(Duration::from_millis(timeout_ms), inner.close())
                        .await
                        .map_err(|_| RociError::Timeout(timeout_ms))??;
                }
                None => inner.close().await?,
            }
        }
        Ok(())
    }

    fn remote_reconnect_policy(&self) -> Option<MCPRemoteReconnectPolicy> {
        Some(self.reconnect_policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RociError;
    use crate::mcp::client::MCPClient;
    use crate::mcp::transport::test_support::{
        test_client_request, test_server_response, MockInnerTransport,
    };
    use std::sync::atomic::Ordering;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, Request, ResponseTemplate,
    };

    #[test]
    fn streamable_http_constructor_defaults_are_empty() {
        let transport = StreamableHttpTransport::new("http://localhost:3000/mcp");
        assert_eq!(transport.url(), "http://localhost:3000/mcp");
        assert_eq!(transport.auth_token_value(), None);
        assert!(transport.custom_headers().is_empty());
    }

    #[test]
    fn streamable_http_constructor_sets_auth_and_headers() {
        let transport = StreamableHttpTransport::with_auth_and_headers(
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
    fn streamable_http_constructor_sets_timeout_and_retry_controls() {
        let transport = StreamableHttpTransport::new("http://localhost:3000/mcp")
            .request_timeout_ms(5_000)
            .connect_timeout_ms(750)
            .reconnect_policy(MCPRemoteReconnectPolicy {
                max_attempts: 3,
                initial_backoff_ms: 250,
                max_backoff_ms: 2_000,
                backoff_multiplier: 3.0,
                jitter_ratio: 0.25,
                idle_timeout_ms: Some(10_000),
                periodic_reconnect_ms: Some(60_000),
            });

        assert_eq!(transport.request_timeout_ms_value(), Some(5_000));
        assert_eq!(transport.connect_timeout_ms_value(), Some(750));
        assert_eq!(transport.retry_max_attempts(), Some(3));
        assert_eq!(transport.retry_initial_delay_ms(), 250);
        assert_eq!(transport.retry_max_delay_ms(), 2_000);
        assert_eq!(transport.retry_multiplier(), 3.0);
        assert_eq!(transport.retry_jitter_ratio(), 0.25);
        assert_eq!(transport.idle_timeout_ms_value(), Some(10_000));
        assert_eq!(transport.periodic_reconnect_ms_value(), Some(60_000));
    }

    #[test]
    fn streamable_http_build_config_includes_auth_and_headers() {
        let transport = StreamableHttpTransport::new("http://localhost:3000/mcp")
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
    fn streamable_http_build_config_applies_retry_policy_controls() {
        let transport = StreamableHttpTransport::new("http://localhost:3000/mcp").reconnect_policy(
            MCPRemoteReconnectPolicy {
                max_attempts: 2,
                initial_backoff_ms: 100,
                max_backoff_ms: 500,
                backoff_multiplier: 2.0,
                jitter_ratio: 0.0,
                idle_timeout_ms: None,
                periodic_reconnect_ms: None,
            },
        );
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
    fn streamable_http_backoff_jitter_stays_within_configured_bounds() {
        let policy = MCPRemoteReconnectPolicy {
            max_attempts: 12,
            initial_backoff_ms: 100,
            max_backoff_ms: 500,
            backoff_multiplier: 2.0,
            jitter_ratio: 0.25,
            idle_timeout_ms: None,
            periodic_reconnect_ms: None,
        };

        let mut saw_variation = false;
        let first = policy
            .backoff_delay(2)
            .expect("attempt should have delay")
            .as_millis();

        for _ in 0..24 {
            let delay = policy
                .backoff_delay(2)
                .expect("attempt should have delay")
                .as_millis();
            assert!((300..=500).contains(&delay));
            saw_variation |= delay != first;
        }

        assert!(saw_variation, "jitter should randomize retry delay");
    }

    #[test]
    fn streamable_http_from_config_applies_auth_hook_headers_and_timeouts() {
        let transport = StreamableHttpTransport::from_config(StreamableHttpTransportConfig {
            url: "http://localhost:3000/mcp".into(),
            headers: HashMap::from([("x-api-key".into(), "abc123".into())]),
            auth_token: Some("fallback".into()),
            auth_header_provider: Some(Arc::new(|| Some("hook-token".into()))),
            request_timeout_ms: Some(250),
            connect_timeout_ms: Some(100),
            reconnect_policy: MCPRemoteReconnectPolicy {
                max_attempts: 2,
                initial_backoff_ms: 10,
                max_backoff_ms: 100,
                backoff_multiplier: 2.0,
                jitter_ratio: 0.0,
                idle_timeout_ms: Some(1_000),
                periodic_reconnect_ms: Some(5_000),
            },
        });

        assert_eq!(transport.url(), "http://localhost:3000/mcp");
        assert_eq!(transport.auth_token_value(), Some("hook-token"));
        assert_eq!(
            transport.custom_headers().get("x-api-key"),
            Some(&"abc123".into())
        );
        assert_eq!(transport.request_timeout_ms_value(), Some(250));
        assert_eq!(transport.connect_timeout_ms_value(), Some(100));
        assert_eq!(transport.retry_max_attempts(), Some(2));
        assert_eq!(transport.retry_initial_delay_ms(), 10);
        assert_eq!(transport.idle_timeout_ms_value(), Some(1_000));
        assert_eq!(transport.periodic_reconnect_ms_value(), Some(5_000));
    }

    #[test]
    fn streamable_http_build_config_rejects_invalid_header_name() {
        let transport = StreamableHttpTransport::with_custom_headers(
            "http://localhost:3000/mcp",
            [("invalid header name", "value")],
        );
        let error = transport
            .build_rmcp_config()
            .expect_err("invalid header should fail");
        match error {
            RociError::Configuration(message) => {
                assert!(message.contains("invalid streamable HTTP header name"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn streamable_http_close_is_idempotent_without_connection() {
        let mut transport = StreamableHttpTransport::new("http://localhost:3000/mcp");
        assert!(transport.close().await.is_ok());
        assert!(transport.close().await.is_ok());
    }

    #[tokio::test]
    async fn streamable_http_close_behaves_like_cancel_and_is_idempotent() {
        let mut transport = StreamableHttpTransport::new("http://localhost:3000/mcp");
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
    async fn streamable_http_receive_closed_peer_maps_to_deterministic_error() {
        let mut transport = StreamableHttpTransport::new("http://localhost:3000/mcp");
        let (mock, _send_calls, _close_calls) = MockInnerTransport::new(vec![None]);
        transport.inner = Some(Box::new(mock));

        let err = transport
            .receive()
            .await
            .expect_err("closed peer should map to stream error");
        assert!(matches!(err, RociError::Stream(message) if message.contains("closed by peer")));
    }

    #[tokio::test]
    async fn streamable_http_send_and_receive_round_trip_via_inner_transport() {
        let mut transport = StreamableHttpTransport::new("http://localhost:3000/mcp");
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
    async fn streamable_http_request_timeout_is_deterministic() {
        let mut transport =
            StreamableHttpTransport::new("http://localhost:3000/mcp").request_timeout_ms(1);
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
    async fn streamable_http_connect_timeout_applies_on_first_operation() {
        let mut transport =
            StreamableHttpTransport::new("http://localhost:3000/mcp").connect_timeout_ms(1);
        let (mock, _send_calls, _close_calls) =
            MockInnerTransport::with_delays(Vec::new(), Some(25), None);
        transport.inner = Some(Box::new(mock));

        let err = transport
            .send(test_client_request())
            .await
            .expect_err("first send should honor connect timeout");
        assert!(matches!(err, RociError::Timeout(1)));
    }

    fn mcp_result_for(method: &str, id: serde_json::Value) -> serde_json::Value {
        match method {
            "initialize" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": {}, "resources": {} },
                    "serverInfo": { "name": "fixture", "version": "0.1.0" }
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
            "resources/list" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "resources": [{
                        "uri": "file:///fixture.txt",
                        "name": "fixture",
                        "mimeType": "text/plain"
                    }]
                }
            }),
            "resources/read" => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "contents": [{
                        "uri": "file:///fixture.txt",
                        "mimeType": "text/plain",
                        "text": "fixture content"
                    }]
                }
            }),
            other => serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("unknown method {other}") }
            }),
        }
    }

    fn streamable_mcp_response(sse_initialize: bool) -> impl Fn(&Request) -> ResponseTemplate {
        move |request: &Request| {
            let payload: serde_json::Value =
                serde_json::from_slice(&request.body).expect("request body should be JSON");
            let method = payload
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let Some(id) = payload.get("id").cloned() else {
                return ResponseTemplate::new(202);
            };
            let body = mcp_result_for(method, id);
            let response = ResponseTemplate::new(200).insert_header("mcp-session-id", "session-1");
            if sse_initialize && method == "initialize" {
                response.set_body_raw(
                    format!("event: message\ndata: {body}\n\n"),
                    "text/event-stream",
                )
            } else {
                response
                    .insert_header("content-type", "application/json")
                    .set_body_json(body)
            }
        }
    }

    async fn mount_streamable_fixture(server: &MockServer, sse_initialize: bool) {
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(streamable_mcp_response(sse_initialize))
            .mount(server)
            .await;
        Mock::given(method("GET"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(405))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn streamable_http_mcp_client_initialize_list_call_json() {
        let server = MockServer::start().await;
        mount_streamable_fixture(&server, false).await;
        let mut client = MCPClient::new(Box::new(StreamableHttpTransport::new(format!(
            "{}/mcp",
            server.uri()
        ))));

        client.initialize().await.expect("initialize should work");
        let tools = client.list_tools().await.expect("tools should list");
        let call = client
            .call_tool("search", serde_json::json!({}))
            .await
            .expect("tool should call");
        let resources = client
            .list_resources()
            .await
            .expect("resources should list");
        let resource = client
            .read_resource("file:///fixture.txt")
            .await
            .expect("resource should read");

        assert_eq!(tools[0].name, "search");
        assert_eq!(
            call.structured_content,
            Some(serde_json::json!({"ok": true}))
        );
        assert_eq!(resources[0].uri, "file:///fixture.txt");
        assert_eq!(resource.contents.len(), 1);
        match &resource.contents[0] {
            crate::mcp::client::MCPResourceContent::Text { text, uri, .. } => {
                assert_eq!(uri, "file:///fixture.txt");
                assert_eq!(text, "fixture content");
            }
            other => panic!("expected Text resource content, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn streamable_http_mcp_client_initialize_sse_then_list_call() {
        let server = MockServer::start().await;
        mount_streamable_fixture(&server, true).await;
        let mut client = MCPClient::new(Box::new(StreamableHttpTransport::new(format!(
            "{}/mcp",
            server.uri()
        ))));

        client
            .initialize()
            .await
            .expect("SSE initialize should work");
        assert_eq!(
            client
                .list_tools()
                .await
                .expect("tools should list after SSE initialize")[0]
                .name,
            "search"
        );
    }

    #[tokio::test]
    async fn streamable_http_delete_session_on_close() {
        let server = MockServer::start().await;
        mount_streamable_fixture(&server, false).await;
        let _delete_mock = Mock::given(method("DELETE"))
            .and(path("/mcp"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount_as_scoped(&server)
            .await;
        let mut transport =
            StreamableHttpTransport::new(format!("{}/mcp", server.uri())).request_timeout_ms(2_000);

        transport
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": { "name": "fixture", "version": "0.1.0" }
                }
            }))
            .await
            .expect("initialize request should send");
        transport
            .receive()
            .await
            .expect("initialize response should receive");
        transport
            .send(serde_json::json!({
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            }))
            .await
            .expect("initialized notification should send");
        transport
            .close()
            .await
            .expect("close should delete session");
    }

    #[tokio::test]
    async fn streamable_http_unsupported_content_type_errors() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/mcp"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/plain")
                    .set_body_string("nope"),
            )
            .mount(&server)
            .await;
        let mut client = MCPClient::new(Box::new(StreamableHttpTransport::new(format!(
            "{}/mcp",
            server.uri()
        ))));

        let error = client
            .initialize()
            .await
            .expect_err("unsupported content type should fail");
        assert!(error.to_string().to_ascii_lowercase().contains("content"));
    }
}
