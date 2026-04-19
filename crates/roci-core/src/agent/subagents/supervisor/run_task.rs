//! Background task that drives a child sub-agent to completion.
//!
//! Extracted from [`super::SubagentSupervisor::spawn_with_context`] to keep the
//! spawn method focused on setup and registration.

use std::sync::Arc;

use tokio::sync::{broadcast, oneshot, watch, Mutex, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::agent::runtime::AgentRuntime;
use crate::agent::subagents::types::{
    SubagentEvent, SubagentId, SubagentRunResult, SubagentSnapshot, SubagentStatus,
};
use crate::agent_loop::RunStatus;
use crate::models::LanguageModel;

/// Run a child agent inside a background task.
///
/// Acquires a semaphore permit, transitions status to `Running`, executes the
/// child runtime (racing against cancellation), maps the outcome, emits
/// terminal events, and sends the result through the oneshot channel.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_child_task(
    semaphore: Arc<Semaphore>,
    event_tx: broadcast::Sender<SubagentEvent>,
    status: Arc<Mutex<SubagentStatus>>,
    snapshot_tx: watch::Sender<SubagentSnapshot>,
    child_id: SubagentId,
    profile_name: String,
    label: Option<String>,
    model: LanguageModel,
    runtime: AgentRuntime,
    cancel_token: CancellationToken,
    completion_tx: oneshot::Sender<SubagentRunResult>,
) {
    // Acquire semaphore permit (blocks if at capacity)
    let _permit = semaphore.acquire().await;

    // Transition to Running
    {
        let mut s = status.lock().await;
        *s = SubagentStatus::Running;
    }
    let _ = event_tx.send(SubagentEvent::StatusChanged {
        subagent_id: child_id,
        status: SubagentStatus::Running,
    });
    let _ = snapshot_tx.send(SubagentSnapshot {
        subagent_id: child_id,
        profile: profile_name.clone(),
        label: label.clone(),
        model: Some(model.clone()),
        status: SubagentStatus::Running,
        turn_index: 0,
        message_count: 0,
        is_streaming: true,
        last_error: None,
    });

    // Run child from seeded messages via continue_without_input(),
    // racing against cancellation. Keep the run future alive across
    // the cancel path so abort can unwind the active run to a real
    // terminal result instead of dropping the in-flight future.
    let run_future = runtime.continue_without_input();
    tokio::pin!(run_future);
    let run_result = tokio::select! {
        result = &mut run_future => result,
        _ = cancel_token.cancelled() => {
            runtime.abort().await;
            run_future.await
        }
    };

    // Map to SubagentRunResult
    let (final_status, subagent_result) = match run_result {
        Ok(rr) => {
            let st = match rr.status {
                RunStatus::Completed => SubagentStatus::Completed,
                RunStatus::Failed => SubagentStatus::Failed,
                RunStatus::Canceled => SubagentStatus::Aborted,
                RunStatus::Running => SubagentStatus::Running,
            };
            let result = SubagentRunResult {
                subagent_id: child_id,
                status: st,
                messages: rr.messages,
                error: rr.error,
            };
            (st, result)
        }
        Err(e) => {
            let err_msg = e.to_string();
            let st = if err_msg.contains("aborted") {
                SubagentStatus::Aborted
            } else {
                SubagentStatus::Failed
            };
            let result = SubagentRunResult {
                subagent_id: child_id,
                status: st,
                messages: Vec::new(),
                error: Some(err_msg),
            };
            (st, result)
        }
    };

    // Update shared status
    {
        let mut s = status.lock().await;
        *s = final_status;
    }

    // Emit terminal event
    match final_status {
        SubagentStatus::Completed => {
            let _ = event_tx.send(SubagentEvent::Completed {
                subagent_id: child_id,
                result: subagent_result.clone(),
            });
        }
        SubagentStatus::Failed => {
            let _ = event_tx.send(SubagentEvent::Failed {
                subagent_id: child_id,
                error: subagent_result
                    .error
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
            });
        }
        SubagentStatus::Aborted => {
            let _ = event_tx.send(SubagentEvent::Aborted {
                subagent_id: child_id,
            });
        }
        _ => {}
    }

    // Final snapshot
    let _ = snapshot_tx.send(SubagentSnapshot {
        subagent_id: child_id,
        profile: profile_name,
        label,
        model: Some(model),
        status: final_status,
        turn_index: 0,
        message_count: subagent_result.messages.len(),
        is_streaming: false,
        last_error: subagent_result.error.clone(),
    });

    // Send completion to handle
    let _ = completion_tx.send(subagent_result);
}
