## Overview
Update attachment docs and run final verification for V1 attachments after `.3`, `.4`, and `.5`.

## Acceptance
- Docs describe text attachments, supported images, unsupported-media marker fallback, and no native `ContentPart::File`.
- Docs/testing commands use zsh-safe exit status variables and self-contained attachment fixtures.
- Parent/task notes clarify safe unsupported images become marker text, not strict preflight failure.
- Automated gates pass: fmt, diff check, core attachment tests, provider payload tests, CLI attach tests, clippy core/providers, full workspace clippy, workspace tests.
- Live `roci-agent` text attachment smoke passes with provider response and exit 0.
- Live `roci-agent` unsupported-media smoke passes with provider response and exit 0.
- Session raw-path check finds no raw attached host path in session storage.
- Vision smoke is run if a vision-capable provider/model is loaded; otherwise task note records unavailable provider/model/auth.

## Test Plan
- `cargo fmt --all -- --check`
- `git diff --check`
- `cargo test -p roci-core --features agent attachments::tests::`
- `cargo test -p roci-core --features agent unsupported_media`
- `cargo test -p roci-providers provider_attachment_payload`
- `cargo test -p roci-cli attach`
- `cargo clippy -p roci-core --features agent --all-targets -- -D warnings`
- `cargo clippy -p roci-providers --all-targets -- -D warnings`
- `cargo clippy --workspace --all-targets --features full -- -D warnings`
- `cargo test --workspace --all-targets`
- Live tmux text and unsupported-media smoke through current `target/debug/roci-agent` binary.
- Live vision smoke if LM Studio reports loaded `type == "vlm"`; otherwise record model list state.
