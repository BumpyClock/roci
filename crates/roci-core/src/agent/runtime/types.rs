use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::agent_loop::compaction::{extract_file_operations, FileOperationSet};
use crate::context::{estimate_message_tokens, PreparedCompaction};
use crate::error::RociError;
use crate::resource::{BranchSummarySettings, CompactionSettings};
use crate::types::ModelMessage;

/// Agent runtime state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// No run in progress; ready to accept prompts.
    Idle,
    /// A run is actively executing.
    Running,
    /// An abort has been requested; waiting for the run to wind down.
    Aborting,
}

/// Queue drain behavior for steering/follow-up messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueDrainMode {
    /// Drain all queued messages at once.
    All,
    /// Drain at most one message per turn/phase.
    OneAtATime,
}

pub(super) fn drain_queue(
    queue: &mut Vec<ModelMessage>,
    mode: QueueDrainMode,
) -> Vec<ModelMessage> {
    match mode {
        QueueDrainMode::All => std::mem::take(queue),
        QueueDrainMode::OneAtATime => {
            if queue.is_empty() {
                Vec::new()
            } else {
                vec![queue.remove(0)]
            }
        }
    }
}

/// Point-in-time snapshot of agent observable state.
///
/// Captures all externally observable dimensions of an [`super::AgentRuntime`] at a
/// single instant. Subscribe to changes via [`super::AgentRuntime::watch_snapshot`].
///
/// # Example
///
/// ```ignore
/// let snap = agent.snapshot().await;
/// println!("turn {}, {} messages", snap.turn_index, snap.message_count);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSnapshot {
    pub state: AgentState,
    pub turn_index: usize,
    pub message_count: usize,
    pub is_streaming: bool,
    pub last_error: Option<String>,
}

/// Async callback that resolves an API key at request time.
///
/// Enables token rotation and dynamic key resolution without rebuilding the
/// agent. The callback is invoked once per run, before the [`crate::agent_loop::RunRequest`]
/// is dispatched to the inner loop.
///
/// # Example
///
/// ```ignore
/// let get_key: GetApiKeyFn = Arc::new(|| {
///     Box::pin(async { Ok("sk-live-rotated-key".to_string()) })
/// });
/// ```
pub type GetApiKeyFn =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Result<String, RociError>> + Send>> + Send + Sync>;

/// Outcome returned by `session_before_compact`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionBeforeCompactOutcome {
    /// Continue with normal compaction summary generation.
    Continue,
    /// Skip compaction for this operation.
    Cancel,
    /// Override compaction summary and kept-boundary metadata.
    OverrideCompaction(SessionCompactionOverride),
}

/// Outcome returned by `session_before_tree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionBeforeTreeOutcome {
    /// Continue with normal branch summary generation.
    Continue,
    /// Skip branch summary generation for this operation.
    Cancel,
    /// Use the provided summary text instead of model-generated text.
    OverrideSummary(String),
}

/// Full compaction override returned by `session_before_compact`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionCompactionOverride {
    pub summary: String,
    pub first_kept_entry_id: usize,
    pub tokens_before: usize,
    pub details: Option<String>,
}

/// Prepared summary input data exposed to session hooks.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryPreparationData {
    pub messages: Vec<ModelMessage>,
    pub token_count: usize,
    pub file_operations: FileOperationSet,
}

impl SummaryPreparationData {
    pub(super) fn from_messages(messages: Vec<ModelMessage>) -> Self {
        let token_count = messages.iter().map(estimate_message_tokens).sum::<usize>();
        let file_operations = extract_file_operations(&messages);
        Self {
            messages,
            token_count,
            file_operations,
        }
    }
}

/// Payload for the `session_before_compact` lifecycle hook.
#[derive(Debug, Clone)]
pub struct SessionBeforeCompactPayload {
    pub to_summarize: SummaryPreparationData,
    pub turn_prefix: SummaryPreparationData,
    pub kept: SummaryPreparationData,
    pub split_turn: bool,
    pub settings: CompactionSettings,
    pub cancellation_token: CancellationToken,
}

impl SessionBeforeCompactPayload {
    pub(super) fn from_prepared(
        prepared: &PreparedCompaction,
        settings: CompactionSettings,
        cancellation_token: CancellationToken,
    ) -> Self {
        Self {
            to_summarize: SummaryPreparationData::from_messages(
                prepared.messages_to_summarize.clone(),
            ),
            turn_prefix: SummaryPreparationData::from_messages(
                prepared.turn_prefix_messages.clone(),
            ),
            kept: SummaryPreparationData::from_messages(prepared.kept_messages.clone()),
            split_turn: prepared.split_turn,
            settings,
            cancellation_token,
        }
    }
}

/// Payload for the `session_before_tree` lifecycle hook.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionBeforeTreePayload {
    pub to_summarize: SummaryPreparationData,
    pub settings: BranchSummarySettings,
}

/// Async hook interface for `session_before_compact`.
pub type SessionBeforeCompactHook = Arc<
    dyn Fn(
            SessionBeforeCompactPayload,
        )
            -> Pin<Box<dyn Future<Output = Result<SessionBeforeCompactOutcome, RociError>> + Send>>
        + Send
        + Sync,
>;

/// Async hook interface for `session_before_tree`.
pub type SessionBeforeTreeHook = Arc<
    dyn Fn(
            SessionBeforeTreePayload,
        )
            -> Pin<Box<dyn Future<Output = Result<SessionBeforeTreeOutcome, RociError>> + Send>>
        + Send
        + Sync,
>;
