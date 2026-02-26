use async_trait::async_trait;
use rmcp::model::{ClientInfo, ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::service::{ClientInitializeError, ServiceExt};
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;

use crate::error::RociError;
use super::{MCPRunningService, MCPTransport};
use super::common::{DynRoleClientTransport, ErasedRoleClientTransport};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::RociError;
    use crate::mcp::transport::test_support::{
        test_client_request, test_server_response, MockInnerTransport,
    };
    use std::sync::atomic::Ordering;

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
}
