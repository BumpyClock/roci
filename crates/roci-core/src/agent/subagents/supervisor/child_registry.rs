//! Internal child entry tracking and status helpers.

use std::sync::Arc;

use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::agent::subagents::types::{SubagentId, SubagentStatus};
use crate::models::LanguageModel;

/// Bookkeeping entry for a registered child sub-agent.
pub(super) struct ChildEntry {
    pub(super) id: SubagentId,
    pub(super) label: Option<String>,
    pub(super) profile: String,
    pub(super) model: Option<LanguageModel>,
    pub(super) status: Arc<Mutex<SubagentStatus>>,
    pub(super) cancel_token: CancellationToken,
}

/// Check whether a status is terminal.
pub(super) fn is_terminal(status: SubagentStatus) -> bool {
    matches!(
        status,
        SubagentStatus::Completed | SubagentStatus::Failed | SubagentStatus::Aborted
    )
}
