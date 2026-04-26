## Goal
Add token-budget-aware trimming and diagnostics for system-prompt assembly.

## Deliver
- Prompt budget config (`max_prompt_tokens`, `reserve_tokens`, estimator selection).
- Deterministic trim pipeline for section/fragment compaction and pruning.
- Render diagnostics describing dropped/truncated fragments.
- Tests/specs for stable trim behavior under different budgets.

## Decision guardrails
- Prefer preserving instruction fidelity over exhaustive catalogs.
- Do not silently corrupt required fragments when over budget.
- Reuse typed fragment metadata from `tsq-tr165sfj.1` and registration behavior from `tsq-tr165sfj.2`.

## Acceptance
- Skills/tool catalogs can compact before required instructions are touched.
- Context-file truncation is explicit and test-covered.
- Overflow cases return diagnostics the caller can surface or inspect.