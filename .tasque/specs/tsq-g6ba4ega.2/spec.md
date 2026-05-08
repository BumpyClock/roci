## Overview

Add explicit retry modes with persistent retry support and structured retry events. The runtime/supervisor layer defines the default retry mode for descendant runs, and an individual run may override that default explicitly. Runtime must retry the same ordered candidate first, keep overflow compaction as the first overflow recovery path, and only advance to the next candidate after eligible retry behavior is exhausted and only when no partial streamed output or tool deltas have been emitted.

## Constraints / Non-goals

- Retry / candidate-advance control flow must operate on the ordered `candidates` abstraction from `tsq-g6ba4ega.1`.
- Overflow compaction remains the first recovery path and does not auto-advance in G3 default.
- No remote probes or persistence.
- No health registry implementation details beyond consuming its contracts.
- No automatic candidate advancement for auth/config/invalid-request/tool/cancel categories.
- No automatic candidate advancement after partial streamed output or tool deltas in G3 default.
- No model heartbeat/ping calls. Retry observability is runtime events only and must not spend provider tokens or consume provider rate limits.

## Interfaces (CLI/API)

```rust
pub enum RetryMode {
    Bounded { max_attempts: u32 },
    Persistent,
}

pub enum RetryEventKind {
    RetryScheduled,
    RetryResuming,
    RetryCanceled,
    CandidateAdvancing,
    RetryExhausted,
}

pub struct RetryEvent {
    pub kind: RetryEventKind,
    pub run_id: String,
    pub provider: String,
    pub model_id: String,
    pub candidate_index: usize,
    pub attempt: u32,
    pub retry_mode: RetryMode,
    pub failure_category: FailureCategory,
    pub sleep_ms: Option<u64>,
    pub elapsed_retry_ms: u64,
    pub candidates_remaining: usize,
    pub partial_output_seen: bool,
    pub next_action: RetryNextAction,
}
```

Retry scope contract:
- runtime/supervisor config provides the default `RetryMode` for descendant runs
- an individual run may replace that default with an explicit per-run override before execution
- once the run starts, retry / candidate-advance decisions use the effective `RetryMode` for the life of that run
- `RetryMode::Bounded { max_attempts }` counts total provider attempts per candidate, including the initial attempt
- `max_attempts` must be >= 1; `1` disables same-candidate retry
- retry counters are candidate-local and reset only after candidate advance

Retry event behavior:
- emit `RetryScheduled` before an interruptible retry sleep
- emit `RetryResuming` when execution resumes after retry sleep
- emit `RetryCanceled` when cancellation interrupts retry wait
- emit `CandidateAdvancing` when eligible retry exhaustion advances to `candidates[i + 1]`
- emit `RetryExhausted` when retry behavior is exhausted and no candidate remains or advancement is disallowed
- do not emit periodic cadence events in v1
- callback failures do not fail the run

## Data model / schema changes

- Rename old heartbeat language to `RetryEvent`; if any existing field used `fallbacks_remaining`, rename it to `candidates_remaining`.
- Define `candidate_index` as an index into the effective ordered `RunRequest.candidates` list.
- Resolve effective retry mode once per run from runtime/supervisor default plus optional per-run override; the override replaces the inherited mode for that run only.
- Keep retry counters candidate-local; reset them only when advancing to the next ordered candidate.
- Treat candidate advancement as movement to `candidates[i + 1]`, not as a separate primary/fallback abstraction.
- If only one candidate is configured, retry that candidate according to the effective retry mode; when exhausted, return the classified/original failure without candidate-advance behavior.

Candidate-advance interaction contract:
- advancement requires transient eligible category + retry exhaustion + later candidate available + no partial output/tool deltas
- auth/config/invalid-request/tool/cancel categories must short-circuit without candidate advancement
- overflow compaction runs before generic retry / candidate-advance logic
- persistent retry never advances candidates; it continues retrying the current candidate until canceled

Cancellation contract:
- cancel during retry sleep stops the run cleanly
- cancel suppresses automatic candidate advancement
- persistent retry remains interruptible at all wait points

## Dependency notes

- Starts after `tsq-g6ba4ega.1` because it needs stable ordered runtime candidate identity.
- Consumes health / candidate-advance signals from `tsq-g6ba4ega.3`, but remains the owner of retry / candidate-advance control flow.

## Acceptance Criteria

1. Same-candidate retry happens before candidate advancement for eligible transient failures.
2. Runtime/supervisor retry defaults inherit deterministically, explicit per-run overrides replace them for that run only, and persistent retry remains explicit and observable.
3. Retry waits emit deterministic `RetryEvent` payloads without provider heartbeat/ping calls.
4. Cancellation during retry sleep or persistent wait terminates cleanly without candidate advancement.
5. Overflow compaction remains the first recovery path.
6. No candidate advancement occurs for auth/config/invalid-request/tool/cancel categories.
7. No candidate advancement occurs after partial streamed output or tool deltas in G3 default.
8. Candidate advancement resets candidate-local retry state and advances only to the next ordered candidate.
9. Single-candidate runs return the final failure after retry exhaustion instead of fabricating fallback behavior.

## Test plan

- Runner tests covering retry-before-advance ordering for rate-limit/server/network failures.
- Retry event tests for schedule, resume, cancel, candidate advancement, exhaustion, and absence of periodic cadence events.
- Cancellation tests proving waits remain interruptible and suppress automatic candidate advancement.
- Regression tests proving no candidate advancement after partial output/tool deltas and no bypass of overflow compaction.
- Supervisor/child tests confirming inherited retry defaults apply unless a run explicitly overrides `RetryMode`.
- Single-candidate tests proving retry exhaustion returns the final classified failure.

## Risks / open questions

- Persistent retry intentionally makes automatic candidate advancement unreachable unless a future hybrid policy is added.
- Future UI may want periodic "still waiting" updates during long persistent sleeps; G3 intentionally omits cadence events until a real host needs them.
