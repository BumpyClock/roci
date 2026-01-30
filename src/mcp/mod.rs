//! Model Context Protocol (MCP) client and tool bridge.

pub mod client;
pub mod transport;
pub mod bridge;
pub mod schema;

pub use client::MCPClient;
pub use bridge::MCPToolAdapter;
