//! Agent system: multi-turn conversations with tool execution.

pub mod conversation;
mod core;
pub mod message;
pub mod runtime;
pub mod session;

pub use conversation::Conversation;
pub use core::Agent;
pub use message::{convert_to_llm, AgentMessage, AgentMessageExt};
pub use runtime::{
    AgentConfig, AgentRuntime, AgentSnapshot, AgentState, GetApiKeyFn, QueueDrainMode,
    SessionBeforeCompactHook, SessionBeforeCompactPayload, SessionBeforeTreeHook,
    SessionBeforeTreePayload, SessionSummaryHookOutcome, SummaryPreparationData,
};
pub use session::AgentSessionManager;
