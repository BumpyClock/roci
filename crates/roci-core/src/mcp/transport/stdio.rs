use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use rmcp::service::{ClientInitializeError, ServiceExt};
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;

use super::common::{DynRoleClientTransport, ErasedRoleClientTransport};
use crate::error::RociError;
use crate::mcp::elicitation::MCPClientHandler;

use super::{MCPRunningService, MCPTransport};

/// Public stdio transport configuration.
#[derive(Clone, Default)]
pub struct StdioTransportConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
}

impl std::fmt::Debug for StdioTransportConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted_args = vec!["<redacted>"; self.args.len()];
        let redacted_env = self
            .env
            .keys()
            .map(|key| (key, "<redacted>"))
            .collect::<HashMap<_, _>>();
        f.debug_struct("StdioTransportConfig")
            .field("command", &self.command)
            .field("args", &redacted_args)
            .field("env", &redacted_env)
            .field("cwd", &self.cwd)
            .finish()
    }
}

/// Stdio-based MCP transport (for local MCP servers).
pub struct StdioTransport {
    config: StdioTransportConfig,
    inner: Option<Box<dyn DynRoleClientTransport>>,
    closed: bool,
}

impl StdioTransport {
    /// Create a stdio transport from command and args.
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self::from_config(StdioTransportConfig {
            command: command.into(),
            args,
            ..StdioTransportConfig::default()
        })
    }

    /// Create a stdio transport from public config.
    pub fn from_config(config: StdioTransportConfig) -> Self {
        Self {
            config,
            inner: None,
            closed: false,
        }
    }

    /// Create a stdio transport from command only.
    pub fn from_command(command: impl Into<String>) -> Self {
        Self::new(command, Vec::new())
    }

    pub fn command(&self) -> &str {
        &self.config.command
    }

    pub fn args(&self) -> &[String] {
        &self.config.args
    }

    pub fn env(&self) -> &HashMap<String, String> {
        &self.config.env
    }

    pub fn cwd(&self) -> Option<&Path> {
        self.config.cwd.as_deref()
    }

    fn build_command(&self) -> Command {
        let mut command = Command::new(&self.config.command);
        command.args(&self.config.args);
        command.envs(&self.config.env);
        if let Some(cwd) = &self.config.cwd {
            command.current_dir(cwd);
        }
        command
    }

    fn ensure_connected(&mut self) -> Result<(), RociError> {
        if self.closed {
            return Err(RociError::Stream("MCP transport closed".into()));
        }
        if self.inner.is_some() {
            return Ok(());
        }

        let command = self.build_command();
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
        client_handler: MCPClientHandler,
    ) -> Result<MCPRunningService, ClientInitializeError> {
        if self.closed {
            return Err(ClientInitializeError::ConnectionClosed(
                "MCP transport closed".into(),
            ));
        }

        let command = self.build_command();
        let transport = TokioChildProcess::new(command).map_err(|error| {
            ClientInitializeError::transport::<TokioChildProcess>(error, "spawn stdio transport")
        })?;

        client_handler.into_dyn().serve(transport).await
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
            .await?
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
    fn stdio_config_applies_env_and_cwd_without_debug_value_leak() {
        let config = StdioTransportConfig {
            command: "node".into(),
            args: vec!["server.js".into(), "--opaque=value-marker".into()],
            env: std::collections::HashMap::from([(
                "SERVICE_OPTION".to_string(),
                "environment-marker".to_string(),
            )]),
            cwd: Some(std::path::PathBuf::from("/tmp/mcp-server")),
        };

        let debug = format!("{config:?}");
        assert!(debug.contains("SERVICE_OPTION"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("environment-marker"));
        assert!(!debug.contains("value-marker"));

        let transport = StdioTransport::from_config(config);
        let command = transport.build_command();
        let command = command.as_std();
        assert_eq!(command.get_program(), "node");
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![
                std::ffi::OsStr::new("server.js"),
                std::ffi::OsStr::new("--opaque=value-marker"),
            ]
        );
        assert_eq!(
            command.get_current_dir(),
            Some(std::path::Path::new("/tmp/mcp-server"))
        );
        assert!(command.get_envs().any(|(key, value)| {
            key == "SERVICE_OPTION" && value == Some(std::ffi::OsStr::new("environment-marker"))
        }));
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
}
