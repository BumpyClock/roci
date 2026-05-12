# Tolerant Session Recovery Design

## Context

Roci already has durable session history in `roci-core`:

- strict semantic runtime events in `events.jsonl`
- strict provider context records in the provider ledger file
  (`PathConventions::provider_ledger_file()`, currently `model_messages.jsonl`)
- derived runtime and ledger snapshot caches
- session create/open/export/import through `LocalSessionStore`
- CLI session commands for create/list/export/import/delete

Those strict artifacts are correct for normal replay. A corrupt committed line in
`events.jsonl` must still fail visible replay. This design adds a separate
recovery pathway for damaged sessions. It does not add normal runtime dual-write
to `history.jsonl`; that remains optional future work if product history needs a
separate source later.

## Goals

- Recover usable session data from partially corrupt durable session artifacts.
- Preserve strict `events.jsonl` replay semantics for normal open/resume.
- Export a recovery artifact that carries both recovered state and diagnostics.
- Import only when recovered runtime state can be rebuilt from canonical events.
- Provide CLI commands so recovery can be verified and used end to end.

## Non-goals

- No tolerant normal replay for `AgentRuntimeEventStore`.
- No generic semantic event filtering to force projection success.
- No snapshot-cache-based runtime import.
- No destructive repair of an existing session directory.
- No `history.jsonl` runtime dual-write in v1.

## Prior Art Decisions

Pi, Codex, and Claude Code all tolerate malformed history lines, but keep a
canonical history/transcript as authority. Derived cache/state is useful for
listing, reporting, or UI state, not authoritative runtime resume.

Roci follows that pattern:

- accepted `events.jsonl` records must project cleanly to become importable
  runtime state
- `runtime.snapshot.json` is report/display only when event projection fails
- provider ledger recovery replays typed valid records, anchored by compacted
  effective-history checkpoints

## Public API

Add `session::recovery` with these public types:

```rust
pub const RECOVERED_SESSION_ARTIFACT_TYPE: &str = "roci_recovered_session";

pub struct RecoveredSession {
    pub artifact_type: String,
    pub schema_version: u16,
    pub snapshot: SessionSnapshot,
    pub report: RecoveryReport,
}

pub struct RecoveryReport {
    pub importable_runtime_state: bool,
    pub sources: Vec<RecoverySourceReport>,
    pub warnings: Vec<RecoveryWarning>,
    pub stats: RecoveryStats,
    pub cache_preview: Option<RuntimeSnapshotCachePreview>,
    pub provider_context: ProviderRecoveryReport,
    pub resource_refs_only: bool,
}

pub struct RecoverySourceReport {
    pub source: RecoverySource,
    pub path: PathBuf,
    pub status: RecoverySourceStatus,
}

#[serde(rename_all = "snake_case")]
pub enum RecoverySource {
    Metadata,
    EventsJsonl,
    ProviderLedgerJsonl,
    RuntimeSnapshotCache,
    Resources,
}

#[serde(rename_all = "snake_case")]
pub enum RecoverySourceStatus {
    Missing,
    Read,
    RecoveredWithWarnings,
    Unusable,
}

#[serde(rename_all = "snake_case")]
pub enum RecoverySeverity {
    Info,
    Warning,
    Error,
}

pub struct RecoveryWarning {
    pub source: RecoverySource,
    pub line: Option<usize>,
    pub record_index: Option<usize>,
    pub severity: RecoverySeverity,
    pub code: String,
    pub message: String,
}

pub struct RecoveryStats {
    pub events: RecoverySourceStats,
    pub provider_ledger: RecoverySourceStats,
    pub resources: RecoverySourceStats,
}

pub struct RecoverySourceStats {
    pub records_read: usize,
    pub records_recovered: usize,
    pub records_skipped: usize,
    pub warnings: usize,
}

pub struct RuntimeSnapshotCachePreview {
    pub parsed: bool,
    pub generated_at: Option<DateTime<Utc>>,
    pub thread_count: Option<usize>,
    pub latest_cursors: Vec<RuntimeCursor>,
    pub parse_error: Option<String>,
}

pub struct ProviderRecoveryReport {
    pub default_thread_id: ThreadId,
    pub recovered_threads: Vec<ThreadId>,
    pub imported_thread_id: ThreadId,
    pub degraded: bool,
}

pub enum SessionRecoverySource {
    SessionId(SessionId),
    SessionDir {
        path: PathBuf,
        source_id: Option<SessionId>,
    },
}
```

`RecoveryStats` records read, recovered, skipped, and warning counts per source.
`RuntimeSnapshotCachePreview` records parse status, `generated_at`, thread count,
latest cursors, and a warning that the cache is not import-authoritative.

`snapshot` is import-authoritative only when
`report.importable_runtime_state=true`. When event projection fails, recovery
still writes a diagnostic artifact with recovered source counts and warnings, but
`recover_import` must reject it. The `snapshot.runtime` value in that case is
diagnostic-only and must not be used to resume a session.

Add `LocalSessionStore` methods:

```rust
pub async fn recover_export(
    &self,
    source: SessionRecoverySource,
) -> SessionResult<RecoveredSession>;

pub async fn recover_import(
    &self,
    recovered: RecoveredSession,
    target_id: SessionId,
) -> SessionResult<SessionResumeState>;
```

`SessionRecoverySource` supports a normal session id under the store root and a
raw session directory path. For raw paths, recovery derives the source id from
the directory name when it is a valid `SessionId`; callers can override with
`source_id`. If neither path basename nor override is valid and metadata is
missing/corrupt, export emits a non-importable diagnostic artifact.

## Recovery Export Flow

1. Resolve the source session directory.
2. Read `metadata.json`.
   - If valid, use it.
   - If corrupt/missing and the caller supplied an id, synthesize minimal
     metadata from that id and source root, and emit a warning.
3. Tolerantly scan `events.jsonl`.
   - Read bytes line-by-line so invalid UTF-8 can be isolated to one line.
   - Skip blank lines.
   - Accept a final complete JSON line without a trailing newline, but warn with
     code `events_final_line_missing_newline`.
   - Skip invalid UTF-8 or malformed JSON records and emit line-numbered
     warnings.
   - Parse the existing strict record envelope variants: `event`, `event_batch`,
     and `thread_invalidated`.
   - `event` applies one event if its per-thread sequence advances.
   - `event_batch` salvages per event. A bad event stops later recovery only for
     that event's thread; valid events for other threads in the batch can still
     be accepted.
   - `thread_invalidated` clears recovered events for that thread and advances
     that thread cursor to `latest_seq`; later events must have greater seq.
   - When a record has a non-increasing per-thread sequence, reject that record,
     warn, and stop accepting later records for that thread in v1.
4. Project accepted events through `ChatProjector`.
   - If projection succeeds, include the projected runtime snapshot and mark
     `importable_runtime_state=true`.
   - If projection fails, keep recovered event diagnostics, set
     `importable_runtime_state=false`, and record the projection error.
5. Tolerantly scan the provider ledger file (`model_messages.jsonl`).
   - Read bytes line-by-line with the same final-newline and UTF-8 behavior as
     `events.jsonl`.
   - Skip malformed records with warnings.
   - Enforce increasing global seq for accepted records because the current
     provider ledger uses one global sequence.
   - Apply valid `Message`, `Compacted`, and `LedgerInvalidated` records to
     reconstruct per-thread effective provider histories.
   - `Compacted` is an authority checkpoint; later valid messages append.
   - When global ordering becomes ambiguous, warn, mark provider context
     degraded, and stop accepting later provider ledger records in v1.
   - `SessionSnapshot.provider_ledger` can carry only one `thread_id` and one
     effective history today, so v1 imports the recovered default thread's
     provider context. The report lists any additional recovered provider
     threads and marks them not imported. Multi-thread provider import is a
     follow-up if needed.
6. For importable recoveries, read resources with the existing export manifest
   logic from projected runtime refs and filesystem resources under the session
   root. Missing resource files remain unavailable refs. For non-importable
   recoveries, include resource diagnostics in the report only; they do not make
   the artifact importable. Recovery artifacts are refs-only JSON, same as
   `SessionSnapshot`; they do not embed or copy resource bytes. Import preserves
   refs and availability metadata but does not transfer source files across roots.
7. Parse `runtime.snapshot.json` only for `cache_preview`. It never makes a
   failed event projection importable.
8. Build `RecoveredSession { snapshot, report }`.

## Recovery Import Flow

`recover-import` accepts only `RecoveredSession`. If given a plain
`SessionSnapshot`, it errors and points users to existing `session import`.

Import rules:

- `report.importable_runtime_state` must be true.
- `artifact_type` must equal `roci_recovered_session`; if it is missing, CLI
  reports that plain `SessionSnapshot` files must use `session import`.
- target id must not already exist.
- before writing target files, re-project `snapshot.events` through
  `ChatProjector` and rebuild the provider summary to reject tampered artifacts.
- write into a staging directory under the session root, then atomically rename
  to the target session directory; on failure, remove the staging directory.
- write clean strict `events.jsonl` from recovered accepted events.
- write provider ledger as one compacted record containing recovered default
  thread effective history.
- write metadata for the target id.
- preserve unavailable resource refs.
- regenerate runtime and ledger snapshot caches with internal load using the
  target lease, avoiding a second open while the new session is still leased.

## CLI

Add:

```bash
roci-agent session recover-export <id> --root <root> --output recovered.json
roci-agent session recover-export --session-dir /path/to/session --output recovered.json
roci-agent session recover-export --session-dir /path/to/session --source-id old-id --output recovered.json
roci-agent session recover-import --input recovered.json --root <root> --id recovered-id
```

`--json` output for `recover-export` includes the full recovery report, including
warnings and stats. Text output is concise and points to the output file.

`recover-import --json` reports target id, root, importability, warning count,
and recovered counts.

## Error Handling

- Normal `LocalSessionStore::open` stays strict.
- Recovery warnings are structured and machine-testable.
- Projection failure does not lose recovered diagnostics.
- Import rejects non-importable artifacts.
- Import never replaces an existing target.
- Import does not leave a partial target session directory when validation or
  write fails.

## Testing

Unit tests:

- corrupt `events.jsonl` line is skipped, later valid seq accepted
- `event_batch` salvages valid events and stops only the broken thread
- `thread_invalidated` clears stale events and prevents resurrection
- non-increasing event seq emits warning and excludes ambiguous tail
- final complete JSON line without newline is accepted with warning
- invalid UTF-8 line is skipped with warning
- projection failure marks artifact not importable
- corrupt provider ledger line is skipped
- compacted provider ledger checkpoint plus valid suffix reconstructs effective
  history
- provider ledger global non-increasing seq marks provider context degraded and
  stops ledger tail
- additional provider threads are reported but not imported into
  `SessionSnapshot.provider_ledger`
- corrupt metadata synthesizes metadata from source id
- raw session dir with corrupt metadata and no derivable id is non-importable
- valid snapshot cache does not make failed projection importable
- recover-import rejects plain `SessionSnapshot`
- recover-import fails when target exists
- recover-import validates artifact before writing and leaves no target on
  tampered input
- normal `LocalSessionStore::open` and `export_snapshot` still fail on corrupt
  `events.jsonl`
- normal `LocalSessionStore::open` still fails on corrupt provider ledger file

CLI smoke:

- create a session fixture with corrupt event and ledger lines
- run `session recover-export`
- assert JSON report includes warnings/stats
- run `session recover-import`
- assert fresh session has clean strict `events.jsonl` and compacted provider
  ledger

Provider resume smoke:

- recover-import a provider-backed session
- resume chat using the recovered session root/id
- verify provider receives recovered context and exits successfully
- run in tmux per `docs/testing.md` and show
  `tmux attach -t roci-session-recovery-live`

## Follow-ups

- Optional `history.jsonl` product-history dual-write if strict runtime artifacts
  prove insufficient as recovery source.
- Optional explicit snapshot-based salvage command that writes new canonical
  events, not direct cache import.
- Optional ledger-only import mode for non-runtime-importable artifacts.
