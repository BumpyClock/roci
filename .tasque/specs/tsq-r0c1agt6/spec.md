## Overview
Add session-level subagent routing on top of the existing supervisor: profile display/routing fields, selectable current subagent set, per-subagent isolation, delegate_subagent tool, semantic events, and CLI profile loading/rendering.

## Constraints / Non-goals
- Active development: breaking API changes allowed; no compatibility shims.
- `infer` is `Option<String>`: model-facing routing hint text, not bool.
- Do not expose raw child `AgentEvent` as stable public runtime API; project semantic subagent events.
- Plan and implement per-subagent MCP/tool isolation in V1, not parse-only placeholder.
- Preserve supervisor-first architecture; no peer bus rewrite.

## Interfaces (CLI/API)
- Extend `SubagentProfile` with `display_name`, `infer`, `skills`, `mcp_servers`, `default_agent_excluded_tools`.
- Add `SubagentRoutingConfig`, `SubagentRoutingController`, `DelegateSubagentRequest`, `DelegateSubagentResult`.
- Add `AgentConfig.subagents` and runtime accessor for controller.
- Add `delegate_subagent` tool generated from selected profiles and isolation policy.
- CLI: `--agent`, `--no-subagents`, `--list-agents` plus profile roots from global/project config.

## Data model / schema changes
- TOML profile parser supports new fields in single and multi-profile files.
- Profile resolution merges scalar fields, model list, tools, skills, MCP server refs, and default-agent exclusions with documented precedence.
- Runtime chat events add semantic subagent lifecycle/update payloads with stable DTOs.
- Child runtime receives only tools, skills, and MCP servers allowed by resolved profile isolation.

## Acceptance criteria
- TOML parse tests cover all new fields.
- Registry/controller tests cover list/current/select/deselect and unknown profile errors.
- Runtime tests prove delegate injection, default-agent exclusion, `--no-tools` behavior, and semantic subagent event projection.
- Fake-provider delegate test proves parent can delegate and receive structured result.
- CLI tests cover profile load, list, select, and no-subagents.
- Docs and live tmux smoke updated/run.

## Test plan
- `cargo test -p roci-core --features agent subagent`
- `cargo test -p roci-core --features agent "agent::runtime::tests::subagent"`
- `cargo test -p roci-cli chat`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets --features full -- -D warnings`
- `cargo test`
- Live tmux subagent delegation smoke per `docs/testing.md`.
