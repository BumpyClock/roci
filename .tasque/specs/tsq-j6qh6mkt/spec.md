# Overview

## Problem statement
Roci already has heuristic token estimation, pre-provider auto-compaction, and typed overflow recovery in the agent loop, but the SDK does not yet expose first-class context management primitives. Today the logic is split across `agent_loop/compaction.rs`, `agent/runtime/summary.rs`, and `runner/engine/llm_phase.rs`; it is mostly `agent`-gated, heuristic-only, and hard to reuse cleanly from the SDK surface.

This epic restores the missing G1 root and plans a clean SDK shape for five linked concerns: `TokenCounter`, provider-specific overflow detection, multi-strategy compaction, overflow recovery, and `ContextBudget` enforcement.

## Prior art

### Pi
- `packages/ai/src/utils/overflow.ts`: reusable overflow classifier, including usage-vs-window checks.
- `packages/agent/src/types.ts` + `packages/agent/src/agent-loop.ts`: generic loop hook for context transforms without baking compaction into the low-level loop.
- `packages/coding-agent/src/core/compaction/compaction.ts`: `estimateContextTokens`, `shouldCompact`, cut-point selection, and explicit summary-based compaction.
- `packages/coding-agent/src/core/agent-session.ts`: one-shot compact-and-retry overflow recovery, distinct from retryable transport/server failures.
- Main lesson: keep low-level counting/detection separate from higher-level compaction and session policy.

### Codex CLI / codex-rs
- `codex-rs/core/src/context_manager/*`: server/core-owned context accounting with explicit estimates and history management.
- `codex-rs/core/src/compact.rs` + `compact_remote.rs`: explicit compact operation, plus incremental degradation when compaction itself overflows.
- `codex-rs/*/model_info.rs`: headroom-aware effective context window and auto-compact thresholds derived from model limits.
- Main lesson: keep context management in SDK/core, not in thin clients; use typed overflow failures and explicit headroom budgets.

### Claude Code
- `src/utils/tokens.ts` + `src/services/tokenEstimation.ts`: canonical context sizing based on actual usage plus estimator deltas.
- `src/services/api/errors.ts` + `src/services/api/withRetry.ts`: parse prompt-too-long / max-token overflow separately from generic retries.
- `src/services/compact/{microCompact,compact,autoCompact}.ts`: multi-strategy compaction ladder and circuit breakers.
- `src/query/tokenBudget.ts`: first-class task/token budget tracking distinct from context-window management.
- Main lesson: unify sizing in one canonical snapshot, separate budget policy from recovery policy, and expose read-only context introspection.

## Constraints / Non-goals
- SDK-only. No app/TUI/config/persistence design in this epic beyond integration notes.
- Roci is still in active development with no external SDK users yet; breaking changes are acceptable and preferred over compatibility shims when they produce a cleaner long-term SDK surface.
- Keep `roci-core` provider-agnostic. Provider-specific detectors and exact tokenizers belong in `roci-providers` (or future external plugins), while `roci-core` owns traits/contracts.
- Preserve visible transcript semantics for compaction (explicit synthetic summary message), not hidden reasoning persistence.
- No new services, queues, databases, or app-level storage formats.
- Do not plan session tree, branch persistence, or CLI settings UX here.

## Proposed architecture

### Module boundaries
Create an ungated `roci_core::context` module with four submodules:
- `context::tokens` — `TokenCounter`, `TokenCount`, `CountAccuracy`, heuristic fallback, and helpers for counting messages/conversations.
- `context::overflow` — `OverflowDetector`, `OverflowSignal`, provider classifier registry contract, and typed/text fallback classification.
- `context::budget` — `ContextBudget`, `BudgetSnapshot`, `BudgetDecision`, and session accumulation helpers.
- `context::compaction` — `CompactionStrategy`, `CompactionRequest`, `CompactionResult`, `SummaryArtifact`, and reusable cut/suffix helpers.

Runtime-specific orchestration stays where it is:
- `agent/runtime/*` keeps runtime wiring and hooks.
- `agent_loop/runner/*` keeps retry/recovery orchestration.
- `roci-providers` supplies built-in provider overflow classifiers and exact token counters where available.

Prefer direct moves/renames into this module over maintaining duplicate old/new APIs. Re-export shims should only exist when they materially reduce implementation risk during the transition.

### Interfaces (CLI/API)
- `TokenCounter` trait with pluggable implementations. Default SDK implementation is heuristic; exact provider/model counters are opt-in plugins.
- `OverflowDetector` trait/classifier that accepts `(provider_name, model, error)` and returns `OverflowSignal` when the error is a context overflow.
- `ContextBudget` value object used to compute a `BudgetSnapshot` before each provider call and after each response.
- `CompactionStrategy` async contract returning a typed `CompactionResult` instead of ad hoc message replacement.
- `OverflowRecoveryPolicy` used by the runner to apply a deterministic ladder: shrink output budget -> cheap compaction -> summary compaction -> fail.
- `ContextUsageSnapshot` read-only SDK value for callers/tests/hooks to inspect estimated vs actual usage.

### Data model / schema changes
Add the following SDK-visible types in `roci-core`:
- `TokenCount { tokens, accuracy, source }`
- `ContextUsageSnapshot { estimated_input_tokens, actual_input_tokens, actual_output_tokens, context_window, reserved_output_tokens, remaining_input_tokens, usage_percent }`
- `ContextBudget { context_window_override?, reserve_output_tokens, max_turn_input_tokens?, max_session_input_tokens?, max_session_output_tokens? }`
- `BudgetDecision { Proceed, Compact { target_tokens }, ReduceMaxTokens { new_max_tokens }, Reject { reason } }`
- `OverflowSignal { typed_code, provider_code, matched_text_pattern, retry_hint }`
- `CompactionResult { strategy, messages, estimated_tokens_removed, preserved_suffix_tokens, summary_artifact? }`
- `SummaryArtifact { strategy, summary_text, covered_message_range }`

Integration notes (not app design):
- `AgentConfig` / `RunRequest` gain optional context policy hooks/objects, but CLI/config plumbing stays out of scope for this epic.
- `Usage` remains the carrier for provider-reported counts; `ContextUsageSnapshot` reconciles `Usage` with `TokenCounter` estimates.

## Compaction strategy ladder
1. **Micro** — remove non-essential high-cost payloads first (images, hidden/thinking content, oversized transient payloads that can be safely omitted).
2. **Snip** — truncate oversized tool results / tool output bodies with explicit markers while preserving tool identity and error semantics.
3. **Summary** — summarize the oldest compactable prefix, preserve the newest suffix, and preserve the active user turn prefix if the cut falls mid-turn.
4. **Fail** — if the ladder cannot get under budget, return a typed overflow failure instead of looping forever.

Each stage must be idempotent, report what changed, and stop once the target budget is met.

## Retry / recovery flow
1. Build `BudgetSnapshot` from model capabilities, `ContextBudget`, last known `Usage`, and `TokenCounter` estimates.
2. If preflight snapshot exceeds soft limit, proactively run the compaction ladder before the provider call.
3. On provider error, run `OverflowDetector`.
4. If overflow is confirmed:
   - first reduce `GenerationSettings.max_tokens` when a safe smaller output budget is available,
   - then retry once with updated output budget,
   - if still over budget, run micro/snip/summary compaction in order,
   - retry after each successful reduction/compaction,
   - stop after dedicated overflow-attempt limits, independent from generic retry/backoff limits.
5. Generic rate-limit/network/server errors keep using `RetryBackoffPolicy`; overflow recovery is a separate lane.

## Acceptance criteria
- `roci-core` exposes an ungated `context` module with public docs/tests for tokens, overflow, budget, and compaction primitives.
- `TokenCounter` supports at least heuristic fallback everywhere and exact counting for built-in providers where a local tokenizer is available.
- Built-in providers can classify both typed and known untyped overflow responses into `ContextLengthExceeded`.
- Compaction is strategy-based, preserves the newest suffix/current turn, and reports typed results.
- The runner uses a separate overflow recovery policy from generic retry backoff.
- `ContextBudget` can enforce per-turn and per-session token budgets without requiring CLI config work.
- Implementation prefers the clean target API over compatibility layers; temporary shims are not required for v1.
- Tests cover typed overflow, untyped overflow, proactive compaction, reactive recovery, and budget exhaustion.

## Test plan
- Unit tests for `TokenCounter` heuristics/exact counters and `ContextUsageSnapshot` math.
- Matrix tests for provider overflow classifiers (typed + text-only samples).
- Unit tests for compaction planning: cut-point selection, suffix preservation, idempotent micro/snip stages.
- Runner tests for recovery ladder ordering and separate overflow-attempt limits.
- Runtime tests for per-turn/per-session budget enforcement and context introspection snapshots.
- Regression tests ensuring visible compaction summary semantics remain stable.

## Rollout order
1. Restore G1 epic root and create `roci_core::context` boundary.
2. Land `TokenCounter` + `ContextUsageSnapshot` + `ContextBudget` core types.
3. Land overflow classifier contracts and built-in provider mappings.
4. Land typed compaction strategy ladder.
5. Wire overflow recovery policy into the runner.
6. Add end-to-end SDK tests and documentation.

## Open questions
- Resolved: exact tokenizers ship behind optional Cargo features, with heuristic fallback always available.
- Should built-in overflow text classifiers live only in `roci-providers`, or should `roci-core` also ship a small provider-name keyed fallback table for common built-ins?
- Resolved: v1 enforces separate input and output budgets rather than a single aggregate token bucket.
- What should the default overflow recovery cap be (recommended starting point: 1 output-budget reduction + 2 compaction retries)?