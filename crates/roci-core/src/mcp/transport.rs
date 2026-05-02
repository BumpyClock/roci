//! MCP transport layer.

use async_trait::async_trait;
use rmcp::service::{ClientInitializeError, DynService, RoleClient, RunningService};

use crate::error::RociError;

use super::elicitation::MCPClientHandler;

pub type DynClientService = Box<dyn DynService<RoleClient>>;
pub type MCPRunningService = RunningService<RoleClient, DynClientService>;

/// Transport trait for MCP communication.
#[async_trait]
pub trait MCPTransport: Send {
    /// Create and initialize a new rmcp running service for this transport.
    async fn connect(
        &mut self,
        client_handler: MCPClientHandler,
    ) -> Result<MCPRunningService, ClientInitializeError>;

    /// Send a JSON-RPC message.
    async fn send(&mut self, message: serde_json::Value) -> Result<(), RociError>;

    /// Receive a JSON-RPC message.
    async fn receive(&mut self) -> Result<serde_json::Value, RociError>;

    /// Close the transport.
    async fn close(&mut self) -> Result<(), RociError>;
}

mod common;
mod sse;
mod stdio;

pub use sse::SSETransport;
pub use stdio::StdioTransport;

#[cfg(test)]
mod test_support;
