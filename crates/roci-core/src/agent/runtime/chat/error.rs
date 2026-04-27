use thiserror::Error;

use super::domain::{ThreadId, TurnId, TurnStatus};

/// Errors returned by semantic chat runtime APIs.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum AgentRuntimeError {
    #[error("runtime is busy")]
    RuntimeBusy,
    #[error("thread not found: {thread_id}")]
    ThreadNotFound { thread_id: ThreadId },
    #[error("turn not found: {turn_id}")]
    TurnNotFound { turn_id: TurnId },
    #[error("turn already terminal: {turn_id} ({status:?})")]
    AlreadyTerminal { turn_id: TurnId, status: TurnStatus },
    #[error(
        "runtime cursor is stale for thread {thread_id}: requested seq {requested_seq}, oldest available {oldest_available_seq}, latest seq {latest_seq}"
    )]
    StaleRuntime {
        thread_id: ThreadId,
        requested_seq: u64,
        oldest_available_seq: u64,
        latest_seq: u64,
    },
    #[error("chat projection failed: {message}")]
    ProjectionFailed { message: String },
}
