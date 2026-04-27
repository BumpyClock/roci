//! Chat runtime public contracts.
//!
//! Defines serializable DTOs used by chat projection, subscription, and storage
//! layers. Projector/store implementations land in later slices.

pub mod domain;
pub mod error;
pub mod event;
pub mod projector;
pub mod store;
pub mod subscription;

pub use domain::{
    ChatRuntimeConfig, MessageId, MessageSnapshot, MessageStatus, RuntimeSnapshot, ThreadId,
    ThreadSnapshot, ToolExecutionSnapshot, ToolStatus, TurnId, TurnSnapshot, TurnStatus,
};
pub use error::AgentRuntimeError;
pub use event::{
    AgentRuntimeEvent, AgentRuntimeEventPayload, RuntimeCursor, AGENT_RUNTIME_EVENT_SCHEMA_VERSION,
};
pub use projector::{ChatProjector, MessageProjection, ModelMessages, TurnProjection};
pub use store::{AgentRuntimeEventStore, InMemoryAgentRuntimeEventStore};
pub use subscription::RuntimeSubscription;
