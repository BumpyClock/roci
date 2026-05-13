//! MCP client for connecting to MCP servers.

use std::{
    future::Future,
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::error::RociError;
use crate::human_interaction::HumanInteractionCoordinator;
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResult, JsonObject, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResult, Resource,
    },
    service::{ClientInitializeError, ServiceError},
};

use super::elicitation::MCPClientHandler;
use super::error::{map_client_initialize_error, map_service_error};
use super::mapping::{coerce_tool_arguments, map_call_result, map_mcp_tool_schema};
use super::transport::{MCPRemoteReconnectPolicy, MCPTransport};

pub type MCPRunningService = super::transport::MCPRunningService;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MCPConnectionState {
    Disconnected,
    Connected,
    Initialized,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MCPRemoteReconnectOutcome {
    /// Reconnect succeeded and a new session is active.
    Recovered,
    /// Reconnect stopped because credentials or authorization are required.
    NeedsAuth,
    /// Reconnect attempts were exhausted without recovering.
    Failed,
}

#[derive(Debug, Clone)]
pub struct MCPToolCallResult {
    pub structured_content: Option<serde_json::Value>,
    pub text_content: Option<String>,
    pub content: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MCPResourceSchema {
    pub uri: String,
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub mime_type: Option<String>,
    pub size: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MCPReadResourceResult {
    pub contents: Vec<serde_json::Value>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReconnectReplayPolicy {
    Replay,
    DoNotReplayTimeouts,
}

impl ReconnectReplayPolicy {
    fn should_replay(self, error: &ServiceError) -> bool {
        match self {
            Self::Replay => true,
            Self::DoNotReplayTimeouts => !matches!(error, ServiceError::Timeout { .. }),
        }
    }
}

/// Client for a Model Context Protocol server.
pub struct MCPClient {
    transport: Option<Box<dyn MCPTransport>>,
    session: Option<MCPRunningService>,
    state: MCPConnectionState,
    server_id: String,
    human_interaction_coordinator: Option<Arc<HumanInteractionCoordinator>>,
    session_started_at: Option<Instant>,
    last_session_used_at: Option<Instant>,
    last_reconnect_outcome: Option<MCPRemoteReconnectOutcome>,
}

impl MCPClient {
    /// Create a new MCP client with the given transport.
    pub fn new(transport: Box<dyn MCPTransport>) -> Self {
        Self {
            transport: Some(transport),
            session: None,
            state: MCPConnectionState::Disconnected,
            server_id: "mcp".to_string(),
            human_interaction_coordinator: None,
            session_started_at: None,
            last_session_used_at: None,
            last_reconnect_outcome: None,
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
            server_id: "mcp".to_string(),
            human_interaction_coordinator: None,
            session_started_at: None,
            last_session_used_at: None,
            last_reconnect_outcome: None,
        }
    }

    /// Set the MCP server identifier used in human interaction source metadata.
    #[must_use]
    pub fn with_server_id(mut self, server_id: impl Into<String>) -> Self {
        self.server_id = server_id.into();
        self
    }

    /// Enable host-mediated UI elicitation through the shared human interaction coordinator.
    #[must_use]
    pub fn with_human_interaction_coordinator(
        mut self,
        coordinator: Arc<HumanInteractionCoordinator>,
    ) -> Self {
        self.human_interaction_coordinator = Some(coordinator);
        self
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
        self.record_new_session();
    }

    pub fn connection_state(&self) -> MCPConnectionState {
        self.state
    }

    pub fn is_initialized(&self) -> bool {
        self.state == MCPConnectionState::Initialized
    }

    /// Result of the most recent reconnect path, if one has run.
    ///
    /// The value is not reset on every request; it may describe an earlier
    /// reconnect until another reconnect attempt updates it.
    pub fn last_reconnect_outcome(&self) -> Option<MCPRemoteReconnectOutcome> {
        self.last_reconnect_outcome
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
                self.record_session_if_missing();
                return Ok(());
            }
        }

        if self.session.is_none() {
            let session = self.connect_with_protocol_fallback().await?;
            self.session = Some(session);
            self.record_new_session();
        }

        self.state = MCPConnectionState::Initialized;
        Ok(())
    }

    /// List available tools from the MCP server.
    pub async fn list_tools(&mut self) -> Result<Vec<super::schema::MCPToolSchema>, RociError> {
        let tools = self
            .with_reconnect(
                "list_tools",
                |client| Box::pin(async move { client.list_tools_from_active_session().await }),
                ReconnectReplayPolicy::Replay,
            )
            .await?;

        Ok(tools.into_iter().map(map_mcp_tool_schema).collect())
    }

    /// List available resources from the MCP server.
    pub async fn list_resources(&mut self) -> Result<Vec<MCPResourceSchema>, RociError> {
        let resources = self
            .with_reconnect(
                "list_resources",
                |client| Box::pin(async move { client.list_resources_from_active_session().await }),
                ReconnectReplayPolicy::Replay,
            )
            .await?;

        Ok(resources
            .into_iter()
            .map(|resource| MCPResourceSchema {
                uri: resource.raw.uri,
                name: resource.raw.name,
                title: resource.raw.title,
                description: resource.raw.description,
                mime_type: resource.raw.mime_type,
                size: resource.raw.size,
            })
            .collect())
    }

    /// Read one MCP resource by upstream URI.
    pub async fn read_resource(&mut self, uri: &str) -> Result<MCPReadResourceResult, RociError> {
        let uri = uri.to_owned();
        let result = self
            .with_reconnect(
                "read_resource",
                move |client| {
                    let uri = uri.clone();
                    Box::pin(async move { client.read_resource_from_active_session(&uri).await })
                },
                ReconnectReplayPolicy::Replay,
            )
            .await?;

        Ok(MCPReadResourceResult {
            contents: result
                .contents
                .into_iter()
                .map(serde_json::to_value)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    /// Execute a tool on the MCP server.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<MCPToolCallResult, RociError> {
        let arguments = coerce_tool_arguments(arguments)?;
        let tool_name = name.to_owned();
        let result = self
            .with_reconnect(
                "call_tool",
                move |client| {
                    let arguments = arguments.clone();
                    let tool_name = tool_name.clone();
                    Box::pin(async move {
                        client
                            .call_tool_from_active_session(&tool_name, arguments)
                            .await
                    })
                },
                ReconnectReplayPolicy::DoNotReplayTimeouts,
            )
            .await?;

        map_call_result(name, result)
    }

    async fn with_reconnect<T, Op>(
        &mut self,
        context: &'static str,
        mut operation: Op,
        replay_policy: ReconnectReplayPolicy,
    ) -> Result<T, RociError>
    where
        Op: for<'a> FnMut(
            &'a mut MCPClient,
        )
            -> Pin<Box<dyn Future<Output = Result<T, ServiceError>> + Send + 'a>>,
    {
        self.prepare_for_request().await?;
        let max_recoveries = self
            .remote_reconnect_policy()
            .map(|policy| policy.max_attempts)
            .unwrap_or(1);
        let mut recoveries = 0;

        loop {
            match operation(self).await {
                Ok(value) => {
                    self.record_session_used();
                    return Ok(value);
                }
                Err(error)
                    if Self::should_reconnect_after_service_error(&error)
                        && recoveries < max_recoveries =>
                {
                    recoveries += 1;
                    self.recover_after_service_error(context, &error).await?;
                    if !replay_policy.should_replay(&error) {
                        return Err(map_service_error(context, error));
                    }
                }
                Err(error) => {
                    if Self::is_auth_service_error(&error) {
                        self.last_reconnect_outcome = Some(MCPRemoteReconnectOutcome::NeedsAuth);
                        return Err(RociError::Authentication(format!(
                            "{context}: MCP auth required: {error}"
                        )));
                    }
                    if Self::should_reconnect_after_service_error(&error) {
                        self.last_reconnect_outcome = Some(MCPRemoteReconnectOutcome::Failed);
                    }
                    return Err(map_service_error(context, error));
                }
            }
        }
    }

    async fn prepare_for_request(&mut self) -> Result<(), RociError> {
        self.ensure_initialized()?;

        if self.should_reconnect_for_idle_timeout() || self.should_reconnect_for_periodic_policy() {
            self.reconnect_with_policy().await?;
        }

        Ok(())
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
        let latest_client_handler = self.client_handler(ProtocolVersion::LATEST);

        let latest_result = {
            let transport = self
                .transport
                .as_mut()
                .ok_or_else(|| RociError::Configuration("Missing MCP session".into()))?;
            transport.connect(latest_client_handler).await
        };

        match latest_result {
            Ok(session) => return Ok(session),
            Err(error) if Self::should_retry_protocol_fallback(&error) => {}
            Err(error) => return Err(map_client_initialize_error(error)),
        }

        let fallback_client_handler = self.client_handler(ProtocolVersion::V_2024_11_05);
        {
            let transport = self
                .transport
                .as_mut()
                .ok_or_else(|| RociError::Configuration("Missing MCP session".into()))?;
            transport.connect(fallback_client_handler).await
        }
        .map_err(map_client_initialize_error)
    }

    async fn recover_after_service_error(
        &mut self,
        context: &str,
        error: &ServiceError,
    ) -> Result<(), RociError> {
        if Self::is_auth_service_error(error) {
            self.last_reconnect_outcome = Some(MCPRemoteReconnectOutcome::NeedsAuth);
            return Err(RociError::Authentication(format!(
                "{context}: MCP auth required: {error}"
            )));
        }

        self.reconnect_with_policy().await
    }

    async fn reconnect_with_policy(&mut self) -> Result<(), RociError> {
        let Some(policy) = self.remote_reconnect_policy() else {
            self.reset_for_reconnect()?;
            self.initialize().await?;
            self.last_reconnect_outcome = Some(MCPRemoteReconnectOutcome::Recovered);
            return Ok(());
        };

        self.reset_for_reconnect()?;
        let mut last_error = None;

        for attempt in 0..policy.max_attempts {
            if attempt > 0 {
                let delay = policy
                    .backoff_delay(attempt - 1)
                    .unwrap_or_else(|| Duration::from_millis(policy.max_backoff_ms));
                tokio::time::sleep(delay).await;
            }

            match self.initialize().await {
                Ok(()) => {
                    self.last_reconnect_outcome = Some(MCPRemoteReconnectOutcome::Recovered);
                    return Ok(());
                }
                Err(error) if Self::is_auth_roci_error(&error) => {
                    self.last_reconnect_outcome = Some(MCPRemoteReconnectOutcome::NeedsAuth);
                    return Err(error);
                }
                Err(error) => {
                    last_error = Some(error);
                }
            }
        }

        self.last_reconnect_outcome = Some(MCPRemoteReconnectOutcome::Failed);
        Err(last_error.unwrap_or_else(|| {
            RociError::Stream("MCP reconnect failed: retry policy allows no attempts".into())
        }))
    }

    fn client_handler(&self, protocol_version: ProtocolVersion) -> MCPClientHandler {
        let handler = MCPClientHandler::new(protocol_version);
        match &self.human_interaction_coordinator {
            Some(coordinator) => {
                handler.with_ui_elicitation(self.server_id.clone(), Arc::clone(coordinator))
            }
            None => handler,
        }
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

    async fn list_resources_from_active_session(&mut self) -> Result<Vec<Resource>, ServiceError> {
        let session = self.session.as_mut().ok_or(ServiceError::TransportClosed)?;

        match session.list_all_resources().await {
            Ok(resources) => Ok(resources),
            Err(ServiceError::UnexpectedResponse) => session
                .list_resources(None)
                .await
                .map(|page| page.resources),
            Err(error) => Err(error),
        }
    }

    async fn read_resource_from_active_session(
        &mut self,
        uri: &str,
    ) -> Result<ReadResourceResult, ServiceError> {
        let session = self.session.as_mut().ok_or(ServiceError::TransportClosed)?;

        session
            .read_resource(ReadResourceRequestParams {
                meta: None,
                uri: uri.to_owned(),
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
        self.session_started_at = None;
        self.last_session_used_at = None;
        Ok(())
    }

    fn should_reconnect_after_service_error(error: &ServiceError) -> bool {
        if Self::is_auth_service_error(error) {
            return false;
        }
        matches!(
            error,
            ServiceError::TransportClosed
                | ServiceError::TransportSend(_)
                | ServiceError::Cancelled { .. }
                | ServiceError::Timeout { .. }
        ) || Self::is_session_expired_service_error(error)
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

    fn remote_reconnect_policy(&self) -> Option<MCPRemoteReconnectPolicy> {
        self.transport
            .as_ref()
            .and_then(|transport| transport.remote_reconnect_policy())
    }

    fn record_new_session(&mut self) {
        let now = Instant::now();
        self.session_started_at = Some(now);
        self.last_session_used_at = Some(now);
    }

    fn record_session_if_missing(&mut self) {
        if self.session_started_at.is_none() || self.last_session_used_at.is_none() {
            self.record_new_session();
        }
    }

    fn record_session_used(&mut self) {
        self.last_session_used_at = Some(Instant::now());
    }

    fn should_reconnect_for_idle_timeout(&self) -> bool {
        let Some(policy) = self.remote_reconnect_policy() else {
            return false;
        };
        let Some(timeout_ms) = policy.idle_timeout_ms else {
            return false;
        };
        let Some(last_used) = self.last_session_used_at else {
            return false;
        };

        last_used.elapsed() >= Duration::from_millis(timeout_ms)
    }

    fn should_reconnect_for_periodic_policy(&self) -> bool {
        let Some(policy) = self.remote_reconnect_policy() else {
            return false;
        };
        let Some(period_ms) = policy.periodic_reconnect_ms else {
            return false;
        };
        let Some(started_at) = self.session_started_at else {
            return false;
        };

        started_at.elapsed() >= Duration::from_millis(period_ms)
    }

    fn is_auth_service_error(error: &ServiceError) -> bool {
        match error {
            ServiceError::McpError(error) => Self::is_auth_error_text(&error.message),
            _ => false,
        }
    }

    fn is_session_expired_service_error(error: &ServiceError) -> bool {
        match error {
            ServiceError::McpError(error) => {
                let message = error.message.to_ascii_lowercase();
                message.contains("session")
                    && (message.contains("expired")
                        || message.contains("invalid")
                        || message.contains("not found")
                        || message.contains("closed"))
            }
            _ => false,
        }
    }

    fn is_auth_roci_error(error: &RociError) -> bool {
        match error {
            RociError::Authentication(_) | RociError::MissingCredential { .. } => true,
            RociError::Api { status, .. } => matches!(status, 401 | 403),
            RociError::Provider { message, .. }
            | RociError::Stream(message)
            | RociError::Configuration(message) => Self::is_auth_error_text(message),
            _ => false,
        }
    }

    fn is_auth_error_text(message: &str) -> bool {
        let message = message.to_ascii_lowercase();
        message.contains("401")
            || message.contains("403")
            || message.contains("unauthorized")
            || message.contains("forbidden")
            || message.contains("authentication")
            || message.contains("authorization")
            || message.contains("auth")
            || message.contains("credential")
            || message.contains("token")
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
