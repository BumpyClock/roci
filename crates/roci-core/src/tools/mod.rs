//! Tool system for function calling.

pub mod arguments;
pub mod catalog;
pub mod dynamic;
pub mod tool;
pub mod types;
pub mod user_input;
pub mod validation;

pub use arguments::ToolArguments;
pub use catalog::{
    catalog_from_groups, count_by_origin, ToolCatalog, ToolDescriptor, ToolOrigin,
    ToolVisibilityPolicy,
};
pub use dynamic::{DynamicTool, DynamicToolAdapter, DynamicToolProvider};
#[cfg(feature = "agent")]
pub use tool::ToolUpdateCallback;
pub use tool::{AgentTool, Tool, ToolApproval, ToolApprovalKind, ToolExecutionContext, ToolSafety};
pub use types::AgentToolParameters;
pub use user_input::{
    AskUserChoice, AskUserFormField, AskUserFormInputKind, AskUserPrompt, RequestUserInputFn,
    UnknownUserInputRequest, UserInputError, UserInputRequest, UserInputRequestId,
    UserInputResponse, UserInputResult, UserInputValue,
};
pub use validation::validate_arguments;
