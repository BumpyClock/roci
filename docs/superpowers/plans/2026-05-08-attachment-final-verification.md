# Attachment Final Verification Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close `tsq-r0c1att8.6` by aligning attachment docs with final fallback behavior and rerunning automated plus live CLI verification.

**Architecture:** No new runtime/provider behavior. Update stale docs and testing snippets to describe the final V1 contract: text and supported images reach providers, safe unsupported media becomes bounded marker text, and raw host paths stay out of persisted metadata. Verification uses current `target/debug/roci-agent` through LM Studio plus cargo gates.

**Tech Stack:** Markdown docs, Tasque, Rust cargo tests/clippy/rustfmt, tmux, LM Studio at `http://127.0.0.1:1234`.

---

## File Structure

- Modify `docs/agent-runtime-chat.md`: clarify attachment compile/preflight behavior and unsupported-media marker persistence.
- Modify `docs/testing.md`: fix zsh-safe exit-code variable, add unsupported-media smoke and raw path check.
- Modify `docs/superpowers/specs/2026-05-08-attachment-capabilities-preflight-design.md`: remove stale “text-only rejects images” language.
- Modify `docs/superpowers/specs/2026-05-08-provider-attachment-payload-design.md`: final verification wording says text/unsupported-media plus vision when available.
- Modify `docs/superpowers/specs/2026-05-08-runtime-prompt-input-design.md`: remove stale non-vision image rejection wording.
- Create or update `tsq-r0c1att8.6` spec with docs + verification acceptance.
- No Rust source edits expected.

---

## Task 1: Patch Documentation

**Files:**
- Modify: `docs/agent-runtime-chat.md`
- Modify: `docs/testing.md`
- Modify: `docs/superpowers/specs/2026-05-08-attachment-capabilities-preflight-design.md`
- Modify: `docs/superpowers/specs/2026-05-08-provider-attachment-payload-design.md`
- Modify: `docs/superpowers/specs/2026-05-08-runtime-prompt-input-design.md`

- [ ] **Step 1: Update runtime docs**

In `docs/agent-runtime-chat.md`, update the attachment method bullets to say runtime resolves, downgrades safe unsupported media to marker text, preflights resource limits, and then mutates runtime state. Add a short paragraph under attachment metadata:

```markdown
Safe unsupported media is rendered as model-visible text:
`User attached unsupported media: <name> (<mime>, <size> bytes). Content omitted.`
The marker uses sanitized display names and MIME metadata; raw host paths are not persisted.
```

- [ ] **Step 2: Update preflight design stale language**

In `docs/superpowers/specs/2026-05-08-attachment-capabilities-preflight-design.md`, replace:

```markdown
Fails before provider execution when selected model capabilities reject the attachment.
Non-vision model rejects image attachments.
Image rejection for text-only model capabilities.
```

with wording that distinguishes content fallback from resource failures:

```markdown
Degrades safe unsupported media and unsupported image inputs into bounded marker text.
Resource limits, unreadable paths, invalid text UTF-8, and malformed marker metadata still fail before provider execution.
```

- [ ] **Step 3: Update testing guide commands**

In `docs/testing.md`, replace zsh `status=$?` snippets inside tmux commands with `roci_status=$?`. Add unsupported-media smoke:

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
```

Add raw path check:

```bash
rg -n '/tmp/roci-unsupported-media.pdf|/tmp/roci-attach-smoke' /tmp/roci-attach-smoke/unsupported-media
```

Expected: exit code `1`.

- [ ] **Step 4: Update provider payload design final verification sentence**

In `docs/superpowers/specs/2026-05-08-provider-attachment-payload-design.md`, replace “text/vision smoke” with “text + unsupported-media smoke, and vision smoke when a vision-capable provider is loaded/configured.”

- [ ] **Step 5: Update prompt input design stale language**

In `docs/superpowers/specs/2026-05-08-runtime-prompt-input-design.md`, remove text that says non-vision images are rejected before provider request. Replace with text that says safe unsupported media and unsupported image inputs are compiled as bounded marker text; native provider file uploads remain out of scope.

---

## Task 2: Update Tasque Spec

**Files:**
- Tasque spec for `tsq-r0c1att8.6`

- [ ] **Step 1: Attach `.6` spec**

Run:

```bash
tsq spec tsq-r0c1att8.6 --force --text '<spec markdown>'
```

Spec content must include:

```markdown
## Overview
Update attachment docs and run final verification for the full V1 attachment stack.

## Acceptance
- Docs describe text attachments, supported images, unsupported-media marker fallback, and no native `ContentPart::File`.
- Docs/testing commands use zsh-safe exit status variables.
- Automated gates pass: fmt, diff check, core attachment tests, provider payload tests, CLI attach tests, clippy core/providers, workspace tests.
- Live `roci-agent` text attachment smoke passes with provider response and exit 0.
- Live `roci-agent` unsupported-media smoke passes with provider response and exit 0.
- Session raw-path check finds no raw attached host path in session storage.
- Vision smoke is run if a vision-capable provider/model is available; otherwise task note records unavailable provider/model.
```

- [ ] **Step 2: Verify `.6` spec**

Run:

```bash
tsq spec tsq-r0c1att8.6 --show
```

Expected: spec reflects final fallback behavior.

---

## Task 3: Automated Verification

**Files:**
- No edits expected.

- [ ] **Step 1: Run formatting and whitespace gates**

Run:

```bash
cargo fmt --all -- --check
git diff --check
```

Expected: both pass.

- [ ] **Step 2: Run focused attachment/provider/CLI tests**

Run:

```bash
cargo test -p roci-core --features agent attachments::tests::
cargo test -p roci-core --features agent unsupported_media
cargo test -p roci-providers provider_attachment_payload
cargo test -p roci-cli attach
```

Expected: all pass.

- [ ] **Step 3: Run clippy gates**

Run:

```bash
cargo clippy -p roci-core --features agent --all-targets -- -D warnings
cargo clippy -p roci-providers --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features full -- -D warnings
```

Expected: all pass.

- [ ] **Step 4: Run full workspace tests**

Run:

```bash
cargo test --workspace --all-targets
```

Expected: pass.

---

## Task 4: Live CLI Verification

**Files:**
- No edits expected.

- [ ] **Step 1: Confirm local provider state**

Run:

```bash
curl -sS http://127.0.0.1:1234/api/v0/models
```

Expected: at least one `state: "loaded"` model. Store the loaded LLM model id as `LMSTUDIO_MODEL_ID` and use `--model "lmstudio:$LMSTUDIO_MODEL_ID"` in live commands. If no vision model is loaded/configured, record that vision smoke is unavailable.

- [ ] **Step 2: Build current CLI**

Run:

```bash
cargo build -p roci-cli --features roci/lmstudio
```

Expected: `target/debug/roci-agent` exists.

- [ ] **Step 3: Run text attachment live smoke**

Show attach command:

```bash
tmux attach -t roci-attach-text
```

Run:

```bash
printf 'roci-text-attach-marker-6201' > /tmp/roci-attach-notes.txt
rm -rf /tmp/roci-attach-text-smoke /tmp/roci-attach-text-smoke.log
tmux new-session -d -s roci-attach-text zsh
tmux send-keys -t roci-attach-text 'cd /Users/adityasharma/Projects/roci' C-m
tmux send-keys -t roci-attach-text 'LMSTUDIO_BASE_URL=http://127.0.0.1:1234 ./target/debug/roci-agent chat --no-skills --model "lmstudio:'"$LMSTUDIO_MODEL_ID"'" --temperature 0 --max-tokens 80 --session-root /tmp/roci-attach-text-smoke --session-id text-attach --attach /tmp/roci-attach-notes.txt "Repeat exactly the roci marker from the attachment." 2>&1 | tee /tmp/roci-attach-text-smoke.log; roci_status=${pipestatus[1]}; printf "\n[roci-agent text attach exit=%s]\n" "$roci_status" | tee -a /tmp/roci-attach-text-smoke.log' C-m
```

Expected: output includes `roci-text-attach-marker-6201` and exit `0`.

- [ ] **Step 4: Run unsupported-media live smoke**

Show attach command:

```bash
tmux attach -t roci-attach-unsupported
```

Run:

```bash
printf '\000\237\222\226' > /tmp/roci-unsupported-media.pdf
rm -rf /tmp/roci-attach-smoke /tmp/roci-attach-smoke.log
tmux new-session -d -s roci-attach-unsupported zsh
tmux send-keys -t roci-attach-unsupported 'cd /Users/adityasharma/Projects/roci' C-m
tmux send-keys -t roci-attach-unsupported 'LMSTUDIO_BASE_URL=http://127.0.0.1:1234 ./target/debug/roci-agent chat --no-skills --model "lmstudio:'"$LMSTUDIO_MODEL_ID"'" --temperature 0 --max-tokens 80 --session-root /tmp/roci-attach-smoke --session-id unsupported-media --attach /tmp/roci-unsupported-media.pdf "Repeat exactly any unsupported attachment notice you see." 2>&1 | tee /tmp/roci-attach-smoke.log; roci_status=${pipestatus[1]}; printf "\n[roci-agent chat --attach exit=%s]\n" "$roci_status" | tee -a /tmp/roci-attach-smoke.log' C-m
```

Expected: output includes `User attached unsupported media: roci-unsupported-media.pdf (application/pdf, 4 bytes). Content omitted.` and exit `0`.

- [ ] **Step 5: Verify raw path absence**

Run:

```bash
rg -n '/tmp/roci-unsupported-media.pdf|/tmp/roci-attach-smoke' /tmp/roci-attach-smoke/unsupported-media
```

Expected: exit code `1`.

- [ ] **Step 6: Run vision smoke or record unavailable**

If `curl -sS http://127.0.0.1:1234/api/v0/models` reports a loaded `type: "vlm"` model, run a PNG attachment smoke with `--model "lmstudio:<loaded-vlm-id>"` and assert provider response plus exit `0`. If no loaded VLM exists, record exact model list state in the `.6` tsq note.

- [ ] **Step 7: Close tmux sessions**

Run:

```bash
tmux kill-session -t roci-attach-text 2>/dev/null || true
tmux kill-session -t roci-attach-unsupported 2>/dev/null || true
```

---

## Task 5: Close Task

**Files:**
- Tasque task history only.

- [ ] **Step 1: Add final evidence note and close `.6`**

Run:

```bash
tsq done tsq-r0c1att8.6 --note '<verification evidence summary>'
```

Expected: `.6` closes and parent has no remaining open P0 attachment child.

## Self-Review

- Spec coverage: docs, tasque spec, automated tests, text live smoke, unsupported live smoke, raw path privacy, and vision availability are all covered.
- Placeholder scan: no TBD/TODO/fill-in placeholders.
- Type/command consistency: commands use existing `roci-agent chat --model provider:model`, `--session-root`, `--session-id`, `--attach`, and zsh-safe `roci_status`.
