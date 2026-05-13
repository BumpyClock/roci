//! Model Context Protocol (MCP) client and tool bridge.

pub mod aggregate;
pub mod bridge;
pub mod client;
mod client_ops;
pub mod elicitation;
mod error;
pub mod instructions;
mod mapping;
pub mod schema;
pub mod server;
pub mod transport;

pub use aggregate::{
    MCPAggregateInitPolicy, MCPAggregateServer, MCPAggregatedResource, MCPAggregatedTool,
    MCPAggregationConfig, MCPCollisionPolicy, MCPToolAggregator, MCPToolRoute,
};
pub use bridge::MCPToolAdapter;
pub use client::{MCPClient, MCPRemoteReconnectOutcome};
pub use instructions::{
    merge_mcp_instructions, MCPInstructionMergePolicy, MCPInstructionSource, MCPResourceIdentity,
    MCPServerKind, MCPServerMetadata,
};
pub use server::{
    McpCallToolResult, McpServerCore, McpServerListedTool, McpServerToolIdentity, McpToolIdentity,
    McpToolSchema,
};
pub use transport::MCPRemoteReconnectPolicy;
