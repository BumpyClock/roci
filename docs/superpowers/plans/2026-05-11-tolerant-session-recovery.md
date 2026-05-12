# Tolerant Session Recovery Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a separate tolerant session recovery/export/import path for damaged durable session artifacts while keeping normal session replay strict.

**Architecture:** Add a feature-gated `session::recovery` module in `roci-core` with typed recovery artifacts, tolerant JSONL scanners, and report structs. Keep strict `JsonlAgentRuntimeEventStore` and `LocalProviderLedger::open` unchanged; `LocalSessionStore` calls recovery helpers and imports through a staging directory before atomic target rename. `roci-cli` wires `session recover-export` and `session recover-import` so live smoke tests exercise the real SDK path.

**Tech Stack:** Rust 2021, `serde`, `serde_json`, `tokio`, existing `roci-core` session/runtime modules, `clap` CLI, `cargo test`, `rustfmt`, `clippy`, tmux live verification per `docs/testing.md`.

---

## File Map

- Create: `crates/roci-core/src/session/recovery/mod.rs` - public recovery types, artifact envelope constants, top-level helper functions.
- Create: `crates/roci-core/src/session/recovery/events.rs` - tolerant `events.jsonl` scanner for `event`, `event_batch`, and `thread_invalidated` records.
- Create: `crates/roci-core/src/session/recovery/provider.rs` - tolerant provider ledger scanner for `PathConventions::provider_ledger_file()` (`model_messages.jsonl`) and default-thread provider summary builder.
- Modify: `crates/roci-core/src/session/mod.rs` - export recovery module/types behind `agent` feature.
- Modify: `crates/roci-core/src/session/error.rs` - add recovery-specific `SessionError` variants.
- Modify: `crates/roci-core/src/session/store.rs` - add `LocalSessionStore::{recover_export,recover_import}` and expose small `pub(crate)` helpers where needed.
- Modify: `crates/roci-core/src/agent/runtime_tests/mod.rs` - include session recovery tests.
- Create: `crates/roci-core/src/agent/runtime_tests/session_recovery.rs` - core recovery tests.
- Modify: `crates/roci-cli/src/cli/mod.rs` - add `recover-export` / `recover-import` args.
- Modify: `crates/roci-cli/src/session_cmd.rs` - implement CLI handlers, summaries, JSON output, and tests.
- Modify: `docs/testing.md` - add durable recovery smoke and live provider resume smoke commands.

Dependency order:

- Task 1 defines shared public types and must land before scanner/store/CLI work.
- Task 2 and Task 3 can run in parallel after Task 1 because they own disjoint scanner files.
- Task 4 starts after Task 1 and can integrate Task 2/3 scanner APIs as they land.
- Task 5 starts after Task 4 exposes compiling public store methods.
- Task 6 runs after CLI commands exist.

Parallel ownership:

- Worker A owns `crates/roci-core/src/session/recovery/*` and core scanner tests.
- Worker B owns `crates/roci-core/src/session/store.rs`, `session/mod.rs`, `session/error.rs`, and import/export tests.
- Worker C owns `crates/roci-cli/src/cli/mod.rs`, `crates/roci-cli/src/session_cmd.rs`, and CLI tests after core public API compiles.
- Integration owner resolves compile/API mismatches, updates `docs/testing.md`, runs gates and live smoke.

## Task 1: Recovery Types And Module Exports

**Files:**
- Create: `crates/roci-core/src/session/recovery/mod.rs`
- Create: `crates/roci-core/src/session/recovery/events.rs`
- Create: `crates/roci-core/src/session/recovery/provider.rs`
- Modify: `crates/roci-core/src/session/mod.rs`
- Modify: `crates/roci-core/src/session/error.rs`

- [ ] **Step 1: Create recovery module skeleton**

Add `crates/roci-core/src/session/recovery/mod.rs`:

```rust
//! Tolerant recovery for damaged local session artifacts.

mod events;
mod provider;

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    RuntimeCursor, SessionId, SessionSnapshot, SessionResult, ThreadId,
};

pub const RECOVERED_SESSION_ARTIFACT_TYPE: &str = "roci_recovered_session";
pub const RECOVERED_SESSION_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveredSession {
    pub artifact_type: String,
    pub schema_version: u16,
    pub snapshot: SessionSnapshot,
    pub report: RecoveryReport,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub importable_runtime_state: bool,
    pub sources: Vec<RecoverySourceReport>,
    pub warnings: Vec<RecoveryWarning>,
    pub stats: RecoveryStats,
    pub cache_preview: Option<RuntimeSnapshotCachePreview>,
    pub provider_context: ProviderRecoveryReport,
    pub resource_refs_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoverySourceReport {
    pub source: RecoverySource,
    pub path: PathBuf,
    pub status: RecoverySourceStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySource {
    Metadata,
    EventsJsonl,
    ProviderLedgerJsonl,
    RuntimeSnapshotCache,
    Resources,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySourceStatus {
    Missing,
    Read,
    RecoveredWithWarnings,
    Unusable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoverySeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryWarning {
    pub source: RecoverySource,
    pub line: Option<usize>,
    pub record_index: Option<usize>,
    pub severity: RecoverySeverity,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecoveryStats {
    pub events: RecoverySourceStats,
    pub provider_ledger: RecoverySourceStats,
    pub resources: RecoverySourceStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RecoverySourceStats {
    pub records_read: usize,
    pub records_recovered: usize,
    pub records_skipped: usize,
    pub warnings: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RuntimeSnapshotCachePreview {
    pub parsed: bool,
    pub generated_at: Option<DateTime<Utc>>,
    pub thread_count: Option<usize>,
    pub latest_cursors: Vec<RuntimeCursor>,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderRecoveryReport {
    pub default_thread_id: ThreadId,
    pub recovered_threads: Vec<ThreadId>,
    pub imported_thread_id: ThreadId,
    pub degraded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionRecoverySource {
    SessionId(SessionId),
    SessionDir {
        path: PathBuf,
        source_id: Option<SessionId>,
    },
}

pub(crate) struct RecoveredEvents {
    pub events: Vec<super::AgentRuntimeEvent>,
    pub stats: RecoverySourceStats,
    pub warnings: Vec<RecoveryWarning>,
}

pub(crate) struct RecoveredProviderLedger {
    pub summary: super::ProviderLedgerSummary,
    pub report: ProviderRecoveryReport,
    pub stats: RecoverySourceStats,
    pub warnings: Vec<RecoveryWarning>,
}
```

Create placeholder scanner files so `mod events; mod provider;` compiles before
Task 2 and Task 3 fill them:

```rust
//! Tolerant runtime event recovery helpers.
```

```rust
//! Tolerant provider ledger recovery helpers.
```

- [ ] **Step 2: Export module from session root**

Patch `crates/roci-core/src/session/mod.rs`:

```rust
#[cfg(feature = "agent")]
pub mod recovery;

#[cfg(feature = "agent")]
pub use recovery::{
    ProviderRecoveryReport, RecoveredSession, RecoveryReport, RecoverySeverity, RecoverySource,
    RecoverySourceReport, RecoverySourceStats, RecoverySourceStatus, RecoveryStats,
    RuntimeSnapshotCachePreview, SessionRecoverySource, RECOVERED_SESSION_ARTIFACT_TYPE,
};
```

- [ ] **Step 3: Add recovery errors**

Patch `crates/roci-core/src/session/error.rs`:

```rust
    #[error("invalid recovered session artifact: {message}")]
    InvalidRecoveredSession { message: String },
    #[error("session recovery artifact is not importable: {message}")]
    NonImportableRecovery { message: String },
```

- [ ] **Step 4: Run focused compile check**

Run:

```bash
cargo check -p roci-core --features agent
```

Expected: compile may fail until later tasks wire private helpers, but new type names and serde derives should parse.

## Task 2: Tolerant Runtime Event Scanner

**Files:**
- Create: `crates/roci-core/src/session/recovery/events.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/session_recovery.rs`
- Modify: `crates/roci-core/src/agent/runtime_tests/mod.rs`

- [ ] **Step 1: Add failing tests for JSONL recovery envelope behavior**

Add module to `crates/roci-core/src/agent/runtime_tests/mod.rs`:

```rust
mod session_recovery;
```

Create `crates/roci-core/src/agent/runtime_tests/session_recovery.rs` with first tests:

```rust
use super::*;
use crate::agent::runtime::chat::{
    AgentRuntimeEvent, AgentRuntimeEventPayload, JsonlAgentRuntimeEventStore, ThreadId,
    TurnId, TurnSnapshot, TurnStatus,
};
use crate::session::{
    LocalSessionStore, PathConventions, RecoverySource, SessionRecoverySource, SessionId,
};

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

#[tokio::test]
async fn recovery_accepts_final_json_line_without_newline_but_open_stays_strict() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("sessions");
    let id = SessionId::parse("recover-final-line").unwrap();
    let store = LocalSessionStore::new(&root);
    store
        .create(crate::session::CreateSessionOptions {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();

    let events_path = root.join(id.as_str()).join("events.jsonl");
    let event = test_event(ThreadId::new(), 1);
    let line = serde_json::json!({ "type": "event", "event": event }).to_string();
    std::fs::write(&events_path, line).unwrap();

    assert!(store.open(id.clone()).await.is_err());

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(id))
        .await
        .unwrap();
    assert!(recovered
        .report
        .warnings
        .iter()
        .any(|warning| warning.code == "events_final_line_missing_newline"));
    assert_eq!(recovered.snapshot.events.len(), 1);
}

#[tokio::test]
async fn recovery_handles_event_batch_and_thread_invalidated() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("sessions");
    let id = SessionId::parse("recover-batch-invalidation").unwrap();
    let thread = ThreadId::new();
    let store = LocalSessionStore::new(&root);
    store
        .create(crate::session::CreateSessionOptions {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let path = root.join(id.as_str()).join("events.jsonl");
    let records = [
        serde_json::json!({
            "type": "event_batch",
            "events": [test_event(thread, 1), test_event(thread, 2)]
        })
        .to_string(),
        serde_json::json!({
            "type": "thread_invalidated",
            "thread_id": thread,
            "latest_seq": 2
        })
        .to_string(),
        serde_json::json!({
            "type": "event",
            "event": test_event(thread, 3)
        })
        .to_string(),
    ]
    .join("\n");
    std::fs::write(&path, format!("{records}\n")).unwrap();

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(id))
        .await
        .unwrap();

    assert_eq!(recovered.snapshot.events.len(), 1);
    assert_eq!(recovered.snapshot.events[0].seq, 3);
}
```

Run:

```bash
cargo test -p roci-core --features agent session_recovery -- --nocapture
```

Expected: fail because recovery methods do not exist yet.

- [ ] **Step 2: Implement tolerant line scanner and runtime record replay**

Add `crates/roci-core/src/session/recovery/events.rs`:

```rust
use std::collections::{HashMap, VecDeque};
use std::path::Path;

use serde::Deserialize;

use crate::agent::runtime::chat::{AgentRuntimeEvent, ThreadId};

use super::{
    RecoveredEvents, RecoverySeverity, RecoverySource, RecoverySourceStats, RecoveryWarning,
};
use crate::session::{SessionError, SessionResult};

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum RecoveryRuntimeRecord {
    Event { event: AgentRuntimeEvent },
    EventBatch { events: Vec<serde_json::Value> },
    ThreadInvalidated { thread_id: ThreadId, latest_seq: u64 },
}

#[derive(Default)]
struct ThreadRecoveryState {
    latest_seq: u64,
    events: VecDeque<AgentRuntimeEvent>,
    stopped: bool,
}

pub(crate) fn recover_events(path: impl AsRef<Path>) -> SessionResult<RecoveredEvents> {
    let path = path.as_ref();
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(SessionError::io(path, source)),
    };

    let mut stats = RecoverySourceStats::default();
    let mut warnings = Vec::new();
    let mut threads: HashMap<ThreadId, ThreadRecoveryState> = HashMap::new();
    let lines = split_jsonl_lines(&bytes);

    for (line_index, raw_line, missing_newline) in lines {
        let line_number = line_index + 1;
        if raw_line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        stats.records_read += 1;
        if missing_newline {
            warn(
                &mut warnings,
                &mut stats,
                line_number,
                None,
                "events_final_line_missing_newline",
                "final nonblank event record is missing trailing newline",
            );
        }
        let Ok(line) = std::str::from_utf8(raw_line) else {
            stats.records_skipped += 1;
            warn(
                &mut warnings,
                &mut stats,
                line_number,
                None,
                "events_invalid_utf8",
                "event record is not valid utf-8",
            );
            continue;
        };
        let Ok(record) = serde_json::from_str::<RecoveryRuntimeRecord>(line) else {
            stats.records_skipped += 1;
            warn(
                &mut warnings,
                &mut stats,
                line_number,
                None,
                "events_malformed_json",
                "event record is not valid runtime JSONL",
            );
            continue;
        };
        apply_record(record, line_number, &mut threads, &mut warnings, &mut stats);
    }

    let mut events = threads
        .into_values()
        .flat_map(|thread| thread.events)
        .collect::<Vec<_>>();
    events.sort_by_key(|event| (event.thread_id.to_string(), event.seq));

    Ok(RecoveredEvents {
        events,
        stats,
        warnings,
    })
}
```

Add helper functions in same file:

```rust
fn split_jsonl_lines(bytes: &[u8]) -> Vec<(usize, &[u8], bool)> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    let mut index = 0;
    for (pos, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            let end = pos;
            out.push((index, trim_line_ending(&bytes[start..end]), false));
            start = pos + 1;
            index += 1;
        }
    }
    if start < bytes.len() {
        out.push((index, trim_line_ending(&bytes[start..]), true));
    }
    out
}

fn trim_line_ending(line: &[u8]) -> &[u8] {
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn apply_record(
    record: RecoveryRuntimeRecord,
    line_number: usize,
    threads: &mut HashMap<ThreadId, ThreadRecoveryState>,
    warnings: &mut Vec<RecoveryWarning>,
    stats: &mut RecoverySourceStats,
) {
    match record {
        RecoveryRuntimeRecord::Event { event } => {
            apply_event(event, line_number, None, threads, warnings, stats);
        }
        RecoveryRuntimeRecord::EventBatch { events } => {
            for (index, value) in events.into_iter().enumerate() {
                match serde_json::from_value::<AgentRuntimeEvent>(value) {
                    Ok(event) => {
                        apply_event(event, line_number, Some(index), threads, warnings, stats);
                    }
                    Err(source) => {
                        stats.records_skipped += 1;
                        warn_with_index(
                            warnings,
                            stats,
                            line_number,
                            Some(index),
                            "events_batch_item_malformed",
                            &format!("event_batch item is not a valid runtime event: {source}"),
                        );
                    }
                }
            }
        }
        RecoveryRuntimeRecord::ThreadInvalidated {
            thread_id,
            latest_seq,
        } => {
            let thread = threads.entry(thread_id).or_default();
            thread.latest_seq = latest_seq;
            thread.events.clear();
            thread.stopped = false;
            stats.records_recovered += 1;
        }
    }
}

fn apply_event(
    event: AgentRuntimeEvent,
    line_number: usize,
    record_index: Option<usize>,
    threads: &mut HashMap<ThreadId, ThreadRecoveryState>,
    warnings: &mut Vec<RecoveryWarning>,
    stats: &mut RecoverySourceStats,
) {
    let thread = threads.entry(event.thread_id).or_default();
    if thread.stopped {
        stats.records_skipped += 1;
        return;
    }
    if event.seq <= thread.latest_seq {
        thread.stopped = true;
        stats.records_skipped += 1;
        warn(
            warnings,
            stats,
            line_number,
            record_index,
            "events_non_increasing_seq",
            "runtime event seq did not increase; stopped recovery for this thread",
        );
        return;
    }
    thread.latest_seq = event.seq;
    thread.events.push_back(event);
    stats.records_recovered += 1;
}

fn warn(
    warnings: &mut Vec<RecoveryWarning>,
    stats: &mut RecoverySourceStats,
    line_number: usize,
    record_index: Option<usize>,
    code: &str,
    message: &str,
) {
    stats.warnings += 1;
    warnings.push(RecoveryWarning {
        source: RecoverySource::EventsJsonl,
        line: Some(line_number),
        record_index,
        severity: RecoverySeverity::Warning,
        code: code.to_string(),
        message: message.to_string(),
    });
}

fn warn_with_index(
    warnings: &mut Vec<RecoveryWarning>,
    stats: &mut RecoverySourceStats,
    line_number: usize,
    record_index: Option<usize>,
    code: &str,
    message: &str,
) {
    warn(warnings, stats, line_number, record_index, code, message);
}
```

- [ ] **Step 3: Run focused test**

Run:

```bash
cargo test -p roci-core --features agent session_recovery -- --nocapture
```

Expected: event scanner tests pass once store methods from Task 4 are wired; until then compile fails on missing methods.

## Task 3: Tolerant Provider Ledger Recovery

**Files:**
- Create: `crates/roci-core/src/session/recovery/provider.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/session_recovery.rs`

- [ ] **Step 1: Add provider ledger tests**

Append tests:

```rust
use crate::session::ProviderLedgerRecord;
use crate::types::ModelMessage;

#[tokio::test]
async fn recovery_provider_ledger_uses_compacted_checkpoint_and_valid_suffix() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("sessions");
    let id = SessionId::parse("recover-provider").unwrap();
    let thread = ThreadId::new();
    let store = LocalSessionStore::new(&root);
    store
        .create(crate::session::CreateSessionOptions {
            id: Some(id.clone()),
            default_thread_id: Some(thread),
            ..Default::default()
        })
        .await
        .unwrap();
    let ledger_path = PathConventions::for_session(&root, &id).provider_ledger_file();
    let records = [
        serde_json::to_string(&ProviderLedgerRecord::Message {
            schema_version: 1,
            seq: 1,
            thread_id: thread,
            message: ModelMessage::user("old"),
        })
        .unwrap(),
        "not json".to_string(),
        serde_json::to_string(&ProviderLedgerRecord::Compacted {
            schema_version: 1,
            seq: 2,
            thread_id: thread,
            replacement_history: vec![ModelMessage::user("checkpoint")],
            replaces_through_seq: 1,
        })
        .unwrap(),
        serde_json::to_string(&ProviderLedgerRecord::Message {
            schema_version: 1,
            seq: 3,
            thread_id: thread,
            message: ModelMessage::assistant("suffix"),
        })
        .unwrap(),
    ]
    .join("\n");
    std::fs::write(&ledger_path, format!("{records}\n")).unwrap();

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(id))
        .await
        .unwrap();

    let text = recovered
        .snapshot
        .provider_ledger
        .effective_history
        .iter()
        .map(ModelMessage::text)
        .collect::<Vec<_>>();
    assert_eq!(text, vec!["checkpoint", "suffix"]);
    assert!(recovered.report.provider_context.degraded);
}

#[tokio::test]
async fn recovery_provider_ledger_stops_on_global_non_increasing_seq() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("sessions");
    let id = SessionId::parse("recover-provider-seq").unwrap();
    let thread = ThreadId::new();
    let store = LocalSessionStore::new(&root);
    store
        .create(crate::session::CreateSessionOptions {
            id: Some(id.clone()),
            default_thread_id: Some(thread),
            ..Default::default()
        })
        .await
        .unwrap();
    let ledger_path = PathConventions::for_session(&root, &id).provider_ledger_file();
    let records = [
        serde_json::to_string(&ProviderLedgerRecord::Message {
            schema_version: 1,
            seq: 2,
            thread_id: thread,
            message: ModelMessage::user("kept"),
        })
        .unwrap(),
        serde_json::to_string(&ProviderLedgerRecord::Message {
            schema_version: 1,
            seq: 2,
            thread_id: thread,
            message: ModelMessage::assistant("dropped"),
        })
        .unwrap(),
        serde_json::to_string(&ProviderLedgerRecord::Message {
            schema_version: 1,
            seq: 3,
            thread_id: thread,
            message: ModelMessage::assistant("tail also dropped"),
        })
        .unwrap(),
    ]
    .join("\n");
    std::fs::write(&ledger_path, format!("{records}\n")).unwrap();

    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(id))
        .await
        .unwrap();

    assert_eq!(recovered.snapshot.provider_ledger.effective_history.len(), 1);
    assert!(recovered.report.provider_context.degraded);
}
```

- [ ] **Step 2: Implement provider recovery**

Add `crates/roci-core/src/session/recovery/provider.rs`:

```rust
use std::collections::HashMap;
use std::path::Path;

use crate::session::{
    ProviderLedgerRecord, ProviderLedgerSummary, SessionError, SessionResult, ThreadId,
};
use crate::types::ModelMessage;

use super::{
    ProviderRecoveryReport, RecoveredProviderLedger, RecoverySeverity, RecoverySource,
    RecoverySourceStats, RecoveryWarning,
};

#[derive(Default)]
struct ProviderRecoveryState {
    latest_seq: u64,
    histories: HashMap<ThreadId, Vec<ModelMessage>>,
    stopped: bool,
    degraded: bool,
}

pub(crate) fn recover_provider_ledger(
    path: impl AsRef<Path>,
    default_thread_id: ThreadId,
) -> SessionResult<RecoveredProviderLedger> {
    let path = path.as_ref();
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(SessionError::io(path, source)),
    };
    let mut stats = RecoverySourceStats::default();
    let mut warnings = Vec::new();
    let mut state = ProviderRecoveryState::default();

    for (index, raw_line, missing_newline) in split_jsonl_lines(&bytes) {
        let line_number = index + 1;
        if raw_line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        stats.records_read += 1;
        if missing_newline {
            warn(&mut warnings, &mut stats, line_number, "provider_final_line_missing_newline", "final nonblank provider ledger record is missing trailing newline");
        }
        if state.stopped {
            stats.records_skipped += 1;
            continue;
        }
        let Ok(line) = std::str::from_utf8(raw_line) else {
            state.degraded = true;
            stats.records_skipped += 1;
            warn(&mut warnings, &mut stats, line_number, "provider_invalid_utf8", "provider ledger record is not valid utf-8");
            continue;
        };
        let Ok(record) = serde_json::from_str::<ProviderLedgerRecord>(line) else {
            state.degraded = true;
            stats.records_skipped += 1;
            warn(&mut warnings, &mut stats, line_number, "provider_malformed_json", "provider ledger record is not valid JSONL");
            continue;
        };
        if record.seq() <= state.latest_seq {
            state.degraded = true;
            state.stopped = true;
            stats.records_skipped += 1;
            warn(&mut warnings, &mut stats, line_number, "provider_non_increasing_seq", "provider ledger global seq did not increase; stopped ledger recovery");
            continue;
        }
        apply_record(&mut state, record);
        stats.records_recovered += 1;
    }

    let mut recovered_threads = state.histories.keys().copied().collect::<Vec<_>>();
    recovered_threads.sort_by_key(ToString::to_string);
    let effective_history = state
        .histories
        .get(&default_thread_id)
        .cloned()
        .unwrap_or_default();

    Ok(RecoveredProviderLedger {
        summary: ProviderLedgerSummary {
            thread_id: default_thread_id,
            latest_seq: state.latest_seq,
            effective_history,
        },
        report: ProviderRecoveryReport {
            default_thread_id,
            recovered_threads,
            imported_thread_id: default_thread_id,
            degraded: state.degraded,
        },
        stats,
        warnings,
    })
}
```

Add helper functions:

```rust
fn apply_record(state: &mut ProviderRecoveryState, record: ProviderLedgerRecord) {
    state.latest_seq = record.seq();
    match record {
        ProviderLedgerRecord::Message {
            thread_id, message, ..
        } => state.histories.entry(thread_id).or_default().push(message),
        ProviderLedgerRecord::Compacted {
            thread_id,
            replacement_history,
            ..
        } => {
            state.histories.insert(thread_id, replacement_history);
        }
        ProviderLedgerRecord::LedgerInvalidated { thread_id, .. } => {
            state.histories.insert(thread_id, Vec::new());
        }
    }
}

fn split_jsonl_lines(bytes: &[u8]) -> Vec<(usize, &[u8], bool)> {
    if bytes.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    let mut index = 0;
    for (pos, byte) in bytes.iter().enumerate() {
        if *byte == b'\n' {
            let end = pos;
            out.push((index, bytes[start..end].strip_suffix(b"\r").unwrap_or(&bytes[start..end]), false));
            start = pos + 1;
            index += 1;
        }
    }
    if start < bytes.len() {
        out.push((index, bytes[start..].strip_suffix(b"\r").unwrap_or(&bytes[start..]), true));
    }
    out
}

fn warn(
    warnings: &mut Vec<RecoveryWarning>,
    stats: &mut RecoverySourceStats,
    line_number: usize,
    code: &str,
    message: &str,
) {
    stats.warnings += 1;
    warnings.push(RecoveryWarning {
        source: RecoverySource::ProviderLedgerJsonl,
        line: Some(line_number),
        record_index: None,
        severity: RecoverySeverity::Warning,
        code: code.to_string(),
        message: message.to_string(),
    });
}
```

- [ ] **Step 3: Run focused test**

Run:

```bash
cargo test -p roci-core --features agent session_recovery -- --nocapture
```

Expected: provider tests compile after Task 4 wires recovery store methods.

## Task 4: LocalSessionStore Recovery Export/Import

**Files:**
- Modify: `crates/roci-core/src/session/store.rs`
- Modify: `crates/roci-core/src/session/recovery/mod.rs`
- Test: `crates/roci-core/src/agent/runtime_tests/session_recovery.rs`

- [ ] **Step 1: Add store-level tests for importability, staging, and strict replay**

Append tests:

```rust
#[tokio::test]
async fn recover_import_rejects_plain_snapshot_and_existing_target() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("sessions");
    let source = SessionId::parse("source-import").unwrap();
    let target = SessionId::parse("source-import-copy").unwrap();
    let store = LocalSessionStore::new(&root);
    store
        .create(crate::session::CreateSessionOptions {
            id: Some(source.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    store
        .create(crate::session::CreateSessionOptions {
            id: Some(target.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let recovered = store
        .recover_export(SessionRecoverySource::SessionId(source))
        .await
        .unwrap();

    assert!(store.recover_import(recovered, target).await.is_err());
}

#[tokio::test]
async fn recover_import_leaves_no_target_on_tampered_artifact() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("sessions");
    let source = SessionId::parse("source-tamper").unwrap();
    let target = SessionId::parse("target-tamper").unwrap();
    let store = LocalSessionStore::new(&root);
    store
        .create(crate::session::CreateSessionOptions {
            id: Some(source.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let mut recovered = store
        .recover_export(SessionRecoverySource::SessionId(source))
        .await
        .unwrap();
    recovered.report.importable_runtime_state = false;

    assert!(store.recover_import(recovered, target.clone()).await.is_err());
    assert!(!root.join(target.as_str()).exists());
}

#[tokio::test]
async fn corrupt_provider_ledger_still_breaks_normal_open() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("sessions");
    let id = SessionId::parse("strict-provider").unwrap();
    let store = LocalSessionStore::new(&root);
    store
        .create(crate::session::CreateSessionOptions {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    let ledger_path = PathConventions::for_session(&root, &id).provider_ledger_file();
    std::fs::write(ledger_path, "not json\n").unwrap();

    assert!(store.open(id).await.is_err());
}

#[tokio::test]
async fn corrupt_events_still_break_normal_open_and_export() {
    let temp = tempfile::tempdir().expect("tempdir");
    let root = temp.path().join("sessions");
    let id = SessionId::parse("strict-events").unwrap();
    let store = LocalSessionStore::new(&root);
    store
        .create(crate::session::CreateSessionOptions {
            id: Some(id.clone()),
            ..Default::default()
        })
        .await
        .unwrap();
    std::fs::write(root.join(id.as_str()).join("events.jsonl"), "not json\n").unwrap();

    assert!(store.open(id.clone()).await.is_err());
    assert!(store.export_snapshot(id).await.is_err());
}
```

- [ ] **Step 2: Add public store methods**

Patch `crates/roci-core/src/session/store.rs` imports:

```rust
use super::recovery::{
    self, RecoveredSession, RecoveryReport, RecoverySource, RecoverySourceReport,
    RecoverySourceStatus, RecoveryStats, RuntimeSnapshotCachePreview, SessionRecoverySource,
    RECOVERED_SESSION_ARTIFACT_TYPE, RECOVERED_SESSION_SCHEMA_VERSION,
};
```

Add methods inside `impl LocalSessionStore`:

```rust
    pub async fn recover_export(
        &self,
        source: SessionRecoverySource,
    ) -> SessionResult<RecoveredSession> {
        recovery::recover_export_from_store(self, source).await
    }

    pub async fn recover_import(
        &self,
        recovered: RecoveredSession,
        target_id: super::SessionId,
    ) -> SessionResult<SessionResumeState> {
        recovery::recover_import_into_store(self, recovered, target_id).await
    }
```

- [ ] **Step 3: Move recovery orchestration into recovery module**

Add to `crates/roci-core/src/session/recovery/mod.rs`:

```rust
use crate::agent::runtime::chat::ChatProjector;
use super::{
    CreateSessionOptions, ImportPolicy, LocalSessionStore, PathConventions, ProviderLedgerSummary,
    RuntimeSnapshotCache, SessionConfig, SessionError, SessionMetadata, SessionResourceManifest,
    SessionResumeState,
};

pub(crate) async fn recover_export_from_store(
    store: &LocalSessionStore,
    source: SessionRecoverySource,
) -> SessionResult<RecoveredSession> {
    let (source_dir, source_id) = resolve_source(store, source)?;
    let conventions = PathConventions::new(source_dir.clone());
    let (metadata, metadata_report, metadata_warnings, metadata_importable) =
        recover_metadata(&source_dir, source_id)?;
    let default_thread_id = metadata
        .as_ref()
        .and_then(|_| read_default_thread_id(&conventions).ok())
        .unwrap_or_default();
    let events_path = conventions.events_file();
    let recovered_events = events::recover_events(&events_path)?;
    let mut warnings = metadata_warnings;
    warnings.extend(recovered_events.warnings.clone());
    let runtime_result = ChatProjector::from_events(
        crate::agent::runtime::chat::ChatRuntimeConfig {
            default_thread_id: Some(default_thread_id),
            ..Default::default()
        },
        recovered_events.events.clone(),
    );
    let (runtime, importable_runtime_state) = match runtime_result {
        Ok(projector) => (projector.read_snapshot(), metadata_importable),
        Err(source) => {
            warnings.push(RecoveryWarning {
                source: RecoverySource::EventsJsonl,
                line: None,
                record_index: None,
                severity: RecoverySeverity::Error,
                code: "events_projection_failed".to_string(),
                message: source.to_string(),
            });
            (
                crate::agent::runtime::chat::RuntimeSnapshot {
                    schema_version: 1,
                    threads: Vec::new(),
                },
                false,
            )
        }
    };
    let provider_path = conventions.provider_ledger_file();
    let provider = provider::recover_provider_ledger(&provider_path, default_thread_id)?;
    warnings.extend(provider.warnings.clone());
    let cache_path = conventions.runtime_snapshot_file();
    let cache_preview = recover_cache_preview(&cache_path);
    let resources = if importable_runtime_state {
        super::build_resource_manifest(&conventions, &runtime)
    } else {
        SessionResourceManifest::default()
    };
    let metadata = metadata.unwrap_or_else(|| {
        SessionMetadata::new(
            SessionId::parse("unimportable-recovery").expect("static session id"),
            None,
            None,
        )
    });
    let snapshot = SessionSnapshot {
        schema_version: 1,
        metadata,
        default_thread_id,
        runtime,
        events: recovered_events.events,
        provider_ledger: provider.summary,
        resources,
        exported_at: Utc::now(),
    };
    let events_status = status_for_stats(&recovered_events.stats);
    let provider_status = status_for_stats(&provider.stats);
    let cache_status = cache_preview.as_ref().map_or(RecoverySourceStatus::Missing, |preview| {
        if preview.parsed {
            RecoverySourceStatus::Read
        } else {
            RecoverySourceStatus::Unusable
        }
    });
    let resource_status = if importable_runtime_state {
        RecoverySourceStatus::Read
    } else {
        RecoverySourceStatus::Unusable
    };
    let report = RecoveryReport {
        importable_runtime_state,
        sources: vec![
            metadata_report,
            RecoverySourceReport {
                source: RecoverySource::EventsJsonl,
                path: events_path,
                status: events_status,
            },
            RecoverySourceReport {
                source: RecoverySource::ProviderLedgerJsonl,
                path: provider_path,
                status: provider_status,
            },
            RecoverySourceReport {
                source: RecoverySource::RuntimeSnapshotCache,
                path: cache_path,
                status: cache_status,
            },
            RecoverySourceReport {
                source: RecoverySource::Resources,
                path: conventions.root().to_path_buf(),
                status: resource_status,
            },
        ],
        warnings,
        stats: RecoveryStats {
            events: recovered_events.stats,
            provider_ledger: provider.stats,
            resources: resource_stats(&resources),
        },
        cache_preview,
        provider_context: provider.report,
        resource_refs_only: true,
    };
    Ok(RecoveredSession {
        artifact_type: RECOVERED_SESSION_ARTIFACT_TYPE.to_string(),
        schema_version: RECOVERED_SESSION_SCHEMA_VERSION,
        snapshot,
        report,
    })
}
```

Implement helper functions in same file:

```rust
fn resolve_source(
    store: &LocalSessionStore,
    source: SessionRecoverySource,
) -> SessionResult<(PathBuf, Option<SessionId>)> {
    match source {
        SessionRecoverySource::SessionId(id) => {
            Ok((PathConventions::for_session(store.root(), &id).root().to_path_buf(), Some(id)))
        }
        SessionRecoverySource::SessionDir { path, source_id } => {
            if !path.is_dir() {
                return Err(SessionError::NotDirectory { path });
            }
            let derived = path
                .file_name()
                .and_then(|value| value.to_str())
                .and_then(|value| SessionId::parse(value).ok());
            Ok((path, source_id.or(derived)))
        }
    }
}

fn recover_metadata(
    source_dir: &std::path::Path,
    source_id: Option<SessionId>,
) -> SessionResult<(
    Option<SessionMetadata>,
    RecoverySourceReport,
    Vec<RecoveryWarning>,
    bool,
)> {
    let path = source_dir.join("metadata.json");
    match SessionMetadata::read_from_path(&path) {
        Ok(metadata) => Ok((
            Some(metadata),
            RecoverySourceReport {
                source: RecoverySource::Metadata,
                path,
                status: RecoverySourceStatus::Read,
            },
            Vec::new(),
            true,
        )),
        Err(err) => {
            let mut warnings = vec![RecoveryWarning {
                source: RecoverySource::Metadata,
                line: None,
                record_index: None,
                severity: RecoverySeverity::Warning,
                code: "metadata_unusable".to_string(),
                message: err.to_string(),
            }];
            if let Some(id) = source_id {
                warnings.push(RecoveryWarning {
                    source: RecoverySource::Metadata,
                    line: None,
                    record_index: None,
                    severity: RecoverySeverity::Warning,
                    code: "metadata_synthesized".to_string(),
                    message: "metadata synthesized from recovery source id".to_string(),
                });
                Ok((
                    Some(SessionMetadata::new(id, None, Some(source_dir.to_path_buf()))),
                    RecoverySourceReport {
                        source: RecoverySource::Metadata,
                        path,
                        status: RecoverySourceStatus::RecoveredWithWarnings,
                    },
                    warnings,
                    true,
                ))
            } else {
                Ok((
                    None,
                    RecoverySourceReport {
                        source: RecoverySource::Metadata,
                        path,
                        status: RecoverySourceStatus::Unusable,
                    },
                    warnings,
                    false,
                ))
            }
        }
    }
}

fn read_default_thread_id(conventions: &PathConventions) -> SessionResult<ThreadId> {
    let bytes = std::fs::read(conventions.runtime_snapshot_file())
        .map_err(|source| SessionError::io(conventions.runtime_snapshot_file(), source))?;
    let cache: RuntimeSnapshotCache =
        serde_json::from_slice(&bytes).map_err(|source| SessionError::InvalidMetadata {
            path: conventions.runtime_snapshot_file(),
            message: source.to_string(),
        })?;
    Ok(cache.default_thread_id)
}

fn recover_cache_preview(path: &std::path::Path) -> Option<RuntimeSnapshotCachePreview> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<RuntimeSnapshotCache>(&bytes) {
        Ok(cache) => Some(RuntimeSnapshotCachePreview {
            parsed: true,
            generated_at: Some(cache.generated_at),
            thread_count: Some(cache.runtime.threads.len()),
            latest_cursors: cache.latest_cursors,
            parse_error: None,
        }),
        Err(source) => Some(RuntimeSnapshotCachePreview {
            parsed: false,
            generated_at: None,
            thread_count: None,
            latest_cursors: Vec::new(),
            parse_error: Some(source.to_string()),
        }),
    }
}

fn status_for_stats(stats: &RecoverySourceStats) -> RecoverySourceStatus {
    if stats.records_read == 0 {
        RecoverySourceStatus::Missing
    } else if stats.records_skipped > 0 || stats.warnings > 0 {
        RecoverySourceStatus::RecoveredWithWarnings
    } else {
        RecoverySourceStatus::Read
    }
}

fn resource_stats(resources: &SessionResourceManifest) -> RecoverySourceStats {
    let records_recovered = resources
        .plan
        .iter()
        .chain(resources.workspace.iter())
        .count()
        + resources.artifacts.len()
        + resources.temp_files.len()
        + resources.checkpoints.len()
        + resources.files.len();
    RecoverySourceStats {
        records_read: records_recovered,
        records_recovered,
        records_skipped: 0,
        warnings: 0,
    }
}
```

- [ ] **Step 4: Add staging import implementation**

Add to `recovery/mod.rs`:

```rust
pub(crate) async fn recover_import_into_store(
    store: &LocalSessionStore,
    recovered: RecoveredSession,
    target_id: SessionId,
) -> SessionResult<SessionResumeState> {
    if recovered.artifact_type != RECOVERED_SESSION_ARTIFACT_TYPE {
        return Err(SessionError::InvalidRecoveredSession {
            message: "missing roci_recovered_session artifact envelope".to_string(),
        });
    }
    if !recovered.report.importable_runtime_state {
        return Err(SessionError::NonImportableRecovery {
            message: "recovery report marked runtime state as not importable".to_string(),
        });
    }
    ChatProjector::from_events(
        crate::agent::runtime::chat::ChatRuntimeConfig {
            default_thread_id: Some(recovered.snapshot.default_thread_id),
            ..Default::default()
        },
        recovered.snapshot.events.clone(),
    )
    .map_err(|source| SessionError::RuntimeProjection {
        path: PathConventions::for_session(store.root(), &target_id).events_file(),
        message: source.to_string(),
    })?;

    let conventions = PathConventions::for_session(store.root(), &target_id);
    if conventions.root().exists() {
        return Err(SessionError::AlreadyExists {
            path: conventions.root().to_path_buf(),
        });
    }

    let staging_root = store
        .root()
        .join(format!(".recovering-{}", target_id.as_str()));
    if staging_root.exists() {
        std::fs::remove_dir_all(&staging_root)
            .map_err(|source| SessionError::io(&staging_root, source))?;
    }
    let staging = PathConventions::new(&staging_root);
    let result =
        write_recovered_snapshot_to_conventions(&staging, recovered.snapshot, target_id.clone())
            .await
            .and_then(|()| {
                std::fs::rename(&staging_root, conventions.root())
                    .map_err(|source| SessionError::io(conventions.root(), source))
            });
    if let Err(err) = result {
        if staging_root.exists() {
            let _ = std::fs::remove_dir_all(&staging_root);
        }
        return Err(err);
    }
    store.open(target_id).await
}
```

Add direct async writer in `recovery/mod.rs` or extract it from `store.rs`:

```rust
async fn write_recovered_snapshot_to_conventions(
    conventions: &PathConventions,
    mut snapshot: SessionSnapshot,
    target_id: SessionId,
) -> SessionResult<()> {
    std::fs::create_dir_all(conventions.root())
        .map_err(|source| SessionError::io(conventions.root(), source))?;
    snapshot.metadata.id = target_id;
    snapshot.metadata.write_to_path(conventions.metadata_file())?;

    let event_store = JsonlAgentRuntimeEventStore::open(conventions.events_file()).map_err(|err| {
        SessionError::RuntimeProjection {
            path: conventions.events_file(),
            message: err.to_string(),
        }
    })?;
    let mut events = snapshot.events;
    let next_seq = events
        .iter()
        .filter(|event| event.thread_id == snapshot.default_thread_id)
        .map(|event| event.seq)
        .max()
        .unwrap_or(0)
        + 1;
    events.extend(super::resource_events_from_manifest(
        snapshot.default_thread_id,
        next_seq,
        &snapshot.resources,
    ));
    events.sort_by_key(|event| (event.thread_id.to_string(), event.seq));
    for event in events {
        event_store
            .append(event)
            .await
            .map_err(|err| SessionError::RuntimeProjection {
                path: conventions.events_file(),
                message: err.to_string(),
            })?;
    }

    let ledger = LocalProviderLedger::open(conventions.provider_ledger_file())?;
    if !snapshot.provider_ledger.effective_history.is_empty() {
        ledger.append_compacted(
            snapshot.provider_ledger.thread_id,
            snapshot.provider_ledger.effective_history.clone(),
        )?;
    }
    let replayed = LocalProviderLedger::open(conventions.provider_ledger_file())?;
    let replayed_state = replayed.state_for_thread(snapshot.default_thread_id);
    if replayed_state.effective_history != snapshot.provider_ledger.effective_history {
        return Err(SessionError::InvalidRecoveredSession {
            message: "provider ledger summary failed strict replay validation".to_string(),
        });
    }
    Ok(())
}
```

- [ ] **Step 5: Expose store helpers safely**

Patch private helper signatures in `store.rs` only if compile requires it:

```rust
pub(crate) fn build_resource_manifest(
    conventions: &PathConventions,
    runtime: &RuntimeSnapshot,
) -> SessionResourceManifest

pub(crate) fn resource_events_from_manifest(
    thread_id: ThreadId,
    start_seq: u64,
    manifest: &SessionResourceManifest,
) -> Vec<AgentRuntimeEvent>
```

Do not change strict replay behavior in `JsonlAgentRuntimeEventStore` or `LocalProviderLedger::open`.

- [ ] **Step 6: Run core recovery tests**

Run:

```bash
cargo test -p roci-core --features agent session_recovery -- --nocapture
```

Expected: all `session_recovery` tests pass.

## Task 5: CLI Recovery Commands

**Files:**
- Modify: `crates/roci-cli/src/cli/mod.rs`
- Modify: `crates/roci-cli/src/session_cmd.rs`

- [ ] **Step 1: Add CLI args**

Patch `crates/roci-cli/src/cli/mod.rs`:

```rust
    /// Export a tolerant recovered session artifact
    RecoverExport(SessionRecoverExportArgs),
    /// Import a tolerant recovered session artifact
    RecoverImport(SessionRecoverImportArgs),
```

Add structs:

```rust
#[derive(Parser, Debug)]
pub struct SessionRecoverExportArgs {
    /// Session root directory. Defaults to the app data session directory.
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Session id to recover. Omit when using --session-dir.
    pub id: Option<String>,

    /// Raw session directory to recover.
    #[arg(long, value_name = "PATH", conflicts_with = "id")]
    pub session_dir: Option<PathBuf>,

    /// Source id to use when --session-dir metadata is corrupt or missing.
    #[arg(long, value_name = "ID", requires = "session_dir")]
    pub source_id: Option<String>,

    /// Output recovered artifact JSON path.
    #[arg(long, value_name = "PATH")]
    pub output: PathBuf,

    /// Print full recovery report as JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Parser, Debug)]
pub struct SessionRecoverImportArgs {
    /// Session root directory. Defaults to the app data session directory.
    #[arg(long, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Input recovered artifact JSON path.
    #[arg(long, value_name = "PATH")]
    pub input: PathBuf,

    /// Imported session id.
    #[arg(long, value_name = "ID")]
    pub id: String,

    /// Print a JSON summary.
    #[arg(long)]
    pub json: bool,
}
```

- [ ] **Step 2: Wire CLI handlers**

Patch `crates/roci-cli/src/session_cmd.rs` imports:

```rust
use roci::session::{
    RecoveredSession, SessionRecoverySource, RECOVERED_SESSION_ARTIFACT_TYPE,
};
```

Patch `handle_session` match:

```rust
        SessionCommands::RecoverExport(args) => {
            let summary = recover_export_session(args).await?;
            print_recover_export_summary(&summary)?;
        }
        SessionCommands::RecoverImport(args) => {
            let summary = recover_import_session(args).await?;
            print_recover_import_summary(&summary)?;
        }
```

Add summary structs and functions:

```rust
#[derive(Debug, Clone)]
struct RecoverExportSummary {
    output: PathBuf,
    importable: bool,
    warnings: usize,
    recovered_events: usize,
    recovered_provider_records: usize,
    json: bool,
    report: roci::session::RecoveryReport,
}

#[derive(Debug, Clone)]
struct RecoverImportSummary {
    id: String,
    root: PathBuf,
    importable: bool,
    warnings: usize,
    recovered_events: usize,
    recovered_provider_records: usize,
    json: bool,
}

async fn recover_export_session(
    args: SessionRecoverExportArgs,
) -> Result<RecoverExportSummary, Box<dyn std::error::Error>> {
    let root = resolve_root(args.root)?;
    let source = if let Some(session_dir) = args.session_dir {
        SessionRecoverySource::SessionDir {
            path: session_dir,
            source_id: args.source_id.map(SessionId::parse).transpose()?,
        }
    } else {
        let id = args.id.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "session id or --session-dir is required")
        })?;
        SessionRecoverySource::SessionId(SessionId::parse(id)?)
    };
    let store = LocalSessionStore::new(root);
    let recovered = store.recover_export(source).await?;
    write_pretty_json_any(&args.output, &recovered)?;
    Ok(RecoverExportSummary {
        output: args.output,
        importable: recovered.report.importable_runtime_state,
        warnings: recovered.report.warnings.len(),
        recovered_events: recovered.report.stats.events.records_recovered,
        recovered_provider_records: recovered.report.stats.provider_ledger.records_recovered,
        json: args.json,
        report: recovered.report,
    })
}

async fn recover_import_session(
    args: SessionRecoverImportArgs,
) -> Result<RecoverImportSummary, Box<dyn std::error::Error>> {
    let root = resolve_root(args.root)?;
    let bytes = fs::read(&args.input)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    if value.get("artifact_type").and_then(serde_json::Value::as_str)
        != Some(RECOVERED_SESSION_ARTIFACT_TYPE)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "input is not a roci recovered session artifact; use `session import` for plain SessionSnapshot files",
        )
        .into());
    }
    let recovered: RecoveredSession = serde_json::from_value(value)?;
    let importable = recovered.report.importable_runtime_state;
    let warnings = recovered.report.warnings.len();
    let recovered_events = recovered.report.stats.events.records_recovered;
    let recovered_provider_records = recovered.report.stats.provider_ledger.records_recovered;
    let target_id = SessionId::parse(args.id)?;
    let store = LocalSessionStore::new(root.clone());
    let state = store.recover_import(recovered, target_id).await?;
    Ok(RecoverImportSummary {
        id: state.metadata.id.to_string(),
        root,
        importable,
        warnings,
        recovered_events,
        recovered_provider_records,
        json: args.json,
    })
}
```

Add printers and generic writer:

```rust
fn print_recover_export_summary(
    summary: &RecoverExportSummary,
) -> Result<(), Box<dyn std::error::Error>> {
    if summary.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "output": summary.output,
                "importable": summary.importable,
                "warnings": summary.warnings,
                "recovered_events": summary.recovered_events,
                "recovered_provider_records": summary.recovered_provider_records,
                "report": summary.report,
            }))?
        );
    } else {
        println!("Recovered session artifact: {}", summary.output.display());
        println!("Importable: {}", summary.importable);
        println!("Warnings: {}", summary.warnings);
    }
    Ok(())
}

fn print_recover_import_summary(
    summary: &RecoverImportSummary,
) -> Result<(), Box<dyn std::error::Error>> {
    if summary.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "id": summary.id,
                "root": summary.root,
                "importable": summary.importable,
                "warnings": summary.warnings,
                "recovered_events": summary.recovered_events,
                "recovered_provider_records": summary.recovered_provider_records,
            }))?
        );
    } else {
        println!("Imported recovered session {}", summary.id);
        println!("Root: {}", summary.root.display());
    }
    Ok(())
}
```

```rust
fn write_pretty_json_any<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}
```

- [ ] **Step 3: Add CLI tests**

Append tests in `session_cmd.rs`:

```rust
#[tokio::test]
async fn recover_export_and_import_session_round_trip() {
    let root = tempdir().unwrap();
    let output = root.path().join("recovered.json");
    create_session(SessionCreateArgs {
        root: root_args(root.path()),
        id: Some("recover-cli".to_string()),
        title: Some("Recover CLI".to_string()),
        json: false,
    })
    .await
    .unwrap();
    fs::write(root.path().join("recover-cli").join("events.jsonl"), "not json\n").unwrap();

    let summary = recover_export_session(SessionRecoverExportArgs {
        root: root_args(root.path()),
        id: Some("recover-cli".to_string()),
        session_dir: None,
        source_id: None,
        output: output.clone(),
        json: true,
    })
    .await
    .unwrap();

    assert!(summary.warnings > 0);
    assert!(output.is_file());

    let imported = recover_import_session(SessionRecoverImportArgs {
        root: root_args(root.path()),
        input: output,
        id: "recover-cli-imported".to_string(),
        json: true,
    })
    .await
    .unwrap();
    assert_eq!(imported.id, "recover-cli-imported");
    assert!(root.path().join("recover-cli-imported").is_dir());
}

#[tokio::test]
async fn recover_import_plain_snapshot_reports_session_import_hint() {
    let root = tempdir().unwrap();
    let input = root.path().join("snapshot.json");
    create_session(SessionCreateArgs {
        root: root_args(root.path()),
        id: Some("plain-source".to_string()),
        title: None,
        json: false,
    })
    .await
    .unwrap();
    export_session(SessionExportArgs {
        root: root_args(root.path()),
        id: "plain-source".to_string(),
        output: input.clone(),
        json: false,
    })
    .await
    .unwrap();

    let err = recover_import_session(SessionRecoverImportArgs {
        root: root_args(root.path()),
        input,
        id: "plain-target".to_string(),
        json: false,
    })
    .await
    .unwrap_err();

    assert!(err.to_string().contains("session import"));
}
```

- [ ] **Step 4: Run CLI tests**

Run:

```bash
cargo test -p roci-cli session_cmd -- --nocapture
```

Expected: CLI session tests pass.

## Task 6: Docs And Verification Gates

**Files:**
- Modify: `docs/testing.md`

- [ ] **Step 1: Add durable recovery smoke docs**

Patch `docs/testing.md` under Durable session verification:

```bash
Recovery smoke:

```bash
ROOT=/tmp/roci-session-recovery-smoke
rm -rf "$ROOT"
cargo run -q -p roci-cli -- session create --root "$ROOT" --id recover-smoke --title Recover --json
printf 'not json\n' >> "$ROOT/recover-smoke/events.jsonl"
cargo run -q -p roci-cli -- session recover-export recover-smoke --root "$ROOT" --output "$ROOT/recovered.json" --json
rg -n '"artifact_type": "roci_recovered_session"|"warnings"' "$ROOT/recovered.json"
cargo run -q -p roci-cli -- session recover-import --root "$ROOT" --input "$ROOT/recovered.json" --id recover-smoke-import --json
```

Provider resume recovery smoke:

```bash
tmux new-session -d -s roci-session-recovery-live \
  'cd /path/to/roci && \
   ROOT=/tmp/roci-session-recovery-live && \
   OPENAI_API_KEY=sk-local-dummy \
   OPENAI_BASE_URL=http://framed:4001/v1 \
   cargo run -q -p roci-cli -- \
   chat --no-skills --model "openai:gemma-4-e4b" \
   --temperature 0 --max-tokens 48 \
   --session-root "$ROOT" --session-id recovery-live \
   "Reply exactly: roci recovery seed ok"; \
   seed_status=$?; printf "\n[seed exit=%s]\n" "$seed_status"; \
   cargo run -q -p roci-cli -- session recover-export recovery-live --root "$ROOT" --output "$ROOT/recovered.json" --json; \
   export_status=$?; printf "\n[recover-export exit=%s]\n" "$export_status"; \
   cargo run -q -p roci-cli -- session recover-import --root "$ROOT" --input "$ROOT/recovered.json" --id recovery-live-import --json; \
   import_status=$?; printf "\n[recover-import exit=%s]\n" "$import_status"; \
   cargo run -q -p roci-cli -- chat --no-skills --model "openai:gemma-4-e4b" \
     --temperature 0 --max-tokens 48 \
     --session-root "$ROOT" --session-id recovery-live-import \
     "Reply exactly: roci recovery resume ok"; \
   resume_status=$?; printf "\n[resume exit=%s]\n" "$resume_status"; \
   exec zsh'
echo "attach: tmux attach -t roci-session-recovery-live"
```
```

- [ ] **Step 2: Run formatting and focused tests**

Run:

```bash
cargo fmt --all
cargo test -p roci-core --features agent session_recovery -- --nocapture
cargo test -p roci-cli session_cmd -- --nocapture
```

Expected: all pass.

- [ ] **Step 3: Run full hermetic gate**

Run:

```bash
cargo test
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

Expected: all pass. If `--all-features` conflicts with repo feature matrix, run the narrow documented replacement and record exact reason.

- [ ] **Step 4: Run CLI durable recovery smoke**

Run:

```bash
ROOT=/tmp/roci-session-recovery-smoke
rm -rf "$ROOT"
cargo run -q -p roci-cli -- session create --root "$ROOT" --id recover-smoke --title Recover --json
printf 'not json\n' >> "$ROOT/recover-smoke/events.jsonl"
cargo run -q -p roci-cli -- session recover-export recover-smoke --root "$ROOT" --output "$ROOT/recovered.json" --json
rg -n '"artifact_type": "roci_recovered_session"|"warnings"' "$ROOT/recovered.json"
```

Expected: `recover-export` exits 0 and `recovered.json` contains artifact envelope and warnings. Import may reject if runtime projection is non-importable; that rejection is acceptable for the corrupt fixture.

- [ ] **Step 5: Run live provider recovery/resume smoke in tmux**

Run:

```bash
tmux new-session -d -s roci-session-recovery-live \
  'cd /Users/adityasharma/Projects/roci && \
   ROOT=/tmp/roci-session-recovery-live && \
   rm -rf "$ROOT" && \
   OPENAI_API_KEY=sk-local-dummy \
   OPENAI_BASE_URL=http://framed:4001/v1 \
   cargo run -q -p roci-cli -- \
   chat --no-skills --model "openai:gemma-4-e4b" \
   --temperature 0 --max-tokens 48 \
   --session-root "$ROOT" --session-id recovery-live \
   "Reply exactly: roci recovery seed ok"; \
   seed_status=$?; printf "\n[seed exit=%s]\n" "$seed_status"; \
   cargo run -q -p roci-cli -- session recover-export recovery-live --root "$ROOT" --output "$ROOT/recovered.json" --json; \
   export_status=$?; printf "\n[recover-export exit=%s]\n" "$export_status"; \
   cargo run -q -p roci-cli -- session recover-import --root "$ROOT" --input "$ROOT/recovered.json" --id recovery-live-import --json; \
   import_status=$?; printf "\n[recover-import exit=%s]\n" "$import_status"; \
   cargo run -q -p roci-cli -- chat --no-skills --model "openai:gemma-4-e4b" \
     --temperature 0 --max-tokens 48 \
     --session-root "$ROOT" --session-id recovery-live-import \
     "Reply exactly: roci recovery resume ok"; \
   resume_status=$?; printf "\n[resume exit=%s]\n" "$resume_status"; \
   ls -la "$ROOT/recovery-live-import"; \
   exec zsh'
echo "attach: tmux attach -t roci-session-recovery-live"
```

Expected: seed, recover-export, recover-import, and resume exit 0; imported session dir exists; provider response includes requested marker.

## Self-Review Checklist

- Spec coverage: plan covers recovery API/data model, tolerant `events.jsonl`, tolerant provider ledger, metadata fallback, refs-only resources, cache preview, staging import, CLI commands, strict replay regression, CLI smoke, live provider smoke.
- No normal replay behavior changes allowed: strict store and strict ledger remain unchanged.
- Main implementation risk: direct staging writer duplicates part of `import_snapshot`. If duplication grows, extract a private `write_snapshot_to_conventions` helper in `store.rs` and let both `import_snapshot` and `recover_import` call it.
- Follow-up intentionally not included: runtime `history.jsonl` dual-write and multi-thread provider import.
