## Overview
Fix compile-blocking clippy/rust errors first so full lint output can be collected and remediation can proceed safely.

## Constraints / Non-goals
- Fix only blocker-level issues in this task.
- Avoid opportunistic unrelated warning cleanups.

## Interfaces (CLI/API)
- Primary target: `examples/agent_runtime.rs`
- Validation: `cargo clippy --all-targets --all-features`

## Data model / schema changes
None.

## Acceptance criteria
- `E0063` missing-field error in `AgentConfig` initializer is resolved.
- Clippy run progresses through full compilation of targets/examples.
- Changes remain minimal and behavior-safe.

## Test plan
- Run clippy and confirm compile blocker no longer appears.
- Run targeted example build if needed.
