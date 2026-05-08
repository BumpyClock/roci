# Session Resume API Design

## Overview

Implement `tsq-r0c1ses7.3`: durable session snapshot and async resume APIs.

This builds on the durable session surface shipped in `tsq-r0c1ses7.2`, `.4`,
and `.5`. Runtime semantic events already persist to strict `events.jsonl`;
session files/resources already live under the host-selected session root. This
slice adds the missing lifecycle layer for creating, opening, exporting,
importing, and resuming sessions without placing session data in project cwd.

## Goals

- Add async session lifecycle APIs around local durable session dirs.
- Add a manifest-style `SessionSnapshot` DTO for state export/import and
  inspection.
- Add a local `SessionResumeState` DTO that can seed `AgentRuntime`.
- Persist provider context in an explicit append-only `model_messages.jsonl`
  ledger, separate from semantic runtime events.
- Support provider ledger compaction via appended `compacted` records.
- Preserve metadata identity across resume; no constructor-side metadata reset.
- Keep `AgentRuntime::new`/`try_new` as prepared-state construction, not session
  file lifecycle or session restore.
- Keep tolerant repair/history salvage out of this slice.

## Non-Goals

- Do not merge two existing sessions.
- Do not inline resource file bytes in `SessionSnapshot`.
- Do not make `model_messages.snapshot.json` canonical.
- Do not make runtime event replay tolerant.
- Do not implement CLI session commands in this slice.
- Do not resurrect in-flight provider execution on resume.

## Prior Art Decisions

Pi uses one append-only session JSONL as durable transcript and derives provider
context from that transcript. Codex stores provider ledger items and UI/runtime
events as separate typed records in one append-only rollout log, with compaction
records that replace effective history without rewriting old history.

Roci keeps its existing stronger split:

- `events.jsonl` is the canonical semantic runtime/UI/resource event log.
- `model_messages.jsonl` is the canonical provider context ledger.
- `SessionSnapshot` is a manifest that references resources, not a blob archive.

This avoids forcing semantic UI events to become provider protocol, and avoids
forcing provider messages to become the UI/runtime event contract.

## Session Layout

Session dir:

```text
<session_root>/<session_id>/
  metadata.json
  events.jsonl
  runtime.snapshot.json
  model_messages.jsonl
  model_messages.snapshot.json
  files/
  artifacts/
  tmp/
  checkpoints/
  plan.md
  workspace.yaml
```

`runtime.snapshot.json` is cache materialized from `events.jsonl`.
`model_messages.snapshot.json` is cache materialized from `model_messages.jsonl`.
Both cache files can be ignored or regenerated at any time.

## Data Model

### Session Options And Policy Types

```rust
pub struct CreateSessionOptions {
    pub id: Option<SessionId>,
    pub title: Option<String>,
    pub host_cwd: Option<PathBuf>,
    pub import_source: Option<PathBuf>,
    pub default_thread_id: Option<ThreadId>,
}

pub enum ImportPolicy {
    FailIfExists,
    NewId(Option<SessionId>),
}
```

`FailIfExists` uses the snapshot metadata id. `NewId(None)` generates a new id.
`NewId(Some(id))` imports into that id if absent. Destructive replace is deferred
until CLI/user-confirmation policy exists.

### SessionSnapshot

`SessionSnapshot` is a portable state manifest:

- schema version
- `SessionMetadata`
- default thread id
- `RuntimeSnapshot`
- semantic runtime event records from `events.jsonl`
- provider ledger summary for the default thread
- resource manifest for plan/workspace/files/artifacts/temp/checkpoints
- export metadata

`SessionSnapshot` does not inline resource bytes. It includes relative resource
paths and sizes. Importing a snapshot recreates state metadata and resource
references, not resource payload files. It does include semantic event records
so import can recreate canonical `events.jsonl` instead of relying on
`runtime.snapshot.json` cache. A future archive API can copy referenced payloads
into a portable bundle.

Rust-ish DTO shape:

```rust
pub struct SessionSnapshot {
    pub schema_version: u16,
    pub metadata: SessionMetadata,
    pub default_thread_id: ThreadId,
    pub runtime: RuntimeSnapshot,
    pub events: Vec<AgentRuntimeEvent>,
    pub provider_ledger: ProviderLedgerSummary,
    pub resources: SessionResourceManifest,
    pub exported_at: DateTime<Utc>,
}

pub struct ProviderLedgerSummary {
    pub thread_id: ThreadId,
    pub latest_seq: u64,
    pub effective_history: Vec<ModelMessage>,
}

pub struct SessionResourceManifest {
    pub plan: Option<SessionResourceRef>,
    pub workspace: Option<SessionResourceRef>,
    pub artifacts: Vec<SessionResourceRef>,
    pub temp_files: Vec<SessionResourceRef>,
    pub checkpoints: Vec<SessionResourceRef>,
    pub files: Vec<SessionResourceRef>,
}

pub struct SessionResourceRef {
    pub namespace: SessionResourceNamespace,
    pub logical_path: Option<LogicalPath>,
    pub storage_path: PathBuf,
    pub len: u64,
    pub updated_at: Option<DateTime<Utc>>,
    pub available: bool,
}
```

Manifest-only import keeps unavailable resource references in import/export
metadata, but omits them from resumed `ThreadSnapshot.resources`. Runtime
resource projections include only payload files that exist under the new local
session root. This keeps the UI from advertising readable artifacts that are not
present locally.

### SessionResumeState

`SessionResumeState` is local, resolved state:

- `SessionConfig`
- `SessionMetadata`
- default thread id
- `RuntimeSnapshot`
- provider ledger as `Vec<ModelMessage>`
- resource manifest
- replayed semantic events
- latest event cursor per thread
- latest provider ledger seq
- held session lease

It is not intended as stable serialized API. Hosts get it from `LocalSessionStore`
and pass it into runtime resume APIs.

### Runtime Snapshot Cache

`events.jsonl` is canonical for semantic runtime state. `runtime.snapshot.json`
is an optional cache containing:

- schema version
- `RuntimeSnapshot`
- latest event cursor per thread
- generated timestamp

`LocalSessionStore::open` can use `runtime.snapshot.json` only when its latest
thread cursors match the latest seqs in `events.jsonl`. If the cache is missing,
corrupt, or stale, open replays `events.jsonl` through the chat projector and
rewrites the cache atomically. The implementation path for `.3` may start with
always replaying events and writing the cache after successful replay.

### Provider Ledger Records

`model_messages.jsonl` records are append-only:

```json
{"type":"message","schema_version":1,"seq":1,"thread_id":"...","message":{...}}
{"type":"compacted","schema_version":1,"seq":42,"thread_id":"...","replacement_history":[...],"replaces_through_seq":41}
{"type":"ledger_invalidated","schema_version":1,"seq":43,"thread_id":"...","latest_seq":42}
```

Rules:

- `seq` is monotonic for the ledger file.
- `thread_id` links provider context to the semantic runtime thread. This slice
  resumes the default thread ledger; non-default thread ledgers are preserved in
  the log for future multi-thread resume work.
- `message` appends one committed `ModelMessage`.
- `compacted` supplies effective provider history through `seq`.
- Resume uses the newest valid `compacted.replacement_history` as base, then
  applies later `message` records.
- `ledger_invalidated` exists for future rewrite/import invalidation semantics.
- Malformed committed nonblank lines fail strict replay with path and line.
- Blank lines are ignored.
- Final nonblank line without newline is rejected, matching strict
  `events.jsonl` behavior.

## API Shape

### LocalSessionStore

```rust
pub struct LocalSessionStore {
    root: PathBuf,
}

impl LocalSessionStore {
    pub async fn create(
        &self,
        options: CreateSessionOptions,
    ) -> Result<SessionResumeState, SessionError>;

    pub async fn open(
        &self,
        id: SessionId,
    ) -> Result<SessionResumeState, SessionError>;

    pub async fn export_snapshot(
        &self,
        id: SessionId,
    ) -> Result<SessionSnapshot, SessionError>;

    pub async fn import_snapshot(
        &self,
        snapshot: SessionSnapshot,
        policy: ImportPolicy,
    ) -> Result<SessionResumeState, SessionError>;
}
```

`create` creates a new session root, metadata, resource dirs, empty event file,
empty runtime snapshot cache, empty provider ledger, and empty provider ledger
snapshot cache. `open` reads existing metadata, strict semantic events, resource
manifests, and provider ledger. `open` ignores corrupt snapshot caches when the
canonical logs replay successfully.

`LocalSessionStore` is the single writer for session lifecycle files. `create`
and `open` acquire a `SessionLease`; `SessionResumeState` carries that lease and
`AgentRuntime::resume_session` holds it until runtime drop. Concurrent opens for
the same session id are rejected with a lock error in this slice. The first
implementation may use an in-process lease registry; cross-process advisory
locking can be layered behind the same `SessionLease` type later.

All session file create/open belongs to `LocalSessionStore`. `AgentRuntime::new`
and `AgentRuntime::try_new` never create metadata, resource dirs,
`events.jsonl`, or `model_messages.jsonl`; they only consume prepared config and
handles from `LocalSessionStore`/`SessionResumeState`.

### Runtime Resume

```rust
impl AgentRuntime {
    pub async fn resume_session(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        config: AgentConfig,
        state: SessionResumeState,
    ) -> Result<Self, RociError>;
}
```

Resume seeds:

- `AgentConfig.session` from `SessionResumeState.session_config`
- chat projector from `RuntimeSnapshot`
- default-thread provider ledger from `SessionResumeState.model_messages`
- runtime event store from existing strict `events.jsonl`
- resource handles from existing session conventions

The `RuntimeSnapshot` in `SessionResumeState` is produced by
`LocalSessionStore::open` from canonical `events.jsonl`, using
`runtime.snapshot.json` only as a validated cache.

`AgentRuntime::new` remains a convenience constructor for non-session or
already-prepared session construction. `AgentRuntime::try_new` must not create
session files, overwrite metadata, or pretend to resume state.

Resume mismatch rules:

- `state.session_config.id` must equal `state.metadata.id`.
- If `config.session` is present, its id/root/cwd must match
  `state.session_config`.
- If `config.chat.default_thread_id` is present, it must match
  `state.default_thread_id`.
- Provider ledger records for non-default threads are preserved on disk but not
  loaded into `AgentRuntime` in this slice.
- Mismatches return `RociError::InvalidState`.

Provider ledger write lifecycle:

- Successful provider turns append the committed provider ledger messages after
  the provider result is accepted and before the turn is marked completed.
- Pre-start canceled turns do not append user input to the provider ledger.
- Failed provider turns append only messages that were already accepted into the
  in-memory provider ledger.
- `replace_messages` writes a `compacted` record whose `replacement_history`
  equals the replacement messages.
- `import_thread` writes a `compacted` record whose `replacement_history` equals
  `ImportedThread::model_messages`.
- Runtime reset writes `ledger_invalidated` for the default thread.
- Existing compaction code writes `compacted` records when it replaces provider
  history.

## Metadata Semantics

Create writes `metadata.json` once:

- `id`
- `title`
- `created_at`
- `updated_at`
- `last_activity_at`
- `host_cwd`
- `import_source`

`last_activity_at` is added with serde default support. Metadata files written
before this field existed read successfully with `last_activity_at = updated_at`.

Resume preserves `created_at`, `host_cwd`, and `import_source`.

`updated_at` changes only through explicit metadata mutation. Runtime activity
uses `last_activity_at` or event/resource timestamps. Opening a session must not
reset creation metadata.

## Resume Normalization

Resume never resurrects in-flight provider execution. If `LocalSessionStore::open`
replays semantic state with queued/running turns, streaming messages, active
tools, pending approvals, or pending human interactions, it appends semantic
cancel events to `events.jsonl` before returning `SessionResumeState`.

After normalization:

- runtime starts idle
- queued/running turns become canceled
- streaming messages become completed or canceled according to existing projector
  invariants
- active tools become completed with an interruption/error result when a final
  result is required by the snapshot type
- pending approvals and human interactions become canceled
- `runtime.snapshot.json` is rewritten from the normalized projection

This makes crash recovery durable and prevents later reopen from seeing the same
stale active state.

## Import Semantics

Default import mode creates a new session from manifest state and exported
semantic event records. Existing target session ids fail. Import writes
`events.jsonl` from `SessionSnapshot.events`, writes provider effective history
as a compacted provider-ledger record, then returns state produced by the same
canonical replay path as `open(target_id)` while keeping the import lease. If the
snapshot carries resource manifest refs
that are absent from the event list, import writes semantic resource records for
those refs with unavailable payloads so export-after-import preserves the
manifest. Runtime projections still omit missing payloads from
`ThreadSnapshot.resources`.

Supported import policies in this slice:

- `FailIfExists`
- `NewId`

No merge or destructive replace mode exists in this slice. Merge requires
explicit conflict semantics for event seqs, provider ledger seqs, resource paths,
metadata, and default thread identity. Replace requires host/CLI confirmation
policy and trash/backup behavior.

## Snapshot Export Semantics

`export_snapshot` exports a manifest only:

- metadata
- runtime snapshot
- semantic event records
- provider ledger summary and optional effective history
- resource entries with namespace, logical path, length, timestamps, and relative
  storage paths

It does not copy payload bytes. Future `export_archive` can package
`SessionSnapshot` plus `files/`, `artifacts/`, `checkpoints/`, `plan.md`, and
`workspace.yaml`.

## Error Handling

- Corrupt `model_messages.jsonl` returns `SessionError` with path and line.
- Corrupt `events.jsonl` detected by runtime event store construction returns
  `AgentRuntimeError::ProjectionFailed`.
- Existing import target with `FailIfExists` returns a typed conflict error.
- Missing `metadata.json` on `open` returns a typed not-found/invalid-session
  error.
- Corrupt `events.jsonl` detected by `LocalSessionStore::open` is wrapped as
  `SessionError::RuntimeProjection { path, message }`.
- Runtime/provider snapshot cache corruption is nonfatal if the canonical logs
  replay.
- Provider ledger compaction record corruption is fatal in strict mode.

## Testing

Automated coverage:

- `LocalSessionStore::create` writes metadata once and creates expected dirs.
- `LocalSessionStore::open` preserves `created_at`, `host_cwd`, and
  `import_source`.
- Metadata without `last_activity_at` reads with `last_activity_at = updated_at`.
- Runtime construction no longer overwrites existing metadata.
- Resume rejects mismatched `SessionConfig`, metadata id, default thread id, and
  config session id/root/cwd.
- Runtime snapshot cache can be corrupt/stale without preventing resume when
  `events.jsonl` is valid.
- Provider ledger appends `message` records and replays them in order.
- Provider ledger appends `compacted` records and resumes from replacement
  history plus suffix.
- Provider ledger snapshot cache can be corrupt without preventing resume when
  the canonical ledger is valid.
- Corrupt committed ledger line errors with path and line.
- Final nonblank ledger line without newline errors.
- `SessionSnapshot` export contains manifest/resource refs and no file bytes.
- Import into existing session fails under `FailIfExists`.
- Import with `NewId` creates a new session id and preserves import source.
- Resume seeds runtime semantic state from `events.jsonl`.
- Resume seeds provider ledger from `model_messages.jsonl`.
- Resumed provider request sends the replayed provider ledger, proven with a
  payload-callback/fake-provider assertion.
- Second writer/open for same local session id is rejected.

Verification commands:

```bash
cargo fmt --all -- --check
cargo test -p roci-core session::
cargo test -p roci-core --features agent "agent::runtime::tests::session"
cargo clippy -p roci-core --features agent -- -D warnings
cargo check -p roci-cli --features roci/lmstudio
```

Because runtime provider request context changes in `.3`, implementation also
runs a tmux-backed LM Studio smoke when `http://127.0.0.1:1234` is reachable and
records whether `metadata.json`, `events.jsonl`, and `model_messages.jsonl` were
created. Full user-facing CLI session commands still belong to `.6`.

## Follow-Up Work

- `tsq-r0c1ses7.6`: CLI session commands and chat resume flags.
- `tsq-r0c1ses7.7`: docs, automated gates, and live verification.
- `tsq-671d72xe`: tolerant session history repair/import layer.
- Future archive API: portable bundle containing manifest plus resource bytes.
