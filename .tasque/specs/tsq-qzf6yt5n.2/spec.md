## Goal
Bound tool-result size centrally without forcing every tool to hand-roll truncation.

## Scope
- Add per-tool result budget (`max_result_size_bytes` recommended unit).
- Apply to serialized JSON output in the runner after execution / post-hook mutation.
- Preserve existing built-in self-truncation where it improves UX; central policy is the safety net.

## Decisions
- Default should be conservative but not lossy for small outputs; tools may opt out with `None`/unbounded if they already self-bound.
- Overflow result should be deterministic and machine-readable, e.g. preview + original size + truncated flag.
- Do not introduce disk persistence in the SDK as part of this task.

## Acceptance
- Oversized results do not blow up conversation state.
- Post-hook mutated results are still bounded before message append.
- Tests cover UTF-8 safe truncation and stable overflow envelope.