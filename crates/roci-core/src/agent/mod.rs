//! Agent system: multi-turn conversations with tool execution.

pub mod conversation;
mod core;
pub mod message;
pub mod runtime;
#[cfg(feature = "agent")]
pub mod subagents;

pub use conversation::Conversation;
pub use core::Agent;
pub use message::{convert_to_llm, AgentMessage, AgentMessageExt};
pub use runtime::chat::*;
#[cfg(feature = "agent")]
pub use runtime::HumanInteractionCoordinator;
pub use runtime::{
    AgentConfig, AgentRuntime, AgentSnapshot, AgentState, GetApiKeyFn, QueueDrainMode,
    SessionBeforeCompactHook, SessionBeforeCompactOutcome, SessionBeforeCompactPayload,
    SessionBeforeTreeHook, SessionBeforeTreeOutcome, SessionBeforeTreePayload,
    SummaryPreparationData,
};
