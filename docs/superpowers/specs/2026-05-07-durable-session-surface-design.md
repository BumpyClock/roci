# Durable Session Surface Design

## Overview

Implement one durable session surface for `tsq-r0c1ses7.2`, `tsq-r0c1ses7.4`, and `tsq-r0c1ses7.5`.

Durable sessions are opt-in. Roci never stores session data in the project cwd by default. Host apps provide a session root. Project cwd is metadata/import context only.

## Goals

- Add strict JSONL replay storage for semantic `AgentRuntimeEvent` values.
- Add explicit session resource APIs and events for plan, workspace, artifacts, temp files, checkpoints, and session files.
- Thread session filesystem and logical cwd through tool execution.
- Keep non-session runtime/tool behavior unchanged.
- Preserve strict runtime replay while tracking tolerant product-history recovery as later work in `tsq-671d72xe`.

## Non-Goals

- Do not implement tolerant runtime event replay.
- Do not silently skip corrupt committed `events.jsonl` records.
- Do not store durable sessions under the project cwd unless a host explicitly chooses that root.
- Do not implement a full OS sandbox in this slice.
- Do not implement CLI session commands in this slice unless needed by tests.

## Session Root

Session root layout:

```text
<session_root>/<session_id>/
  metadata.json
  events.jsonl
  files/
  artifacts/
  tmp/
  checkpoints/
  plan.md
  workspace.yaml
```

`<session_root>` is host/app supplied. `SessionConfig` or `LocalSessionStore` owns this location. `SessionMetadata.host_cwd` records where a session was created/imported from, but tools and storage do not operate directly in that cwd.

## Architecture

Core contracts:

- `SessionConfig { id, root, cwd }`
- `LocalSessionFs` for logical paths under `files/`
- `LocalSessionResources` for `artifacts/`, `tmp/`, `checkpoints/`, `plan.md`, and `workspace.yaml`
- `JsonlAgentRuntimeEventStore` implementing `AgentRuntimeEventStore`
- `ToolExecutionContext.session_fs` and `ToolExecutionContext.session_cwd`

Runtime wiring:

- `AgentConfig.session: Option<SessionConfig>` opts into durable sessions.
- Session config creates/injects `JsonlAgentRuntimeEventStore` into `ChatRuntimeConfig.event_store`.
- Runtime event publishing still appends before broadcast.
- Session file/resource APIs emit explicit runtime events where a `thread_id` is known.

## JSONL Runtime Events

`events.jsonl` stores strict records:

```json
{"type":"event","event":{...AgentRuntimeEvent...}}
{"type":"thread_invalidated","thread_id":"...","latest_seq":12}
```

Append rules:

- Validate monotonic `seq` per thread before writing an event.
- Serialize exactly one JSON object per line.
- Append newline.
- Flush after append.
- `invalidate_thread(thread_id, latest_seq)` appends a `thread_invalidated` tombstone.

Replay rules:

- Blank lines are ignored.
- `event` records rebuild per-thread replay state.
- `thread_invalidated` clears prior replay for that thread and sets `latest_seq`.
- Any malformed nonblank line returns `AgentRuntimeError::ProjectionFailed`.
- Error message includes `events.jsonl` path and line number.

This follows Codex `rollout-trace` strictness, not Pi/Codex tolerant history loaders.

## Resource APIs

Session resources use files as payloads. Runtime events carry path and metadata, not file bytes.

APIs:

- `write_plan(markdown)` writes `plan.md`
- `write_workspace_yaml(value_or_string)` writes `workspace.yaml`
- `write_artifact(path, bytes)` writes under `artifacts/`
- `write_temp(path, bytes)` writes under `tmp/`
- `write_checkpoint(path, bytes)` writes under `checkpoints/`
- read/list/delete APIs for artifacts, temp files, and checkpoints
- read APIs for `plan.md` and `workspace.yaml`; no delete API for those root files in this slice

Explicit event payloads:

- `PlanWritten`
- `WorkspaceUpdated`
- `ArtifactCreated`
- `TempFileWritten`
- `CheckpointCreated`
- `SessionFileWritten`
- `SessionFileDeleted`

Events carry:

- optional `thread_id`
- namespace
- logical path
- size
- timestamp
- metadata

Snapshots include resource summaries, not bytes:

- latest plan path and update time
- workspace path and update time
- artifact entries
- checkpoint entries
- temp entries

## Plan Mirroring

Runtime `PlanUpdated` remains the semantic source of truth for plans.

When a session is configured and runtime emits `PlanUpdated`, Roci mirrors the plan text to `plan.md` and emits `PlanWritten`. `plan.md` is a durable workspace artifact, not a second plan authority.

## Tool Filesystem

`ToolExecutionContext` gains optional session fields:

- `session_fs`
- `session_cwd: LogicalPath`

Behavior:

- If session fields are present, file tools resolve paths through `session_cwd.join(path)`.
- `read_file`, `write_file`, `list_directory`, and `grep` operate within `files/`.
- Absolute paths, `..`, Windows separators, and symlink escapes fail.
- If no session context exists, built-in tools keep current host filesystem behavior.
- Session cwd is logical and immutable for this slice. Tools cannot persistently change runtime cwd.

## Shell Classifier

Sessioned shell runs with `current_dir = files/<session_cwd>`.

Before execution, a command classifier rejects obvious escape/read/write risks:

- absolute paths
- `..`
- `cd /`
- redirection targets outside logical paths
- common destructive or project-external patterns

The classifier is not a security boundary. Add a `SandboxProvider` seam in config/context so later security work can replace classifier-only enforcement with OS sandboxing. The full sandbox implementation belongs to `tsq-1av9jz0z`.

## Error Handling

- Runtime replay corruption returns `AgentRuntimeError::ProjectionFailed` with path and line number.
- Session path violations return `SessionError::InvalidLogicalPath` or `SessionError::PathEscapesFilesRoot`.
- Tool session path errors return tool error JSON with tool name and path reason.
- Shell classifier denials return tool error JSON with classifier reason.

## Test Plan

Automated coverage:

- JSONL append/replay order.
- JSONL tombstone invalidation.
- Corrupt nonblank JSONL line errors with line number.
- Blank trailing JSONL line ignored.
- Resource writes create correct files and events.
- `PlanUpdated` mirrors to `plan.md`.
- File tools use session cwd, not process cwd.
- Absolute paths, `..`, Windows separators, and symlink escapes are denied in session tools.
- Shell classifier denies obvious escape commands.
- Non-session tool behavior remains unchanged.

Verification commands:

```bash
cargo test -p roci-core session::
cargo test -p roci-core --features agent "agent::runtime::tests::session"
cargo test -p roci-tools
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --features full -- -D warnings
cargo test
```

Because runtime/tool loop behavior changes, final provider-facing completion also requires live tmux provider verification per `docs/testing.md`.

## Follow-Up Work

`tsq-671d72xe` tracks tolerant product-history recovery. That work should add a separate `SessionHistoryStore` or `ConversationStore` that can return salvageable items plus warnings. It must not make strict `AgentRuntimeEventStore` replay tolerant.
