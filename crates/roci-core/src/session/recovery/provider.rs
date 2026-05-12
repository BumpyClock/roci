//! Tolerant provider ledger recovery helpers.

use std::collections::{HashMap, HashSet};
use std::io::ErrorKind;
use std::path::Path;

use crate::session::{ProviderLedgerRecord, SessionError, SessionResult, ThreadId};
use crate::types::ModelMessage;

use super::{
    RecoveredProviderLedgerScan, RecoverySeverity, RecoverySource, RecoverySourceStats,
    RecoveryWarning,
};

#[derive(Debug, Default)]
struct RecoveryProviderLedgerState {
    histories: HashMap<ThreadId, Vec<ModelMessage>>,
    recovered_threads: HashSet<ThreadId>,
    latest_seq: u64,
    stopped: bool,
    degraded: bool,
    stats: RecoverySourceStats,
    warnings: Vec<RecoveryWarning>,
}

pub(crate) fn recover_provider_ledger(
    path: impl AsRef<Path>,
) -> SessionResult<RecoveredProviderLedgerScan> {
    let path = path.as_ref();
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(SessionError::io(path, source)),
    };
    let mut state = RecoveryProviderLedgerState::default();

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
                "provider_final_line_missing_newline",
                "provider ledger final line is missing trailing newline",
                false,
            );
        }

        if state.stopped {
            state.stats.records_skipped += 1;
            continue;
        }

        let Ok(text) = std::str::from_utf8(line) else {
            state.stats.records_skipped += 1;
            state.warn(
                Some(line_number),
                None,
                "provider_invalid_utf8",
                "provider ledger record is not valid UTF-8",
                true,
            );
            continue;
        };

        let record = match serde_json::from_str::<ProviderLedgerRecord>(text) {
            Ok(record) => record,
            Err(source) => {
                state.stats.records_skipped += 1;
                state.warn(
                    Some(line_number),
                    None,
                    "provider_malformed_json",
                    format!("provider ledger record is malformed: {source}"),
                    true,
                );
                continue;
            }
        };

        state.apply_record(record, line_number);
    }

    let mut recovered_threads = state.recovered_threads.into_iter().collect::<Vec<_>>();
    recovered_threads.sort_by_key(thread_sort_key);
    state.stats.warnings = state.warnings.len();

    Ok(RecoveredProviderLedgerScan {
        histories: state.histories,
        recovered_threads,
        latest_seq: state.latest_seq,
        degraded: state.degraded,
        stats: state.stats,
        warnings: state.warnings,
    })
}

impl RecoveryProviderLedgerState {
    fn apply_record(&mut self, record: ProviderLedgerRecord, line_number: usize) {
        let seq = record.seq();
        if seq <= self.latest_seq {
            self.stats.records_skipped += 1;
            self.stopped = true;
            self.warn(
                Some(line_number),
                None,
                "provider_non_increasing_seq",
                format!(
                    "provider ledger seq must increase globally: received {seq}, latest {}",
                    self.latest_seq
                ),
                true,
            );
            return;
        }

        self.latest_seq = seq;
        self.recovered_threads.insert(record.thread_id());
        match record {
            ProviderLedgerRecord::Message {
                thread_id, message, ..
            } => {
                self.histories.entry(thread_id).or_default().push(message);
            }
            ProviderLedgerRecord::Compacted {
                thread_id,
                replacement_history,
                ..
            } => {
                self.histories.insert(thread_id, replacement_history);
            }
            ProviderLedgerRecord::LedgerInvalidated { thread_id, .. } => {
                self.histories.insert(thread_id, Vec::new());
            }
        }
        self.stats.records_recovered += 1;
    }

    fn warn(
        &mut self,
        line: Option<usize>,
        record_index: Option<usize>,
        code: &'static str,
        message: impl Into<String>,
        marks_degraded: bool,
    ) {
        self.degraded |= marks_degraded;
        self.warnings.push(RecoveryWarning {
            source: RecoverySource::ProviderLedgerJsonl,
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

fn thread_sort_key(thread_id: &ThreadId) -> String {
    serde_json::to_string(thread_id).unwrap_or_else(|_| format!("{thread_id:?}"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn provider_record(record: ProviderLedgerRecord) -> String {
        serde_json::to_string(&record).expect("provider ledger record should serialize")
    }

    fn message_texts(messages: &[ModelMessage]) -> Vec<String> {
        messages
            .iter()
            .map(|message| message.text().to_string())
            .collect()
    }

    #[test]
    fn recovery_provider_ledger_uses_compacted_checkpoint_and_valid_suffix() {
        let temp = tempdir().expect("tempdir should be created");
        let path = temp.path().join("model_messages.jsonl");
        let thread_id = ThreadId::new();
        fs::write(
            &path,
            format!(
                "{}\nnot-json\n{}\n{}",
                provider_record(ProviderLedgerRecord::Message {
                    schema_version: 1,
                    seq: 1,
                    thread_id,
                    message: ModelMessage::user("old"),
                }),
                provider_record(ProviderLedgerRecord::Compacted {
                    schema_version: 1,
                    seq: 2,
                    thread_id,
                    replacement_history: vec![ModelMessage::user("checkpoint")],
                    replaces_through_seq: 1,
                }),
                provider_record(ProviderLedgerRecord::Message {
                    schema_version: 1,
                    seq: 3,
                    thread_id,
                    message: ModelMessage::assistant("suffix"),
                }),
            ),
        )
        .expect("provider ledger jsonl should be written");

        let recovered = recover_provider_ledger(&path).expect("provider ledger should recover");
        let effective_history = recovered
            .histories
            .get(&thread_id)
            .expect("thread history should recover");

        assert_eq!(
            message_texts(effective_history),
            vec!["checkpoint", "suffix"]
        );
        assert!(recovered.degraded);
        assert_eq!(recovered.stats.records_read, 4);
        assert_eq!(recovered.stats.records_recovered, 3);
        assert_eq!(recovered.stats.records_skipped, 1);
        assert!(recovered
            .warnings
            .iter()
            .any(|warning| warning.code == "provider_malformed_json"));
    }

    #[test]
    fn recovery_provider_ledger_stops_on_global_non_increasing_seq() {
        let temp = tempdir().expect("tempdir should be created");
        let path = temp.path().join("model_messages.jsonl");
        let thread_id = ThreadId::new();
        fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                provider_record(ProviderLedgerRecord::Message {
                    schema_version: 1,
                    seq: 2,
                    thread_id,
                    message: ModelMessage::user("first"),
                }),
                provider_record(ProviderLedgerRecord::Message {
                    schema_version: 1,
                    seq: 2,
                    thread_id,
                    message: ModelMessage::assistant("duplicate"),
                }),
                provider_record(ProviderLedgerRecord::Message {
                    schema_version: 1,
                    seq: 3,
                    thread_id,
                    message: ModelMessage::assistant("tail"),
                }),
            ),
        )
        .expect("provider ledger jsonl should be written");

        let recovered = recover_provider_ledger(&path).expect("provider ledger should recover");
        let effective_history = recovered
            .histories
            .get(&thread_id)
            .expect("thread history should recover");

        assert_eq!(message_texts(effective_history), vec!["first"]);
        assert_eq!(recovered.latest_seq, 2);
        assert!(recovered.degraded);
        assert_eq!(recovered.stats.records_read, 3);
        assert_eq!(recovered.stats.records_recovered, 1);
        assert_eq!(recovered.stats.records_skipped, 2);
        assert!(recovered
            .warnings
            .iter()
            .any(|warning| warning.code == "provider_non_increasing_seq"));
    }
}
