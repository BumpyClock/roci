## Overview
Implement durable sessions as a provider-agnostic roci-core foundation: stable session IDs, session-owned filesystem, JSONL runtime event persistence, snapshot import/export, plan/workspace APIs, tool execution context, and CLI session commands.

## Constraints / Non-goals
- Active development: breaking API changes allowed; no compatibility shims.
- `AgentConfig.session_id` remains provider prompt-cache/session ID. Durable sessions use new `AgentConfig.session` / async session constructors.
- Workspace paths are session-owned under `files/`; host cwd is metadata/import source only.
- JSONL load corruption must return a visible error. Allow only final empty/truncated line recovery if clearly incomplete.
- No provider-specific storage in roci-core.

## Interfaces (CLI/API)
- `roci_core::session::{SessionId, LogicalPath, PathConventions, SessionMetadata, SessionSnapshot, SessionWorkspace}`.
- `SessionStore`, `SessionFs`, `LocalSessionStore`, `LocalSessionFs`.
- `AgentConfig.session: Option<SessionConfig>`.
- Async runtime constructors/resume APIs, e.g. `AgentRuntime::new_sessioned(...)`, `resume_session(...)`, `export_session_snapshot(...)`.
- CLI: `roci-agent session create/list/delete/export/import`; chat flags `--new-session`, `--session <id>`, `--session-root <dir>`.

## Data model / schema changes
- Session root contains metadata, semantic runtime event JSONL, artifacts, temp, checkpoints, plan.md, workspace.yaml, files/.
- Logical paths cannot be absolute, contain `..`, or escape session root through symlinks.
- `ToolExecutionContext` gains optional session fs/cwd.
- Runtime session snapshot includes metadata, thread snapshot, model messages, session usage, plan, workspace.

## Acceptance criteria
- Session path/id/local fs tests pass.
- JSONL replay works and corrupt committed JSONL line returns user-visible error.
- Runtime can create/resume/export/import sessions through async API.
- Builtin file tools operate through session cwd/fs when configured.
- CLI session commands and chat flags work.
- Docs ADR and testing docs updated.

## Test plan
- `cargo test -p roci-core session::`
- `cargo test -p roci-core --features agent "agent::runtime::tests::session"`
- `cargo test -p roci-tools`
- `cargo test -p roci-cli`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --features full -- -D warnings`
- `cargo test`
- Live tmux provider smoke after provider loop/tool path touched per `docs/testing.md`.
