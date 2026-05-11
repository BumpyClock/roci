# roci-cli Subagent Profile Loading Design

## Overview

`tsq-r0c1agt6.5` - Add roci-cli subagent profile loading and selection rendering.

## Goals

- Load built-in, global, and project subagent profiles in `roci-agent chat`.
- Add CLI controls:
  - `--agent <PROFILE>` selects the main/default agent profile projection.
  - `--no-subagents` disables subagent runtime tooling.
  - `--list-agents` prints loaded profile summaries and exits.
- Wire loaded profiles into `AgentConfig.subagents`.
- Render parent-visible semantic subagent runtime events in `roci-agent chat`.

## Non-Goals

- Full interactive `/agent` or `/model` command.
- Live provider verification; `tsq-r0c1agt6.6` owns docs plus live tmux smoke.
- Recursive child-to-child routing.
- MCP server projection in CLI beyond profile data already parsed by core.

## Profile Roots

Start with `SubagentProfileRegistry::with_builtins()`, then load TOML profile files from roots in this order, where later roots override earlier profiles:

1. global `.roci`: `~/.roci/agent/subagents/*.toml`
2. global `.agents`: `~/.agents/subagents/*.toml`
3. project `.roci`: `<cwd>/.roci/subagents/*.toml`
4. project `.agents`: `<cwd>/.agents/subagents/*.toml`

This mirrors existing resource/skill global-vs-project ergonomics while keeping profile files out of arbitrary project cwd.

## Interfaces (CLI/API)

- Default chat enables subagent routing tools when profiles load successfully.
- `--no-subagents` leaves `AgentConfig.subagents = None`; no routing tools appear.
- `--agent <PROFILE>` validates that the profile resolves and sets `AgentSubagentConfig.main_profile`.
- `--list-agents` prints deterministic text rows:
  - id
  - display name
  - default marker
  - model candidates
  - description/infer preview
- `--list-agents --no-subagents` is valid and prints the catalog without wiring tools.

API surface:

- `roci::agent::AgentSubagentConfig` is re-exported so CLI can construct runtime subagent config without depending on private modules.
- CLI helper returns a `SubagentProfileRegistry` plus optional selected main profile reference.

## Data model / schema changes

- No persisted session schema changes.
- No provider payload schema changes.
- CLI args add three fields to `ChatArgs`:
  - `agent: Option<String>`
  - `no_subagents: bool`
  - `list_agents: bool`
- `AgentConfig.subagents` receives `AgentSubagentConfig` when subagents are enabled.

## Runtime Rendering

`RuntimeEventRenderer` must render semantic subagent payloads, not raw child `AgentEvent`.

Minimum events:

- `SubagentStarted`: profile/label/model/id.
- `SubagentProgress`: optional short progress message.
- `SubagentToolCallStarted` / `Completed`: child tool name/call/result.
- `SubagentMessage`: child message text preview.
- `SubagentNeedsInput`: child question.
- `SubagentCompleted` / `Failed` / `Cancelled`: terminal status.

Renderer output goes to stderr so assistant stdout remains model response text.

## Test plan

- `roci-agent chat --list-agents` lists built-ins without requiring a prompt/provider call.
- `roci-agent chat --agent <profile>` parses and validates the profile.
- `roci-agent chat --no-subagents` parses and does not wire subagent config.
- Chat config sets `AgentSubagentConfig` when subagents are enabled.
- Subagent runtime events produce visible CLI lifecycle lines.
- Automated tests cover CLI parsing, profile root loading/override, subagent config wiring, list output, and event rendering.

Commands:

- `cargo test -p roci-cli`
- `cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture`
- `cargo clippy -p roci-cli --all-targets -- -D warnings`
- `cargo clippy -p roci-core --features agent,mcp --all-targets -- -D warnings`

## Acceptance Criteria

- `roci-agent chat --list-agents` lists built-ins without requiring a prompt/provider call.
- `roci-agent chat --agent <profile>` parses and validates the profile.
- `roci-agent chat --no-subagents` parses and does not wire subagent config.
- Chat config sets `AgentSubagentConfig` when subagents are enabled.
- Subagent runtime events produce visible CLI lifecycle lines.
- Automated tests cover CLI parsing, profile root loading/override, subagent config wiring, list output, and event rendering.
