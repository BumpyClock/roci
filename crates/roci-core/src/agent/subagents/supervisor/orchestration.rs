//! Parallel orchestration and supervisor-level snapshot watching.

use async_stream::stream;
use futures::future::join_all;
use futures::stream::{BoxStream, FuturesUnordered, StreamExt};
use tokio::sync::{mpsc, watch};

use crate::agent::subagents::handle::SubagentHandle;
use crate::agent::subagents::types::{
    SubagentCompletion, SubagentSnapshot, SubagentSpec, SubagentStatus,
};
use crate::error::RociError;

use super::child_registry::is_terminal;
use super::SubagentSupervisor;

impl SubagentSupervisor {
    /// Spawn all specs, wait for every spawned child, and return completions in
    /// spawn order.
    ///
    /// If any spawn fails, already-spawned children are aborted and drained
    /// before the error is returned.
    pub async fn run_parallel(
        &self,
        specs: Vec<SubagentSpec>,
    ) -> Result<Vec<SubagentCompletion>, RociError> {
        let handles = self.spawn_all_or_abort(specs).await?;
        let completions = join_all(handles.iter().map(|handle| async move {
            let result = handle.wait().await;
            completion_from_handle(handle, result)
        }))
        .await;
        Ok(completions)
    }

    /// Spawn all specs and return the first child to complete.
    ///
    /// Remaining children are aborted and drained before this method returns.
    /// Returns `Ok(None)` when called with an empty spec list.
    pub async fn race(
        &self,
        specs: Vec<SubagentSpec>,
    ) -> Result<Option<SubagentCompletion>, RociError> {
        let handles = self.spawn_all_or_abort(specs).await?;
        if handles.is_empty() {
            return Ok(None);
        }

        let child_ids: Vec<_> = handles.iter().map(SubagentHandle::id).collect();
        let mut waits = FuturesUnordered::new();
        for handle in handles {
            waits.push(tokio::spawn(async move {
                let label = handle.label().map(str::to_owned);
                let profile = handle.profile_name().to_owned();
                let result = handle.wait().await;
                SubagentCompletion {
                    subagent_id: result.subagent_id,
                    label,
                    profile,
                    result,
                }
            }));
        }

        let first = match waits.next().await {
            Some(joined) => joined.map_err(join_error)?,
            None => return Ok(None),
        };

        for child_id in child_ids {
            if child_id != first.subagent_id {
                let _ = self.abort(child_id).await;
            }
        }

        while let Some(joined) = waits.next().await {
            joined.map_err(join_error)?;
        }

        Ok(Some(first))
    }

    /// Watch snapshots for all children active when this method is called.
    ///
    /// The stream yields each child's current snapshot immediately, then emits
    /// updates until every watched child reaches a terminal state.
    pub async fn watch_all(&self) -> BoxStream<'static, SubagentSnapshot> {
        watch_snapshots(self.active_snapshot_receivers().await, WatchMode::All)
    }

    /// Watch snapshots for children active when this method is called until the
    /// first watched child reaches a terminal state.
    pub async fn watch_any(&self) -> BoxStream<'static, SubagentSnapshot> {
        watch_snapshots(self.active_snapshot_receivers().await, WatchMode::Any)
    }

    async fn spawn_all_or_abort(
        &self,
        specs: Vec<SubagentSpec>,
    ) -> Result<Vec<SubagentHandle>, RociError> {
        let mut handles = Vec::with_capacity(specs.len());
        for spec in specs {
            match self.spawn(spec).await {
                Ok(handle) => handles.push(handle),
                Err(error) => {
                    abort_and_drain(&handles).await;
                    return Err(error);
                }
            }
        }
        Ok(handles)
    }

    async fn active_snapshot_receivers(&self) -> Vec<watch::Receiver<SubagentSnapshot>> {
        let children = self.children.lock().await;
        let mut receivers = Vec::new();
        for entry in children.values() {
            let status = *entry.status.lock().await;
            if matches!(status, SubagentStatus::Pending | SubagentStatus::Running) {
                receivers.push(entry.snapshot_rx.clone());
            }
        }
        receivers
    }
}

fn completion_from_handle(
    handle: &SubagentHandle,
    result: crate::agent::subagents::types::SubagentRunResult,
) -> SubagentCompletion {
    SubagentCompletion {
        subagent_id: result.subagent_id,
        label: handle.label().map(str::to_owned),
        profile: handle.profile_name().to_owned(),
        result,
    }
}

async fn abort_and_drain(handles: &[SubagentHandle]) {
    for handle in handles {
        handle.abort().await;
    }
    join_all(handles.iter().map(SubagentHandle::wait)).await;
}

fn join_error(error: tokio::task::JoinError) -> RociError {
    RociError::InvalidState(format!("subagent wait task failed: {error}"))
}

#[derive(Clone, Copy)]
enum WatchMode {
    All,
    Any,
}

fn watch_snapshots(
    receivers: Vec<watch::Receiver<SubagentSnapshot>>,
    mode: WatchMode,
) -> BoxStream<'static, SubagentSnapshot> {
    let capacity = receivers.len().max(1) * 4;
    let (tx, mut rx) = mpsc::channel(capacity);

    for receiver in receivers {
        let tx = tx.clone();
        tokio::spawn(forward_snapshots(receiver, tx));
    }
    drop(tx);

    Box::pin(stream! {
        while let Some(snapshot) = rx.recv().await {
            let terminal = is_terminal(snapshot.status);
            yield snapshot;
            if matches!(mode, WatchMode::Any) && terminal {
                break;
            }
        }
    })
}

async fn forward_snapshots(
    mut receiver: watch::Receiver<SubagentSnapshot>,
    tx: mpsc::Sender<SubagentSnapshot>,
) {
    let snapshot = receiver.borrow().clone();
    let terminal = is_terminal(snapshot.status);
    if tx.send(snapshot).await.is_err() || terminal {
        return;
    }

    while receiver.changed().await.is_ok() {
        let snapshot = receiver.borrow().clone();
        let terminal = is_terminal(snapshot.status);
        if tx.send(snapshot).await.is_err() || terminal {
            break;
        }
    }
}
