# roci-cli Subagent Profile Loading Plan

## Task

`tsq-r0c1agt6.5`

## Implementation Steps

1. Export runtime subagent config
   - Re-export `AgentSubagentConfig` from `roci::agent`.

2. Add chat CLI args
   - `--agent <PROFILE>`
   - `--no-subagents`
   - `--list-agents`
   - Update parse tests.

3. Add CLI subagent profile loader
   - New `crates/roci-cli/src/chat/subagents.rs`.
   - Add `mod subagents;` from `crates/roci-cli/src/chat.rs`.
   - Build profile roots from resolved default `ResourceDirectories`.
   - Load built-ins + roots in override order:
     - `~/.roci/agent/subagents/*.toml`
     - `~/.agents/subagents/*.toml`
     - `<cwd>/.roci/subagents/*.toml`
     - `<cwd>/.agents/subagents/*.toml`
   - Validate selected `--agent`.
   - Print deterministic profile summaries for `--list-agents`.

4. Wire runtime config
   - In `handle_chat`, load profile registry after `cwd` is known.
   - If `--list-agents`, print summaries and return before prompt/model/provider call.
   - If not `--no-subagents`, set:
     - `profiles: loaded registry`
     - `supervisor: SubagentSupervisorConfig::default()`
     - `enabled: true`
     - `main_profile: --agent`
   - If `--no-subagents`, set `AgentConfig.subagents = None`.

5. Render subagent semantic events
   - Extend `ChatRenderer::render_payload_to`.
   - Handle all subagent variants:
     - `SubagentStarted`
     - `SubagentProgress`
     - `SubagentToolCallStarted`
     - `SubagentToolCallCompleted`
     - `SubagentMessage`
     - `SubagentNeedsInput`
     - `SubagentCompleted`
     - `SubagentFailed`
     - `SubagentCancelled`
   - Keep output on stderr.
   - Add focused renderer tests asserting lifecycle stderr output.

6. Verify
   - CLI/helper tests prove:
     - `--list-agents` works without prompt/provider execution.
     - `--no-subagents` produces no runtime subagent config.
     - enabled config carries selected `main_profile`.
   - `cargo test -p roci-cli`
   - `cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture`
   - `cargo clippy -p roci-cli --all-targets -- -D warnings`
   - `cargo clippy -p roci-core --features agent,mcp --all-targets -- -D warnings`
   - Live tmux/provider smoke deferred to `tsq-r0c1agt6.6`.

## Review Notes

- Keep CLI ownership in `roci-cli`; core owns profile parsing/registry.
- Do not store subagent profile state in project cwd except user-authored config files.
- Do not expose raw child `AgentEvent` in CLI output.
