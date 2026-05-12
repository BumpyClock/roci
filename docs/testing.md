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
cargo test -p roci-cli        # CLI tests (arg parsing, error formatting, session commands)
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

Model catalog smoke (required for model listing changes):

```bash
tmux new-session -d -s roci-model-catalog \
  'cd /path/to/roci && \
   set -o pipefail; \
   cargo build -q -p roci-cli; \
   code=$?; printf "\n[roci cli build exit=%s]\n" "$code"; \
   if [ "$code" -ne 0 ]; then \
     echo "[roci cli build failed]"; \
     exit "$code"; \
   fi; \
   cargo run -q -p roci-cli -- models list --provider openai --json; \
   code=$?; printf "\n[models list --provider openai exit=%s]\n" "$code"; \
   exec zsh'
echo "attach: tmux attach -t roci-model-catalog"
```

Runtime model switching smoke (required for runtime switching changes):

```bash
tmux new-session -d -s roci-model-switch \
  'cd /path/to/roci && \
   set -o pipefail; \
   cargo build -q -p roci-cli; \
   code=$?; printf "\n[roci cli build exit=%s]\n" "$code"; \
   if [ "$code" -ne 0 ]; then \
     echo "[roci cli build failed]"; \
     exit "$code"; \
   fi; \
   cargo run -q -p roci-cli -- models switch-smoke --from openai:gpt-4o --to openai:gpt-4.1 --json; \
   code=$?; printf "\n[models switch-smoke exit=%s]\n" "$code"; \
   exec zsh'
echo "attach: tmux attach -t roci-model-switch"
```

Copilot catalog smoke (authenticated, with static fallback):

```bash
tmux new-session -d -s roci-model-catalog-copilot \
  'cd /path/to/roci && \
   set -o pipefail; \
   cargo run -q -p roci-cli -- models list --provider github-copilot --json; \
   code=$?; printf "\n[github-copilot models list exit=%s]\n" "$code"; \
   if [ "$code" -ne 0 ]; then \
     echo "[copilot dynamic unavailable] falling back to all-provider/static catalog smoke"; \
     cargo run -q -p roci-cli -- models list --json; \
     code=$?; printf "\n[models list --json exit=%s]\n" "$code"; \
   fi; \
   exec zsh'
echo "attach: tmux attach -t roci-model-catalog-copilot"
```

```bash
tmux new-session -d -s roci-live-provider \
  'cd /path/to/roci && \
   LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
   cargo run -q -p roci-cli --features roci/lmstudio -- \
   chat --no-skills --model "lmstudio:<model-id>" \
   --temperature 0 --max-tokens 32 \
   "Reply exactly: roci-local-smoke-ok"; \
   roci_status=$?; printf "\n[roci smoke exit=%s]\n" "$roci_status"; exec zsh'
echo "attach: tmux attach -t roci-live-provider"
```

Attachment changes need end-to-end CLI check (not only unit tests):

```bash
printf 'roci-text-attach-marker-6201' > /tmp/roci-attach-notes.txt
tmux new-session -d -s roci-live-attach \
  'cd /path/to/roci && \
   LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
   cargo run -q -p roci-cli --features roci/lmstudio -- \
   chat --attach /tmp/roci-attach-notes.txt \
   --session-root /tmp/roci-attach-smoke \
   --model "lmstudio:<model-id>" \
   "Repeat exactly the roci marker from the attachment."; \
   roci_status=$?; printf "\n[roci-agent chat --attach exit=%s]\n" "$roci_status"; exec zsh'
echo "attach: tmux attach -t roci-live-attach"
```

Unsupported media should reach the model as a bounded text marker:

```bash
printf '\000\237\222\226' > /tmp/roci-unsupported-media.pdf
tmux new-session -d -s roci-live-attach-unsupported \
  'cd /path/to/roci && \
   LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
   cargo run -q -p roci-cli --features roci/lmstudio -- \
   chat --no-skills --model "lmstudio:<model-id>" \
   --session-root /tmp/roci-attach-smoke --session-id unsupported-media \
   --attach /tmp/roci-unsupported-media.pdf \
   "Repeat exactly any unsupported attachment notice you see."; \
   roci_status=$?; printf "\n[roci-agent chat --attach exit=%s]\n" "$roci_status"; exec zsh'
echo "attach: tmux attach -t roci-live-attach-unsupported"
rg -n '/tmp/roci-unsupported-media.pdf|/tmp/roci-attach-smoke' /tmp/roci-attach-smoke/unsupported-media
rg -n 'roci-unsupported-media.pdf|application/pdf' /tmp/roci-attach-smoke/unsupported-media
```

The raw-path `rg` should exit `1`; marker text may include only the attachment
basename, MIME type, and size, which the second `rg` should find.

Session verification must inspect persisted snapshots/files and assert sanitized
attachment metadata has no raw host paths:

```bash
SESSION_ROOT=/tmp/roci-session-smoke-attach
SESSION_ID=runtime-attach-smoke
cargo run -q -p roci-cli -- session export "$SESSION_ID" --root "$SESSION_ROOT" --output "$SESSION_ROOT/snapshot.json"
rg -n 'file_path|raw_data|<attached-source-path>' "$SESSION_ROOT/$SESSION_ID"
```

Acceptance: metadata entries can include attachment names, MIME/type, and sizes,
but not host file paths. Provider-facing payloads may include rendered text
attachments or base64 image parts because they preserve the model input.

Use `curl http://127.0.0.1:1234/api/v0/models` to confirm at least one local model is `loaded`. If local models are unavailable or not loaded, say so and use the configured remote provider most relevant to the change.

Remote provider smoke tests must not print secrets. Pass credentials through the environment or existing auth store, print only whether a credential is configured, and keep the prompt/token budget small.

Tool contract smoke (result cap, prompt metadata, alias normalization evidence):

```bash
tmux new-session -d -s roci-tool-contracts \
  'cd /path/to/roci && \
   set -o pipefail; \
   OPENAI_API_KEY=sk-local-dummy \
   cargo run -q -p roci-cli -- tool-contracts-smoke \
     --model openai:gemma-4-e4b \
     --endpoint http://framed:4001/v1 \
     --case all; \
   code=$?; printf "\n[tool contracts smoke exit=%s]\n" "$code"; \
   exec zsh'
echo "attach: tmux attach -t roci-tool-contracts"
```

## Durable session verification

Session storage must stay under the host-selected root. Do not treat the
project cwd as implicit session storage.

CLI session management smoke:

```bash
ROOT=/tmp/roci-session-smoke
cargo run -q -p roci-cli -- session create --root "$ROOT" --id smoke --title Smoke --json
cargo run -q -p roci-cli -- session list --root "$ROOT" --json
cargo run -q -p roci-cli -- session export smoke --root "$ROOT" --output "$ROOT/export.json" --json
cargo run -q -p roci-cli -- session import --root "$ROOT" --input "$ROOT/export.json" --id smoke-import --json
cargo run -q -p roci-cli -- session delete smoke-import --root "$ROOT"
test -f "$ROOT/smoke/metadata.json"
test -f "$ROOT/export.json"
test ! -e "$ROOT/smoke-import"
```

Recovery smoke:

```bash
ROOT=/tmp/roci-session-recovery-smoke
rm -rf "$ROOT"
cargo run -q -p roci-cli -- session create --root "$ROOT" --id recover-smoke --title Recover --json
printf 'not json\n' >> "$ROOT/recover-smoke/events.jsonl"
cargo run -q -p roci-cli -- session recover-export recover-smoke --root "$ROOT" --output "$ROOT/recovered.json" --json
rg -n '"artifact_type": "roci_recovered_session"|"warnings"' "$ROOT/recovered.json"
cargo run -q -p roci-cli -- session recover-import --root "$ROOT" --input "$ROOT/recovered.json" --id recover-smoke-import --json
```

Provider-facing durable resume smoke should run in tmux and exercise chat with
an explicit session root:

```bash
tmux new-session -d -s roci-session-resume \
  'cd /path/to/roci && \
   ROOT=/tmp/roci-session-resume && \
   LMSTUDIO_BASE_URL=http://127.0.0.1:1234 \
   cargo run -q -p roci-cli --features roci/lmstudio -- \
   chat --no-skills --model "lmstudio:<model-id>" \
   --temperature 0 --max-tokens 32 \
   --session-root "$ROOT" --session-id live-resume \
   "Reply exactly: roci session live ok"; \
   roci_status=$?; printf "\n[roci session smoke exit=%s]\n" "$roci_status"; \
   ls -la "$ROOT/live-resume"; exec zsh'
echo "attach: tmux attach -t roci-session-resume"
```

Provider-facing durable recovery smoke should run in tmux and use framed OpenAI-compatible endpoint:

```bash
tmux new-session -d -s roci-session-recovery-live \
  'cd /Users/adityasharma/Projects/roci && \
   set -o pipefail; \
   ROOT=/tmp/roci-session-recovery-live && \
   rm -rf "$ROOT" && \
   OPENAI_API_KEY=sk-local-dummy \
   OPENAI_BASE_URL=http://framed:4001/v1 \
   cargo run -q -p roci-cli -- chat --no-skills --no-tools --model "openai:gemma-4-e4b" \
   --session-root "$ROOT" --session-id recovery-live \
   "Seed durability context for recovery smoke."; \
   seed_status=$?; printf "\n[seed chat recovery-live exit=%s]\n" "$seed_status"; \
   cargo run -q -p roci-cli -- session recover-export recovery-live --root "$ROOT" --output "$ROOT/recovered.json" --json; \
   recover_export_status=$?; printf "\n[session recover-export exit=%s]\n" "$recover_export_status"; \
   rg -n '\"artifact_type\": \"roci_recovered_session\"|\"warnings\"' "$ROOT/recovered.json"; \
   cargo run -q -p roci-cli -- session recover-import --root "$ROOT" --input "$ROOT/recovered.json" --id recovery-live-import --json; \
   recover_import_status=$?; printf "\n[session recover-import exit=%s]\n" "$recover_import_status"; \
   ls -la "$ROOT/recovery-live-import"; \
   cargo run -q -p roci-cli -- chat --no-skills --no-tools --model "openai:gemma-4-e4b" \
   --session-root "$ROOT" --session-id recovery-live-import \
   "Reply exactly: roci session recovery live ok"; \
   resume_status=$?; printf "\n[resume chat recovery-live-import exit=%s]\n" "$resume_status"; \
   exec zsh'
echo "attach: tmux attach -t roci-session-recovery-live"
```

## Subagent live verification

Subagent CLI/runtime changes must prove the real `roci-agent` binary can:

- load a profile from `.roci/subagents/*.toml`
- expose `delegate_subagent` to the parent model
- run a child provider call
- render semantic subagent events
- return the child summary to the parent turn

Framed OpenAI-compatible endpoint:

```bash
# Replace /path/to/roci/Cargo.toml with the local checkout path.
tmux new-session -d -s roci-subagent-live '
  set -o pipefail
  # Temporary cwd prevents session/test artifacts from polluting the repo.
  WORKDIR=$(mktemp -d /tmp/roci-subagent-live-cwd.XXXXXX)
  mkdir -p "$WORKDIR/.roci/subagents"
  cat > "$WORKDIR/.roci/subagents/smoke.toml" <<EOF
[[profiles]]
name = "smoke"
display_name = "Smoke Worker"
default = true
[[profiles.models]]
provider = "openai"
# Local framed test infra model. Use an equivalent configured smoke model if unavailable.
model = "gemma-4-e4b"
EOF
  cd "$WORKDIR"
  # framed:4001 is local OpenAI-compatible test infra from AGENTS.md.
  OPENAI_API_KEY=sk-local-dummy \
  OPENAI_BASE_URL=http://framed:4001/v1 \
  cargo run -q --manifest-path /path/to/roci/Cargo.toml -p roci-cli -- \
    chat --no-skills --agent smoke --model openai:gemma-4-e4b \
    --temperature 0 --max-tokens 220 \
    "Use delegate_subagent with profile smoke to ask child to reply exactly roci-subagent-live-ok. Then return the child summary." \
    2>&1 | tee /tmp/roci-subagent-live.log
  roci_status=$?
  printf "\n[roci subagent live exit=%s]\n" "$roci_status" | tee -a /tmp/roci-subagent-live.log
  exec zsh
'
echo "attach: tmux attach -t roci-subagent-live"
```

Acceptance: log contains `[subagent] started`, `[subagent] ... completed`,
`roci-subagent-live-ok`, and `[roci subagent live exit=0]`.

## Environment

Use `.env` for local secrets and `.env.example` as the template.
