//! Model Context Protocol (MCP) client and tool bridge.

pub mod bridge;
pub mod client;
pub mod schema;
pub mod transport;

pub use bridge::MCPToolAdapter;
pub use client::MCPClient;
