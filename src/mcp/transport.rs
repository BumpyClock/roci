//! MCP transport layer.

use async_trait::async_trait;

use crate::error::RociError;

/// Transport trait for MCP communication.
#[async_trait]
pub trait MCPTransport: Send + Sync {
    /// Send a JSON-RPC message.
    async fn send(&mut self, message: serde_json::Value) -> Result<(), RociError>;

    /// Receive a JSON-RPC message.
    async fn receive(&mut self) -> Result<serde_json::Value, RociError>;

    /// Close the transport.
    async fn close(&mut self) -> Result<(), RociError>;
}

/// Stdio-based MCP transport (for local MCP servers).
pub struct StdioTransport {
    _command: String,
    _args: Vec<String>,
}

impl StdioTransport {
    pub fn new(command: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            _command: command.into(),
            _args: args,
        }
    }
}

#[async_trait]
impl MCPTransport for StdioTransport {
    async fn send(&mut self, _message: serde_json::Value) -> Result<(), RociError> {
        Err(RociError::UnsupportedOperation("Stdio MCP transport not yet implemented".into()))
    }
    async fn receive(&mut self) -> Result<serde_json::Value, RociError> {
        Err(RociError::UnsupportedOperation("Stdio MCP transport not yet implemented".into()))
    }
    async fn close(&mut self) -> Result<(), RociError> {
        Ok(())
    }
}

/// SSE-based MCP transport (for remote MCP servers).
pub struct SSETransport {
    _url: String,
}

impl SSETransport {
    pub fn new(url: impl Into<String>) -> Self {
        Self { _url: url.into() }
    }
}

#[async_trait]
impl MCPTransport for SSETransport {
    async fn send(&mut self, _message: serde_json::Value) -> Result<(), RociError> {
        Err(RociError::UnsupportedOperation("SSE MCP transport not yet implemented".into()))
    }
    async fn receive(&mut self) -> Result<serde_json::Value, RociError> {
        Err(RociError::UnsupportedOperation("SSE MCP transport not yet implemented".into()))
    }
    async fn close(&mut self) -> Result<(), RociError> {
        Ok(())
    }
}
