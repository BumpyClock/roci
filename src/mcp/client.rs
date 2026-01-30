//! MCP client for connecting to MCP servers.

use crate::error::RociError;

use super::transport::MCPTransport;

/// Client for a Model Context Protocol server.
pub struct MCPClient {
    _transport: Box<dyn MCPTransport>,
}

impl MCPClient {
    /// Create a new MCP client with the given transport.
    pub fn new(transport: Box<dyn MCPTransport>) -> Self {
        Self {
            _transport: transport,
        }
    }

    /// Initialize the MCP connection.
    pub async fn initialize(&mut self) -> Result<(), RociError> {
        Err(RociError::UnsupportedOperation(
            "MCP client not yet implemented".into(),
        ))
    }

    /// List available tools from the MCP server.
    pub async fn list_tools(&self) -> Result<Vec<super::schema::MCPToolSchema>, RociError> {
        Err(RociError::UnsupportedOperation(
            "MCP client not yet implemented".into(),
        ))
    }
}
