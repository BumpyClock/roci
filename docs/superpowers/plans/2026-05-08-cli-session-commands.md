# CLI Session Commands Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `roci-agent session` create/list/delete/export/import commands backed by the local durable session store.

**Architecture:** Keep CLI parsing in `crates/roci-cli/src/cli/mod.rs`, command execution in a focused `crates/roci-cli/src/session_cmd.rs`, and root resolution as a CLI-local helper. Reuse `LocalSessionStore` for create/export/import and read `metadata.json` for lightweight list/show output.

**Tech Stack:** Rust, clap, tokio, serde_json, directories, roci-core session APIs.

---

### Task 1: CLI Parser Contract

**Files:**
- Modify: `crates/roci-cli/src/cli/mod.rs`

- [ ] Add `Commands::Session(SessionArgs)`.
- [ ] Add `SessionCommands::{Create,List,Delete,Export,Import}`.
- [ ] Add shared `--root <PATH>` and per-command args:
  - create: `--id`, `--title`, `--json`
  - list: `--json`
  - delete: `<id>`
  - export: `<id> --output <PATH> --json`
  - import: `--input <PATH> --id <ID> --json`
- [ ] Add parser tests for every subcommand.

Run: `cargo test -p roci-cli cli::tests::parse_session`

### Task 2: Session Command Handler

**Files:**
- Create: `crates/roci-cli/src/session_cmd.rs`
- Modify: `crates/roci-cli/src/main.rs`
- Modify: `crates/roci-cli/Cargo.toml`

- [ ] Add `directories = "6"` dependency.
- [ ] Implement default root via `ProjectDirs::from("", "", "roci").data_dir().join("sessions")`.
- [ ] Implement `handle_session`.
- [ ] Implement create/list/delete/export/import helpers.
- [ ] Use `LocalSessionStore::{create, export_snapshot, import_snapshot}` where possible.
- [ ] Use `SessionMetadata::read_from_path` for list.
- [ ] Serialize JSON with `serde_json::to_string_pretty`.

Run: `cargo test -p roci-cli session_cmd`

### Task 3: Docs And Verification

**Files:**
- Modify: `docs/agent-runtime-chat.md`

- [ ] Document default root and session commands.
- [ ] Run full gates:
  - `cargo fmt --all -- --check`
  - `cargo clippy -p roci-cli --features roci/lmstudio -- -D warnings`
  - `cargo test -p roci-cli`
  - `cargo check -p roci-cli --features roci/lmstudio`
- [ ] Run live tmux smoke:
  - create session in temp root
  - list it
  - export it
  - import it under new id
  - delete imported id

Expected: command output shows created/listed/exported/imported/deleted sessions and files exist under app/root, not project cwd.
