//! Tool system for function calling.

pub mod arguments;
pub mod dynamic;
pub mod tool;
pub mod types;
pub mod user_input;
pub mod validation;

pub use arguments::ToolArguments;
pub use dynamic::{DynamicTool, DynamicToolAdapter, DynamicToolProvider};
#[cfg(feature = "agent")]
pub use tool::ToolUpdateCallback;
pub use tool::{AgentTool, Tool, ToolExecutionContext};
pub use types::AgentToolParameters;
pub use user_input::{
    Answer, Question, QuestionOption, RequestUserInputFn, UnknownUserInputRequest, UserInputError,
    UserInputRequest, UserInputRequestId, UserInputResponse,
};
pub use validation::validate_arguments;
