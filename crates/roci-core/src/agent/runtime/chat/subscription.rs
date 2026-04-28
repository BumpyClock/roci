use std::collections::HashMap;

use tokio::sync::broadcast;

use super::{AgentRuntimeError, AgentRuntimeEvent, RuntimeCursor, ThreadId};

/// Replay plus live stream subscription for semantic runtime events.
#[derive(Debug)]
pub struct RuntimeSubscription {
    replay: Result<Vec<AgentRuntimeEvent>, AgentRuntimeError>,
    live_rx: broadcast::Receiver<AgentRuntimeEvent>,
    last_replay_seq_by_thread: HashMap<ThreadId, u64>,
    startup_error_returned: bool,
}

impl RuntimeSubscription {
    pub(crate) fn new(
        replay: Result<Vec<AgentRuntimeEvent>, AgentRuntimeError>,
        live_rx: broadcast::Receiver<AgentRuntimeEvent>,
        cursor: Option<RuntimeCursor>,
    ) -> Self {
        let mut last_replay_seq_by_thread = HashMap::new();
        if let Some(cursor) = cursor {
            last_replay_seq_by_thread.insert(cursor.thread_id, cursor.seq);
        }
        if let Ok(events) = &replay {
            for event in events {
                last_replay_seq_by_thread
                    .entry(event.thread_id)
                    .and_modify(|seq| *seq = (*seq).max(event.seq))
                    .or_insert(event.seq);
            }
        }

        Self {
            replay,
            live_rx,
            last_replay_seq_by_thread,
            startup_error_returned: false,
        }
    }

    /// Return retained replay events requested by the subscription cursor.
    ///
    /// This does not consume live events. Call this first when resuming from a
    /// cursor, then call [`recv`](Self::recv) or [`next`](Self::next) for fresh
    /// events.
    ///
    /// # Errors
    ///
    /// Returns [`AgentRuntimeError::StaleRuntime`] when the requested cursor is
    /// older than the configured event store can replay.
    pub fn replay(&self) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        self.replay.clone()
    }

    /// Receive the next live semantic runtime event.
    ///
    /// If the subscription was created with a stale cursor and [`replay`](Self::replay)
    /// was not checked, the startup replay error is returned before any live
    /// event is read.
    ///
    /// # Errors
    ///
    /// Returns startup replay errors before reading live events. Broadcast
    /// lag is reported as [`AgentRuntimeError::StaleRuntime`] because missed
    /// live events require resubscribing from a known cursor.
    pub async fn recv(&mut self) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        if !self.startup_error_returned {
            self.startup_error_returned = true;
            if let Err(err) = &self.replay {
                return Err(err.clone());
            }
        }

        loop {
            match self.live_rx.recv().await {
                Ok(event) => {
                    if self.is_duplicate_live_event(&event) {
                        continue;
                    }
                    self.last_replay_seq_by_thread
                        .entry(event.thread_id)
                        .and_modify(|seq| *seq = (*seq).max(event.seq))
                        .or_insert(event.seq);
                    return Ok(event);
                }
                Err(broadcast::error::RecvError::Closed) => {
                    return Err(AgentRuntimeError::ProjectionFailed {
                        message: "runtime event stream closed".to_string(),
                    });
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    return Err(self.lagged_error());
                }
            }
        }
    }

    /// Alias for [`recv`](Self::recv).
    pub async fn next(&mut self) -> Result<AgentRuntimeEvent, AgentRuntimeError> {
        self.recv().await
    }

    fn is_duplicate_live_event(&self, event: &AgentRuntimeEvent) -> bool {
        self.last_replay_seq_by_thread
            .get(&event.thread_id)
            .is_some_and(|seq| event.seq <= *seq)
    }

    fn lagged_error(&self) -> AgentRuntimeError {
        let (thread_id, latest_seq) = self
            .last_replay_seq_by_thread
            .iter()
            .next()
            .map(|(thread_id, seq)| (*thread_id, *seq))
            .unwrap_or((ThreadId::nil(), 0));
        AgentRuntimeError::StaleRuntime {
            thread_id,
            requested_seq: latest_seq,
            oldest_available_seq: latest_seq.saturating_add(1),
            latest_seq,
        }
    }
}
