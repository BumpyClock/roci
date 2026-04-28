use std::{fmt, sync::Arc};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent_loop::{ApprovalDecision, ApprovalRequest, ToolUpdatePayload};
use crate::types::{AgentToolResult, ModelMessage};

use super::store::AgentRuntimeEventStore;

macro_rules! runtime_id {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn nil() -> Self {
                Self(Uuid::nil())
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

runtime_id!(ThreadId);

/// Runtime-owned turn id. Carries a thread revision so old ids can become
/// stale after out-of-band history rewrites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TurnId {
    thread_id: ThreadId,
    revision: u64,
    ordinal: u64,
}

impl TurnId {
    #[must_use]
    pub const fn new(thread_id: ThreadId, revision: u64, ordinal: u64) -> Self {
        Self {
            thread_id,
            revision,
            ordinal,
        }
    }

    #[must_use]
    pub const fn thread_id(self) -> ThreadId {
        self.thread_id
    }

    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn ordinal(self) -> u64 {
        self.ordinal
    }
}

impl fmt::Display for TurnId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}",
            self.thread_id, self.revision, self.ordinal
        )
    }
}

/// Runtime-owned message id. Carries a thread revision so old ids can become
/// stale after out-of-band history rewrites.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MessageId {
    thread_id: ThreadId,
    revision: u64,
    ordinal: u64,
}

impl MessageId {
    #[must_use]
    pub const fn new(thread_id: ThreadId, revision: u64, ordinal: u64) -> Self {
        Self {
            thread_id,
            revision,
            ordinal,
        }
    }

    #[must_use]
    pub const fn thread_id(self) -> ThreadId {
        self.thread_id
    }

    #[must_use]
    pub const fn revision(self) -> u64 {
        self.revision
    }

    #[must_use]
    pub const fn ordinal(self) -> u64 {
        self.ordinal
    }
}

impl fmt::Display for MessageId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}",
            self.thread_id, self.revision, self.ordinal
        )
    }
}

/// Configuration for runtime-owned chat projection.
#[derive(Clone, Serialize, Deserialize)]
pub struct ChatRuntimeConfig {
    /// Number of semantic events retained in memory per thread for cursor replay.
    pub replay_capacity: usize,
    /// Optional semantic runtime event store used for replay.
    #[serde(default, skip)]
    pub event_store: Option<Arc<dyn AgentRuntimeEventStore>>,
}

impl fmt::Debug for ChatRuntimeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatRuntimeConfig")
            .field("replay_capacity", &self.replay_capacity)
            .field(
                "event_store",
                &self.event_store.as_ref().map(|_| "<configured>"),
            )
            .finish()
    }
}

impl PartialEq for ChatRuntimeConfig {
    fn eq(&self, other: &Self) -> bool {
        self.replay_capacity == other.replay_capacity
    }
}

impl Eq for ChatRuntimeConfig {}

impl Default for ChatRuntimeConfig {
    fn default() -> Self {
        Self {
            replay_capacity: 512,
            event_store: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Streaming,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Running,
    Completed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalStatus {
    Pending,
    Resolved,
    Canceled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApprovalSnapshot {
    pub request: ApprovalRequest,
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub status: ApprovalStatus,
    pub decision: Option<ApprovalDecision>,
    pub requested_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningSnapshot {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub message_id: Option<MessageId>,
    pub text: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanSnapshot {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub plan: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiffSnapshot {
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub diff: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageSnapshot {
    pub message_id: MessageId,
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub status: MessageStatus,
    pub payload: ModelMessage,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionSnapshot {
    pub tool_call_id: String,
    pub thread_id: ThreadId,
    pub turn_id: TurnId,
    pub tool_name: String,
    pub args: serde_json::Value,
    pub status: ToolStatus,
    pub partial_result: Option<ToolUpdatePayload>,
    pub final_result: Option<AgentToolResult>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurnSnapshot {
    pub turn_id: TurnId,
    pub thread_id: ThreadId,
    pub status: TurnStatus,
    pub message_ids: Vec<MessageId>,
    pub active_tool_call_ids: Vec<String>,
    pub error: Option<String>,
    pub queued_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadSnapshot {
    pub thread_id: ThreadId,
    pub revision: u64,
    pub last_seq: u64,
    pub active_turn_id: Option<TurnId>,
    pub turns: Vec<TurnSnapshot>,
    pub messages: Vec<MessageSnapshot>,
    pub tools: Vec<ToolExecutionSnapshot>,
    pub approvals: Vec<ApprovalSnapshot>,
    pub reasoning: Vec<ReasoningSnapshot>,
    pub plans: Vec<PlanSnapshot>,
    pub diffs: Vec<DiffSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshot {
    pub schema_version: u16,
    pub threads: Vec<ThreadSnapshot>,
}
