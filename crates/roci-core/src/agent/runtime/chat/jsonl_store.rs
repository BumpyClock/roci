use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, ErrorKind};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use super::domain::ThreadId;
use super::error::AgentRuntimeError;
use super::event::{AgentRuntimeEvent, RuntimeCursor};
use super::store::AgentRuntimeEventStore;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum JsonlRuntimeRecord {
    Event {
        event: AgentRuntimeEvent,
    },
    EventBatch {
        events: Vec<AgentRuntimeEvent>,
    },
    ThreadInvalidated {
        thread_id: ThreadId,
        latest_seq: u64,
    },
}

/// Durable JSONL-backed semantic runtime event store.
#[derive(Debug)]
pub struct JsonlAgentRuntimeEventStore {
    path: PathBuf,
    inner: Mutex<JsonlEventStoreState>,
}

impl JsonlAgentRuntimeEventStore {
    /// Open a JSONL runtime event store, replaying all committed records.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, AgentRuntimeError> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                AgentRuntimeError::ProjectionFailed {
                    message: format!(
                        "failed to create runtime event store directory {}: {source}",
                        parent.display()
                    ),
                }
            })?;
        }

        let inner = replay_file(&path)?;

        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    async fn append_record(&self, record: &JsonlRuntimeRecord) -> Result<(), AgentRuntimeError> {
        let mut encoded =
            serde_json::to_vec(record).map_err(|source| AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "failed to serialize runtime event record for {}: {source}",
                    self.path.display()
                ),
            })?;
        encoded.push(b'\n');
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .await
            .map_err(|source| AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "failed to open runtime event store {} for append: {source}",
                    self.path.display()
                ),
            })?;

        file.write_all(&encoded)
            .await
            .map_err(|source| AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "failed to append runtime event record to {}: {source}",
                    self.path.display()
                ),
            })?;
        file.flush()
            .await
            .map_err(|source| AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "failed to flush runtime event store {}: {source}",
                    self.path.display()
                ),
            })?;
        file.sync_data()
            .await
            .map_err(|source| AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "failed to sync runtime event store {}: {source}",
                    self.path.display()
                ),
            })?;

        Ok(())
    }

    /// Return all retained replay events sorted by thread id and per-thread seq.
    pub async fn all_events(&self) -> Vec<AgentRuntimeEvent> {
        let inner = self.inner.lock().await;
        let mut events = inner
            .threads
            .values()
            .flat_map(|thread| thread.events.iter().cloned())
            .collect::<Vec<_>>();
        events.sort_by_key(|event| (event.thread_id.to_string(), event.seq));
        events
    }
}

#[async_trait]
impl AgentRuntimeEventStore for JsonlAgentRuntimeEventStore {
    async fn append(&self, event: AgentRuntimeEvent) -> Result<RuntimeCursor, AgentRuntimeError> {
        let mut inner = self.inner.lock().await;
        inner.validate_event(&event)?;

        let cursor = event.cursor();
        self.append_record(&JsonlRuntimeRecord::Event {
            event: event.clone(),
        })
        .await?;
        inner.apply_event(event);

        Ok(cursor)
    }

    async fn append_batch(
        &self,
        events: Vec<AgentRuntimeEvent>,
    ) -> Result<Vec<RuntimeCursor>, AgentRuntimeError> {
        let mut inner = self.inner.lock().await;
        inner.validate_events(&events)?;

        let cursors = events
            .iter()
            .map(AgentRuntimeEvent::cursor)
            .collect::<Vec<_>>();
        self.append_record(&JsonlRuntimeRecord::EventBatch {
            events: events.clone(),
        })
        .await?;
        inner.apply_events(events);

        Ok(cursors)
    }

    async fn events_after(
        &self,
        cursor: RuntimeCursor,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        let inner = self.inner.lock().await;
        inner.events_after(cursor)
    }

    async fn invalidate_thread(
        &self,
        thread_id: ThreadId,
        latest_seq: u64,
    ) -> Result<(), AgentRuntimeError> {
        let mut inner = self.inner.lock().await;
        self.append_record(&JsonlRuntimeRecord::ThreadInvalidated {
            thread_id,
            latest_seq,
        })
        .await?;
        inner.invalidate_thread(thread_id, latest_seq);
        Ok(())
    }
}

fn replay_file(path: &Path) -> Result<JsonlEventStoreState, AgentRuntimeError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == ErrorKind::NotFound => {
            return Ok(JsonlEventStoreState::default());
        }
        Err(source) => {
            return Err(AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "failed to replay runtime events from {}: {source}",
                    path.display()
                ),
            });
        }
    };

    let mut reader = std::io::BufReader::new(file);
    let mut state = JsonlEventStoreState::default();
    let mut line = String::new();
    let mut line_number = 0;

    loop {
        line.clear();
        let bytes_read =
            reader
                .read_line(&mut line)
                .map_err(|source| AgentRuntimeError::ProjectionFailed {
                    message: format!(
                        "failed to replay runtime events from {} at line {}: {source}",
                        path.display(),
                        line_number + 1
                    ),
                })?;
        if bytes_read == 0 {
            break;
        }

        line_number += 1;
        if line.trim().is_empty() {
            continue;
        }
        if !line.ends_with('\n') {
            return Err(AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "failed to replay runtime events from {} at final line {line_number}: record missing trailing newline",
                    path.display()
                ),
            });
        }

        let record = serde_json::from_str::<JsonlRuntimeRecord>(&line).map_err(|source| {
            AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "failed to replay runtime events from {} at line {line_number}: {source}",
                    path.display()
                ),
            }
        })?;
        state.apply_record(record)?;
    }

    Ok(state)
}

#[derive(Debug, Default)]
struct JsonlEventStoreState {
    threads: HashMap<ThreadId, ThreadEvents>,
}

impl JsonlEventStoreState {
    fn validate_event(&self, event: &AgentRuntimeEvent) -> Result<(), AgentRuntimeError> {
        let latest_seq = self
            .threads
            .get(&event.thread_id)
            .map_or(0, |thread| thread.latest_seq);
        if event.seq <= latest_seq {
            return Err(AgentRuntimeError::ProjectionFailed {
                message: format!(
                    "runtime event seq must increase for thread {}: received {}, latest {}",
                    event.thread_id, event.seq, latest_seq
                ),
            });
        }

        Ok(())
    }

    fn apply_record(&mut self, record: JsonlRuntimeRecord) -> Result<(), AgentRuntimeError> {
        match record {
            JsonlRuntimeRecord::Event { event } => {
                self.validate_event(&event)?;
                self.apply_event(event);
            }
            JsonlRuntimeRecord::EventBatch { events } => {
                self.validate_events(&events)?;
                self.apply_events(events);
            }
            JsonlRuntimeRecord::ThreadInvalidated {
                thread_id,
                latest_seq,
            } => self.invalidate_thread(thread_id, latest_seq),
        }

        Ok(())
    }

    fn validate_events(&self, events: &[AgentRuntimeEvent]) -> Result<(), AgentRuntimeError> {
        let mut latest_by_thread = events
            .iter()
            .map(|event| {
                (
                    event.thread_id,
                    self.threads
                        .get(&event.thread_id)
                        .map_or(0, |thread| thread.latest_seq),
                )
            })
            .collect::<HashMap<_, _>>();

        for event in events {
            let latest_seq = latest_by_thread
                .get(&event.thread_id)
                .copied()
                .unwrap_or_default();
            if event.seq <= latest_seq {
                return Err(AgentRuntimeError::ProjectionFailed {
                    message: format!(
                        "runtime event seq must increase for thread {}: received {}, latest {}",
                        event.thread_id, event.seq, latest_seq
                    ),
                });
            }
            latest_by_thread.insert(event.thread_id, event.seq);
        }

        Ok(())
    }

    fn apply_event(&mut self, event: AgentRuntimeEvent) {
        let thread = self.threads.entry(event.thread_id).or_default();
        thread.latest_seq = event.seq;
        thread.events.push_back(event);
    }

    fn apply_events(&mut self, events: Vec<AgentRuntimeEvent>) {
        for event in events {
            self.apply_event(event);
        }
    }

    fn events_after(
        &self,
        cursor: RuntimeCursor,
    ) -> Result<Vec<AgentRuntimeEvent>, AgentRuntimeError> {
        let Some(thread) = self.threads.get(&cursor.thread_id) else {
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

    fn invalidate_thread(&mut self, thread_id: ThreadId, latest_seq: u64) {
        let thread = self.threads.entry(thread_id).or_default();
        thread.latest_seq = latest_seq;
        thread.events.clear();
    }
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

    fn test_path(temp: &tempfile::TempDir) -> PathBuf {
        temp.path().join("events.jsonl")
    }

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

    #[tokio::test]
    async fn append_two_events_reopens_and_replays_after_cursor_zero() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let path = test_path(&temp);
        let thread_id = ThreadId::new();
        let store = JsonlAgentRuntimeEventStore::open(path.clone()).unwrap();

        store.append(test_event(thread_id, 1)).await.unwrap();
        store.append(test_event(thread_id, 2)).await.unwrap();
        drop(store);

        let reopened = JsonlAgentRuntimeEventStore::open(path).unwrap();
        let events = reopened
            .events_after(RuntimeCursor::new(thread_id, 0))
            .await
            .unwrap();

        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[tokio::test]
    async fn append_batch_reopens_and_replays_as_one_committed_record() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let path = test_path(&temp);
        let thread_id = ThreadId::new();
        let store = JsonlAgentRuntimeEventStore::open(path.clone()).unwrap();

        store
            .append_batch(vec![test_event(thread_id, 1), test_event(thread_id, 2)])
            .await
            .unwrap();
        drop(store);

        let contents = std::fs::read_to_string(&path).expect("jsonl should be readable");
        assert_eq!(contents.lines().count(), 1);

        let reopened = JsonlAgentRuntimeEventStore::open(path).unwrap();
        let events = reopened
            .events_after(RuntimeCursor::new(thread_id, 0))
            .await
            .unwrap();

        assert_eq!(
            events.iter().map(|event| event.seq).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[tokio::test]
    async fn invalidation_reopens_and_marks_prior_cursor_stale() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let path = test_path(&temp);
        let thread_id = ThreadId::new();
        let store = JsonlAgentRuntimeEventStore::open(path.clone()).unwrap();

        store.append(test_event(thread_id, 1)).await.unwrap();
        store.append(test_event(thread_id, 2)).await.unwrap();
        store.invalidate_thread(thread_id, 3).await.unwrap();
        drop(store);

        let reopened = JsonlAgentRuntimeEventStore::open(path).unwrap();
        let stale = reopened
            .events_after(RuntimeCursor::new(thread_id, 2))
            .await
            .unwrap_err();

        assert_eq!(
            stale,
            AgentRuntimeError::StaleRuntime {
                thread_id,
                requested_seq: 2,
                oldest_available_seq: 4,
                latest_seq: 3,
            }
        );
    }

    #[test]
    fn corrupt_nonblank_committed_line_returns_projection_failed_with_path_and_line() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let path = test_path(&temp);
        let thread_id = ThreadId::new();
        let valid = serde_json::to_string(&JsonlRuntimeRecord::Event {
            event: test_event(thread_id, 1),
        })
        .unwrap();
        std::fs::write(&path, format!("{valid}\nnot-json\n")).unwrap();

        let err = JsonlAgentRuntimeEventStore::open(path).unwrap_err();

        match err {
            AgentRuntimeError::ProjectionFailed { message } => {
                assert!(message.contains("events.jsonl"));
                assert!(message.contains("line 2"));
            }
            other => panic!("expected projection failure, got {other:?}"),
        }
    }

    #[test]
    fn valid_json_without_trailing_newline_fails_replay_as_uncommitted() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let path = test_path(&temp);
        let thread_id = ThreadId::new();
        let valid = serde_json::to_string(&JsonlRuntimeRecord::Event {
            event: test_event(thread_id, 1),
        })
        .unwrap();
        std::fs::write(&path, valid).unwrap();

        let err = JsonlAgentRuntimeEventStore::open(path).unwrap_err();

        match err {
            AgentRuntimeError::ProjectionFailed { message } => {
                assert!(message.contains("events.jsonl"));
                assert!(message.contains("final line 1"));
            }
            other => panic!("expected projection failure, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn blank_trailing_lines_are_ignored() {
        let temp = tempfile::tempdir().expect("tempdir should be created");
        let path = test_path(&temp);
        let thread_id = ThreadId::new();
        let store = JsonlAgentRuntimeEventStore::open(path.clone()).unwrap();

        store.append(test_event(thread_id, 1)).await.unwrap();
        drop(store);
        let mut contents = std::fs::read_to_string(&path).unwrap();
        contents.push_str("\n\n  \n");
        std::fs::write(&path, contents).unwrap();

        let reopened = JsonlAgentRuntimeEventStore::open(path).unwrap();
        let events = reopened
            .events_after(RuntimeCursor::new(thread_id, 0))
            .await
            .unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].seq, 1);
    }
}
