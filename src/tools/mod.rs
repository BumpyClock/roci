//! Tool system for function calling.

pub mod arguments;
pub mod dynamic;
pub mod tool;
pub mod types;

pub use arguments::ToolArguments;
pub use dynamic::{DynamicTool, DynamicToolProvider};
pub use tool::{AgentTool, Tool, ToolExecutionContext};
#[cfg(feature = "agent")]
pub use tool::ToolUpdateCallback;
pub use types::AgentToolParameters;
