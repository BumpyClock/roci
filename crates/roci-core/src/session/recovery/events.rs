//! Tolerant runtime event recovery helpers.

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

use crate::agent::runtime::chat::{AgentRuntimeEvent, ThreadId};

use super::{
    RecoveredEvents, RecoverySeverity, RecoverySource, RecoverySourceStats, RecoveryWarning,
};
use crate::session::{SessionError, SessionResult};

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RecoveryRuntimeRecord {
    Event {
        event: AgentRuntimeEvent,
    },
    EventBatch {
        events: Vec<serde_json::Value>,
    },
    ThreadInvalidated {
        thread_id: ThreadId,
        latest_seq: u64,
    },
}

#[derive(Debug, Default)]
struct RecoveryEventState {
    events: HashMap<ThreadId, Vec<AgentRuntimeEvent>>,
    first_thread_id: Option<ThreadId>,
    latest_seq: HashMap<ThreadId, u64>,
    stopped_threads: HashSet<ThreadId>,
    stats: RecoverySourceStats,
    warnings: Vec<RecoveryWarning>,
}

pub(crate) fn recover_events(path: impl AsRef<Path>) -> SessionResult<RecoveredEvents> {
    let path = path.as_ref();
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(SessionError::io(path, source)),
    };
    let mut state = RecoveryEventState::default();

    for (line_index, raw_line) in bytes.split_inclusive(|byte| *byte == b'\n').enumerate() {
        let line_number = line_index + 1;
        let has_newline = raw_line.ends_with(b"\n");
        let line = if has_newline {
            &raw_line[..raw_line.len().saturating_sub(1)]
        } else {
            raw_line
        };

        if line_is_blank(line) {
            continue;
        }

        state.stats.records_read += 1;
        if !has_newline {
            state.warn(
                Some(line_number),
                None,
                "events_final_line_missing_newline",
                "runtime event final line is missing trailing newline",
            );
        }

        let Ok(text) = std::str::from_utf8(line) else {
            state.stats.records_skipped += 1;
            state.warn(
                Some(line_number),
                None,
                "events_invalid_utf8",
                "runtime event record is not valid UTF-8",
            );
            continue;
        };

        let record = match serde_json::from_str::<RecoveryRuntimeRecord>(text) {
            Ok(record) => record,
            Err(source) => {
                state.stats.records_skipped += 1;
                state.warn(
                    Some(line_number),
                    None,
                    "events_malformed_json",
                    format!("runtime event record is malformed: {source}"),
                );
                continue;
            }
        };

        state.apply_record(record, line_number);
    }

    let mut events = state
        .events
        .into_values()
        .flatten()
        .collect::<Vec<AgentRuntimeEvent>>();
    events.sort_by_key(|event| (event.thread_id.to_string(), event.seq));
    state.stats.warnings = state.warnings.len();

    Ok(RecoveredEvents {
        events,
        first_thread_id: state.first_thread_id,
        stats: state.stats,
        warnings: state.warnings,
    })
}

impl RecoveryEventState {
    fn apply_record(&mut self, record: RecoveryRuntimeRecord, line_number: usize) {
        match record {
            RecoveryRuntimeRecord::Event { event } => {
                self.apply_event(event, Some(line_number), None);
            }
            RecoveryRuntimeRecord::EventBatch { events } => {
                for (index, value) in events.into_iter().enumerate() {
                    match serde_json::from_value::<AgentRuntimeEvent>(value) {
                        Ok(event) => self.apply_event(event, Some(line_number), Some(index)),
                        Err(source) => {
                            self.stats.records_skipped += 1;
                            self.warn(
                                Some(line_number),
                                Some(index),
                                "events_batch_item_malformed",
                                format!("runtime event batch item is malformed: {source}"),
                            );
                        }
                    }
                }
            }
            RecoveryRuntimeRecord::ThreadInvalidated {
                thread_id,
                latest_seq,
            } => {
                self.events.remove(&thread_id);
                self.latest_seq.insert(thread_id, latest_seq);
                self.stopped_threads.remove(&thread_id);
                self.stats.records_recovered += 1;
            }
        }
    }

    fn apply_event(
        &mut self,
        event: AgentRuntimeEvent,
        line: Option<usize>,
        record_index: Option<usize>,
    ) {
        if self.stopped_threads.contains(&event.thread_id) {
            self.stats.records_skipped += 1;
            return;
        }

        let latest_seq = self.latest_seq.get(&event.thread_id).copied().unwrap_or(0);
        if event.seq <= latest_seq {
            self.stats.records_skipped += 1;
            self.stopped_threads.insert(event.thread_id);
            self.warn(
                line,
                record_index,
                "events_non_increasing_seq",
                format!(
                    "runtime event seq must increase for thread {}: received {}, latest {}",
                    event.thread_id, event.seq, latest_seq
                ),
            );
            return;
        }

        self.latest_seq.insert(event.thread_id, event.seq);
        self.first_thread_id.get_or_insert(event.thread_id);
        self.events.entry(event.thread_id).or_default().push(event);
        self.stats.records_recovered += 1;
    }

    fn warn(
        &mut self,
        line: Option<usize>,
        record_index: Option<usize>,
        code: &'static str,
        message: impl Into<String>,
    ) {
        self.warnings.push(RecoveryWarning {
            source: RecoverySource::EventsJsonl,
            line,
            record_index,
            severity: RecoverySeverity::Warning,
            code: code.to_string(),
            message: message.into(),
        });
    }
}

fn line_is_blank(line: &[u8]) -> bool {
    line.iter().all(u8::is_ascii_whitespace)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::agent::runtime::chat::{AgentRuntimeEventPayload, TurnId, TurnSnapshot, TurnStatus};

    fn test_event(thread_id: ThreadId, seq: u64) -> AgentRuntimeEvent {
        let now = chrono::Utc::now();
        let turn = TurnSnapshot {
            turn_id: TurnId::new(thread_id, 0, seq),
            thread_id,
            status: TurnStatus::Queued,
            message_ids: Vec::new(),
            active_tool_call_ids: Vec::new(),
            error: None,
            queued_at: now,
            started_at: None,
            completed_at: None,
        };

        AgentRuntimeEvent::new(
            seq,
            thread_id,
            Some(turn.turn_id),
            AgentRuntimeEventPayload::TurnQueued { turn },
        )
    }

    fn event_record(event: AgentRuntimeEvent) -> String {
        serde_json::json!({
            "type": "event",
            "event": event,
        })
        .to_string()
    }

    #[test]
    fn recovery_accepts_final_json_line_without_newline() {
        let temp = tempdir().expect("tempdir should be created");
        let path = temp.path().join("events.jsonl");
        let thread_id = ThreadId::new();
        fs::write(&path, event_record(test_event(thread_id, 1)))
            .expect("events jsonl should be written");

        let recovered = recover_events(&path).expect("events should recover");

        assert_eq!(recovered.events.len(), 1);
        assert_eq!(recovered.events[0].seq, 1);
        assert_eq!(recovered.stats.records_read, 1);
        assert_eq!(recovered.stats.records_recovered, 1);
        assert!(recovered
            .warnings
            .iter()
            .any(|warning| warning.code == "events_final_line_missing_newline"));
    }

    #[test]
    fn recovery_handles_event_batch_and_thread_invalidated() {
        let temp = tempdir().expect("tempdir should be created");
        let path = temp.path().join("events.jsonl");
        let thread_id = ThreadId::new();
        let batch = serde_json::json!({
            "type": "event_batch",
            "events": [
                test_event(thread_id, 1),
                test_event(thread_id, 2),
            ],
        });
        let invalidated = serde_json::json!({
            "type": "thread_invalidated",
            "thread_id": thread_id,
            "latest_seq": 2,
        });
        fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                batch,
                invalidated,
                event_record(test_event(thread_id, 3))
            ),
        )
        .expect("events jsonl should be written");

        let recovered = recover_events(&path).expect("events should recover");
        let seqs = recovered
            .events
            .iter()
            .map(|event| event.seq)
            .collect::<Vec<_>>();

        assert_eq!(seqs, vec![3]);
        assert_eq!(recovered.stats.records_read, 3);
        assert_eq!(recovered.stats.records_recovered, 4);
    }
}
