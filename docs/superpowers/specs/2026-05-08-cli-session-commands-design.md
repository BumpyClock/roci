# CLI Session Commands Design

## Goal

Add `roci-agent session` commands that let users manage durable local sessions without placing session data in project cwd.

## Scope

This slice covers CLI management for the local durable session store:

- `roci-agent session create`
- `roci-agent session list`
- `roci-agent session delete`
- `roci-agent session export`
- `roci-agent session import`
- existing `roci-agent chat --session-root --session-id` behavior remains supported

## Root Resolution

Session commands default to app data storage, not cwd:

- macOS: `~/Library/Application Support/roci/sessions`
- Linux/Windows: platform app-data equivalent from `directories::ProjectDirs`

Each command accepts `--root <PATH>` to override the default. Chat keeps its explicit `--session-root` requirement for now so chat invocations stay unambiguous and do not silently create app-global sessions.

## Command Behavior

`create` creates a session with optional `--id`, `--title`, and `--json`. It records current cwd as metadata only.

`list` scans direct child dirs under the root and includes entries with valid `metadata.json`. Invalid session dirs are skipped in text mode and surfaced as errors in JSON only if command parsing or root access fails.

`delete <id>` removes one local session dir. It fails if the session does not exist.

`export <id> --output <PATH>` writes a `SessionSnapshot` JSON manifest. Snapshot export does not include resource bytes.

`import --input <PATH>` imports a snapshot. Optional `--id <ID>` imports under a chosen id; otherwise it generates a new id.

## Output

Text output is stable and human-readable. `--json` on create/list/export/import prints machine-readable JSON for automation. Delete prints the removed session id and path.

## Errors

Invalid ids fail at parse/use boundary with a direct error. Missing roots for list return an empty list. Missing session dirs for delete/export fail. Import fails if snapshot JSON is invalid or target exists.

## Tests

Add parser tests in `crates/roci-cli/src/cli/mod.rs` and command tests in a new `crates/roci-cli/src/session_cmd.rs` test module using temp dirs.
