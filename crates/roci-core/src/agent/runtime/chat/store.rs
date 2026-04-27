use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::sync::Mutex;

use super::domain::ThreadId;
use super::error::AgentRuntimeError;
use super::event::{AgentRuntimeEvent, RuntimeCursor};

/// Sync storage contract for semantic [`AgentRuntimeEvent`] replay.
pub trait AgentRuntimeEventStore: Send + Sync {
    /// Append one semantic runtime event.
    fn append(&self, event: AgentRuntimeEvent) -> Result<RuntimeCursor, AgentRuntimeError>;

    /// Return retained events for `cursor.thread_id` with `seq > cursor.seq`.
    fn events_after(
        &self,
        cursor: RuntimeCursor,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError>;

    /// Invalidate retained replay for a thread after an out-of-band snapshot rewrite.
    fn invalidate_thread(
        &self,
        thread_id: ThreadId,
        latest_seq: u64,
    ) -> Result<(), AgentRuntimeError>;
}

/// In-memory semantic event store with optional per-thread replay capacity.
#[derive(Debug, Default)]
pub struct InMemoryAgentRuntimeEventStore {
    inner: Mutex<InMemoryAgentRuntimeEventStoreInner>,
    replay_capacity: Option<NonZeroUsize>,
}

impl InMemoryAgentRuntimeEventStore {
    /// Create an unbounded in-memory semantic event store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create an in-memory semantic event store capped per thread.
    ///
    /// Capacity is non-zero so the oldest retained seq is always meaningful for
    /// stale cursor errors.
    #[must_use]
    pub fn with_replay_capacity(replay_capacity: NonZeroUsize) -> Self {
        Self {
            inner: Mutex::new(InMemoryAgentRuntimeEventStoreInner {
                threads: HashMap::new(),
            }),
            replay_capacity: Some(replay_capacity),
        }
    }
}

impl AgentRuntimeEventStore for InMemoryAgentRuntimeEventStore {
    fn append(&self, event: AgentRuntimeEvent) -> Result<RuntimeCursor, AgentRuntimeError> {
        let mut inner = self.lock_inner()?;
        let thread = inner.threads.entry(event.thread_id).or_default();
        if event.seq <= thread.latest_seq {
            return Err(AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "runtime event seq must increase for thread {}: received {}, latest {}",
                    event.thread_id, event.seq, thread.latest_seq
                ),
            });
        }

        let cursor = event.cursor();
        thread.latest_seq = event.seq;
        thread.events.push_back(event);
        if let Some(replay_capacity) = self.replay_capacity {
            while thread.events.len() > replay_capacity.get() {
                thread.events.pop_front();
            }
        }

        Ok(cursor)
    }

    fn events_after(
        &self,
        cursor: RuntimeCursor,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        let inner = self.lock_inner()?;
        let Some(thread) = inner.threads.get(&cursor.thread_id) else {
            return Ok(Vec::new());
        };

        if cursor.seq < thread.oldest_replayable_cursor_seq() {
            return Err(AgentRuntimeError::StaleRuntime {
                thread_id: cursor.thread_id,
                requested_seq: cursor.seq,
                oldest_available_seq: thread.oldest_available_seq(),
                latest_seq: thread.latest_seq,
            });
        }

        Ok(thread
            .events
            .iter()
            .filter(|event| event.seq > cursor.seq)
            .cloned()
            .collect())
    }

    fn invalidate_thread(
        &self,
        thread_id: ThreadId,
        latest_seq: u64,
    ) -> Result<(), AgentRuntimeError> {
        let mut inner = self.lock_inner()?;
        let thread = inner.threads.entry(thread_id).or_default();
        thread.latest_seq = latest_seq;
        thread.events.clear();
        Ok(())
    }
}

impl InMemoryAgentRuntimeEventStore {
    fn lock_inner(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, InMemoryAgentRuntimeEventStoreInner>, AgentRuntimeError>
    {
        self.inner
            .lock()
            .map_err(|_| AgentRuntimeError::ProjectionFailed {
                message: "runtime event store lock poisoned".to_string(),
            })
    }
}

#[derive(Debug, Default)]
struct InMemoryAgentRuntimeEventStoreInner {
    threads: HashMap<ThreadId, ThreadEvents>,
}

#[derive(Debug, Default)]
struct ThreadEvents {
    latest_seq: u64,
    events: VecDeque<AgentRuntimeEvent>,
}

impl ThreadEvents {
    fn oldest_available_seq(&self) -> u64 {
        self.events
            .front()
            .map_or(self.latest_seq.saturating_add(1), |event| event.seq)
    }

    fn oldest_replayable_cursor_seq(&self) -> u64 {
        self.oldest_available_seq().saturating_sub(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::runtime::chat::{AgentRuntimeEventPayload, TurnId, TurnSnapshot, TurnStatus};

    fn test_turn(thread_id: ThreadId, seq: u64) -> TurnSnapshot {
        let now = chrono::Utc::now();
        TurnSnapshot {
            turn_id: TurnId::new(thread_id, 0, seq),
            thread_id,
            status: TurnStatus::Queued,
            message_ids: Vec::new(),
            active_tool_call_ids: Vec::new(),
            error: None,
            queued_at: now,
            started_at: None,
            completed_at: None,
        }
    }

    fn test_event(thread_id: ThreadId, seq: u64) -> AgentRuntimeEvent {
        let turn = test_turn(thread_id, seq);
        AgentRuntimeEvent::new(
            seq,
            thread_id,
            Some(turn.turn_id),
            AgentRuntimeEventPayload::TurnQueued { turn },
        )
    }

    #[test]
    fn events_after_returns_events_after_cursor_for_same_thread() {
        let store = InMemoryAgentRuntimeEventStore::new();
        let thread_id = ThreadId::new();
        let other_thread_id = ThreadId::new();

        store.append(test_event(thread_id, 1)).unwrap();
        store.append(test_event(other_thread_id, 1)).unwrap();
        store.append(test_event(thread_id, 2)).unwrap();
        store.append(test_event(thread_id, 3)).unwrap();

        let events = store
            .events_after(RuntimeCursor::new(thread_id, 1))
            .unwrap();

        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert!(events.iter().all(|event| event.thread_id == thread_id));
    }

    #[test]
    fn append_returns_cursor_and_preserves_append_order() {
        let store = InMemoryAgentRuntimeEventStore::new();
        let thread_id = ThreadId::new();

        let cursor = store.append(test_event(thread_id, 1)).unwrap();
        store.append(test_event(thread_id, 2)).unwrap();
        store.append(test_event(thread_id, 3)).unwrap();

        let events = store
            .events_after(RuntimeCursor::new(thread_id, 0))
            .unwrap();

        assert_eq!(cursor, RuntimeCursor::new(thread_id, 1));
        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
    }

    #[test]
    fn bounded_store_evicts_old_events_and_reports_stale_cursors() {
        let store =
            InMemoryAgentRuntimeEventStore::with_replay_capacity(NonZeroUsize::new(2).unwrap());
        let thread_id = ThreadId::new();

        store.append(test_event(thread_id, 1)).unwrap();
        store.append(test_event(thread_id, 2)).unwrap();
        store.append(test_event(thread_id, 3)).unwrap();

        let events = store
            .events_after(RuntimeCursor::new(thread_id, 1))
            .unwrap();
        let stale = store
            .events_after(RuntimeCursor::new(thread_id, 0))
            .unwrap_err();

        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert_eq!(
            stale,
            AgentRuntimeError::StaleRuntime {
                thread_id,
                requested_seq: 0,
                oldest_available_seq: 2,
                latest_seq: 3,
            }
        );
    }

    #[test]
    fn append_rejects_non_increasing_seq_per_thread() {
        let store = InMemoryAgentRuntimeEventStore::new();
        let thread_id = ThreadId::new();

        store.append(test_event(thread_id, 2)).unwrap();
        let err = store.append(test_event(thread_id, 2)).unwrap_err();

        assert!(matches!(err, AgentRuntimeError::ProjectionFailed { .. }));
    }

    #[test]
    fn invalidate_thread_clears_replay_and_marks_old_cursors_stale() {
        let store = InMemoryAgentRuntimeEventStore::new();
        let thread_id = ThreadId::new();

        store.append(test_event(thread_id, 1)).unwrap();
        store.append(test_event(thread_id, 2)).unwrap();
        store.invalidate_thread(thread_id, 3).unwrap();

        let err = store
            .events_after(RuntimeCursor::new(thread_id, 2))
            .unwrap_err();

        assert_eq!(
            err,
            AgentRuntimeError::StaleRuntime {
                thread_id,
                requested_seq: 2,
                oldest_available_seq: 4,
                latest_seq: 3,
            }
        );
    }
}
