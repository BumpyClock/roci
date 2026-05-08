use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::types::ModelMessage;

use super::snapshot::ThreadId;
use super::{SessionError, SessionResult};

const PROVIDER_LEDGER_SCHEMA_VERSION: u16 = 1;

/// Provider message ledger record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderLedgerRecord {
    /// Append one provider message.
    Message {
        schema_version: u16,
        seq: u64,
        thread_id: ThreadId,
        message: ModelMessage,
    },
    /// Replace effective provider history through a sequence.
    Compacted {
        schema_version: u16,
        seq: u64,
        thread_id: ThreadId,
        replacement_history: Vec<ModelMessage>,
        replaces_through_seq: u64,
    },
    /// Mark provider ledger history invalid.
    LedgerInvalidated {
        schema_version: u16,
        seq: u64,
        thread_id: ThreadId,
        latest_seq: u64,
    },
}

impl ProviderLedgerRecord {
    #[must_use]
    pub const fn seq(&self) -> u64 {
        match self {
            Self::Message { seq, .. }
            | Self::Compacted { seq, .. }
            | Self::LedgerInvalidated { seq, .. } => *seq,
        }
    }

    #[must_use]
    pub const fn thread_id(&self) -> ThreadId {
        match self {
            Self::Message { thread_id, .. }
            | Self::Compacted { thread_id, .. }
            | Self::LedgerInvalidated { thread_id, .. } => *thread_id,
        }
    }
}

/// Replayed provider ledger state.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderLedgerState {
    pub latest_seq: u64,
    pub latest_thread_id: Option<ThreadId>,
    pub effective_history: Vec<ModelMessage>,
}

impl ProviderLedgerState {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            latest_seq: 0,
            latest_thread_id: None,
            effective_history: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct ProviderLedgerInner {
    latest_seq: u64,
    latest_thread_id: Option<ThreadId>,
    histories: HashMap<ThreadId, Vec<ModelMessage>>,
}

impl ProviderLedgerInner {
    fn state(&self) -> ProviderLedgerState {
        let effective_history = self
            .latest_thread_id
            .and_then(|thread_id| self.histories.get(&thread_id))
            .cloned()
            .unwrap_or_default();
        ProviderLedgerState {
            latest_seq: self.latest_seq,
            latest_thread_id: self.latest_thread_id,
            effective_history,
        }
    }

    fn apply(&mut self, record: &ProviderLedgerRecord) {
        self.latest_seq = record.seq();
        self.latest_thread_id = Some(record.thread_id());
        match record {
            ProviderLedgerRecord::Message {
                thread_id, message, ..
            } => {
                self.histories
                    .entry(*thread_id)
                    .or_default()
                    .push(message.clone());
            }
            ProviderLedgerRecord::Compacted {
                thread_id,
                replacement_history,
                ..
            } => {
                self.histories
                    .insert(*thread_id, replacement_history.clone());
            }
            ProviderLedgerRecord::LedgerInvalidated { thread_id, .. } => {
                self.histories.insert(*thread_id, Vec::new());
            }
        }
    }
}

/// Provider ledger snapshot cache.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProviderLedgerSnapshot {
    pub schema_version: u16,
    pub thread_id: ThreadId,
    pub latest_seq: u64,
    pub effective_history: Vec<ModelMessage>,
    pub generated_at: DateTime<Utc>,
}

/// Append-only local provider message ledger.
#[derive(Debug)]
pub struct LocalProviderLedger {
    path: PathBuf,
    inner: Mutex<ProviderLedgerInner>,
}

impl LocalProviderLedger {
    /// Open a provider ledger and replay existing records.
    ///
    /// # Errors
    ///
    /// Returns an error when the ledger cannot be read or replayed strictly.
    pub fn open(path: impl Into<PathBuf>) -> SessionResult<Self> {
        let path = path.into();
        let inner = replay_provider_ledger(&path)?;
        Ok(Self {
            path,
            inner: Mutex::new(inner),
        })
    }

    /// Return replayed provider ledger state.
    ///
    /// # Panics
    ///
    /// Panics only if another thread panicked while holding the ledger mutex.
    #[must_use]
    pub fn state(&self) -> ProviderLedgerState {
        self.inner
            .lock()
            .expect("provider ledger mutex poisoned")
            .state()
    }

    /// Return replayed provider ledger state for one thread.
    ///
    /// # Panics
    ///
    /// Panics only if another thread panicked while holding the ledger mutex.
    #[must_use]
    pub fn state_for_thread(&self, thread_id: ThreadId) -> ProviderLedgerState {
        let inner = self.inner.lock().expect("provider ledger mutex poisoned");
        ProviderLedgerState {
            latest_seq: inner.latest_seq,
            latest_thread_id: Some(thread_id),
            effective_history: inner.histories.get(&thread_id).cloned().unwrap_or_default(),
        }
    }

    /// Append one provider message.
    ///
    /// # Errors
    ///
    /// Returns an error when the ledger file cannot be appended.
    pub fn append_message(
        &self,
        thread_id: ThreadId,
        message: ModelMessage,
    ) -> SessionResult<ProviderLedgerRecord> {
        self.append_with(|inner| ProviderLedgerRecord::Message {
            schema_version: PROVIDER_LEDGER_SCHEMA_VERSION,
            seq: inner.latest_seq + 1,
            thread_id,
            message,
        })
    }

    /// Append provider ledger compaction.
    ///
    /// # Errors
    ///
    /// Returns an error when the ledger file cannot be appended.
    pub fn append_compacted(
        &self,
        thread_id: ThreadId,
        replacement_history: Vec<ModelMessage>,
    ) -> SessionResult<ProviderLedgerRecord> {
        self.append_with(|inner| ProviderLedgerRecord::Compacted {
            schema_version: PROVIDER_LEDGER_SCHEMA_VERSION,
            seq: inner.latest_seq + 1,
            thread_id,
            replacement_history,
            replaces_through_seq: inner.latest_seq,
        })
    }

    /// Append provider ledger invalidation.
    ///
    /// # Errors
    ///
    /// Returns an error when the ledger file cannot be appended.
    pub fn append_ledger_invalidated(
        &self,
        thread_id: ThreadId,
    ) -> SessionResult<ProviderLedgerRecord> {
        self.append_with(|inner| ProviderLedgerRecord::LedgerInvalidated {
            schema_version: PROVIDER_LEDGER_SCHEMA_VERSION,
            seq: inner.latest_seq + 1,
            thread_id,
            latest_seq: inner.latest_seq,
        })
    }

    fn append_with(
        &self,
        build: impl FnOnce(&ProviderLedgerInner) -> ProviderLedgerRecord,
    ) -> SessionResult<ProviderLedgerRecord> {
        let mut inner = self.inner.lock().expect("provider ledger mutex poisoned");
        let record = build(&inner);
        append_record(&self.path, &record)?;
        inner.apply(&record);
        Ok(record)
    }
}

fn replay_provider_ledger(path: &Path) -> SessionResult<ProviderLedgerInner> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Default::default())
        }
        Err(source) => return Err(SessionError::io(path, source)),
    };

    if has_final_nonblank_line_without_newline(&bytes) {
        return Err(SessionError::InvalidProviderLedger {
            path: path.to_path_buf(),
            message: "final nonblank line missing trailing newline".to_string(),
        });
    }

    let text =
        std::str::from_utf8(&bytes).map_err(|source| SessionError::InvalidProviderLedger {
            path: path.to_path_buf(),
            message: source.to_string(),
        })?;

    let mut inner = ProviderLedgerInner::default();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: ProviderLedgerRecord =
            serde_json::from_str(line).map_err(|source| SessionError::InvalidProviderLedger {
                path: path.to_path_buf(),
                message: format!("line {}: {}", index + 1, source),
            })?;
        if record.seq() <= inner.latest_seq {
            return Err(SessionError::InvalidProviderLedger {
                path: path.to_path_buf(),
                message: format!("line {}: seq must increase", index + 1),
            });
        }
        inner.apply(&record);
    }

    Ok(inner)
}

fn has_final_nonblank_line_without_newline(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes.ends_with(b"\n") {
        return false;
    }
    let start = bytes
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |index| index + 1);
    bytes[start..]
        .iter()
        .any(|byte| !byte.is_ascii_whitespace())
}

fn append_record(path: &Path, record: &ProviderLedgerRecord) -> SessionResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|source| SessionError::io(parent, source))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| SessionError::io(path, source))?;
    serde_json::to_writer(&mut file, record).map_err(|source| {
        SessionError::InvalidProviderLedger {
            path: path.to_path_buf(),
            message: source.to_string(),
        }
    })?;
    file.write_all(b"\n")
        .map_err(|source| SessionError::io(path, source))?;
    file.flush()
        .map_err(|source| SessionError::io(path, source))?;
    file.sync_data()
        .map_err(|source| SessionError::io(path, source))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_provider_ledger_replays_empty_state() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ledger = LocalProviderLedger::open(temp.path().join("missing.jsonl")).expect("open");

        assert_eq!(ledger.state(), ProviderLedgerState::empty());
    }

    #[test]
    fn provider_ledger_appends_and_replays_messages() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("model_messages.jsonl");
        let thread_id = ThreadId::new();
        let ledger = LocalProviderLedger::open(&path).expect("open");

        ledger
            .append_message(thread_id, ModelMessage::user("hello"))
            .expect("append message");
        ledger
            .append_message(thread_id, ModelMessage::assistant("hi"))
            .expect("append message");

        let replayed = LocalProviderLedger::open(&path).expect("replay");
        let state = replayed.state();
        assert_eq!(state.latest_seq, 2);
        assert_eq!(state.latest_thread_id, Some(thread_id));
        assert_eq!(
            state
                .effective_history
                .iter()
                .map(ModelMessage::text)
                .collect::<Vec<_>>(),
            vec!["hello", "hi"]
        );
    }

    #[test]
    fn provider_ledger_compaction_replaces_effective_history() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("model_messages.jsonl");
        let thread_id = ThreadId::new();
        let ledger = LocalProviderLedger::open(&path).expect("open");

        ledger
            .append_message(thread_id, ModelMessage::user("old"))
            .expect("append message");
        ledger
            .append_compacted(thread_id, vec![ModelMessage::system("summary")])
            .expect("append compacted");
        ledger
            .append_message(thread_id, ModelMessage::user("new"))
            .expect("append message");

        let replayed = LocalProviderLedger::open(&path).expect("replay");
        assert_eq!(
            replayed
                .state()
                .effective_history
                .iter()
                .map(ModelMessage::text)
                .collect::<Vec<_>>(),
            vec!["summary", "new"]
        );
    }

    #[test]
    fn provider_ledger_invalidation_clears_effective_history() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("model_messages.jsonl");
        let thread_id = ThreadId::new();
        let ledger = LocalProviderLedger::open(&path).expect("open");

        ledger
            .append_message(thread_id, ModelMessage::user("old"))
            .expect("append message");
        ledger
            .append_ledger_invalidated(thread_id)
            .expect("append invalidation");

        assert!(LocalProviderLedger::open(&path)
            .expect("replay")
            .state()
            .effective_history
            .is_empty());
    }

    #[test]
    fn provider_ledger_ignores_blank_lines() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("model_messages.jsonl");
        fs::write(&path, b"\n  \n\t\n").expect("write");

        assert_eq!(
            LocalProviderLedger::open(&path).expect("open").state(),
            ProviderLedgerState::empty()
        );
    }

    #[test]
    fn provider_ledger_reports_malformed_line_with_path_and_line() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("model_messages.jsonl");
        fs::write(&path, b"\n{\"bad\": true}\n").expect("write");

        let error = LocalProviderLedger::open(&path).expect_err("invalid ledger");
        match error {
            SessionError::InvalidProviderLedger {
                path: err_path,
                message,
            } => {
                assert_eq!(err_path, path);
                assert!(message.contains("line 2"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn provider_ledger_rejects_final_nonblank_line_without_trailing_newline() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("model_messages.jsonl");
        fs::write(&path, br#"{"type":"message"}"#).expect("write");

        let error = LocalProviderLedger::open(&path).expect_err("invalid ledger");
        match error {
            SessionError::InvalidProviderLedger {
                path: err_path,
                message,
            } => {
                assert_eq!(err_path, path);
                assert!(message.contains("trailing newline"));
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
