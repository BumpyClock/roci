# Roci Testing Guide

## Default suite (hermetic)

- Command: `cargo test`
- Network: no external calls
- Runs tests across all workspace crates

### Per-crate testing

```bash
cargo test -p roci-core       # Core SDK kernel (traits, types, config, auth)
cargo test -p roci-core --features agent "agent::runtime::tests::"  # AgentRuntime tests in crates/roci-core/src/agent/runtime_tests
cargo test -p roci-providers  # Provider transports
cargo test -p roci            # Meta-crate integration tests
cargo test -p roci-cli        # CLI tests (arg parsing, error formatting)
cargo test -p roci-tools      # Tool tests (25 tests covering all tools)
```

## Integration and feature-gated tests

- Root integration tests: `cargo test --test meta_crate_integration`
- Core integration tests: `cargo test -p roci-core --test registry_integration`
- MCP tests (feature-gated in `roci-core`): `cargo test -p roci-core --features mcp`
- Runtime namespace inventory: `cargo test -p roci-core --features agent "agent::runtime::tests::" -- --list`
- To inspect test output: append `-- --nocapture`

## Live tmux/provider verification

Do not call provider-facing work done, fixed, ready, or successfully verified until a live provider call has run in an interactive tmux session and produced a successful provider response.

Minimum evidence:

- Show the attach command before or while the live run is active.
- Run the CLI or example app path that exercises the changed provider-facing behavior.
- Use `roci-agent` for end-to-end SDK + CLI validation when possible.
- Capture the provider, model, endpoint when relevant, command, observable response text, and exit code.
- Treat automated tests and subagent reports as insufficient by themselves for provider-facing completion claims.

Preferred local provider smoke test:

```bash
tmux new-session -d -s roci-live-provider \
  'cd /path/to/roci && \
   LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
   cargo run -q -p roci-cli --features roci/lmstudio -- \
   chat --no-skills --model "lmstudio:<model-id>" \
   --temperature 0 --max-tokens 32 \
   "Reply exactly: roci-local-smoke-ok"; \
   status=$?; printf "\n[roci smoke exit=%s]\n" "$status"; exec zsh'
echo "attach: tmux attach -t roci-live-provider"
```

Use `curl http://127.0.0.1:1234/api/v0/models` to confirm at least one local model is `loaded`. If local models are unavailable or not loaded, say so and use the configured remote provider most relevant to the change.

Remote provider smoke tests must not print secrets. Pass credentials through the environment or existing auth store, print only whether a credential is configured, and keep the prompt/token budget small.

## Environment

Use `.env` for local secrets and `.env.example` as the template.
