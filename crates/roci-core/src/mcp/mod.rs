//! Model Context Protocol (MCP) client and tool bridge.

pub mod aggregate;
pub mod bridge;
pub mod client;
mod client_ops;
mod error;
pub mod instructions;
mod mapping;
pub mod schema;
pub mod transport;

pub use aggregate::{
    MCPAggregateInitPolicy, MCPAggregateServer, MCPAggregatedTool, MCPAggregationConfig,
    MCPCollisionPolicy, MCPToolAggregator, MCPToolRoute,
};
pub use bridge::MCPToolAdapter;
pub use client::MCPClient;
pub use instructions::{
    merge_mcp_instructions, MCPInstructionMergePolicy, MCPInstructionSource, MCPServerKind,
    MCPServerMetadata,
};
