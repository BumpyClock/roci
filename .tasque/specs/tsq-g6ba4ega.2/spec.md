## Overview

Add explicit retry modes with persistent retry support and structured retry heartbeat callbacks. The runtime/supervisor layer defines the default retry mode for descendant runs, and an individual run may override that default explicitly. Runtime must retry the same ordered candidate first, keep overflow compaction as the first overflow recovery path, and only advance to the next candidate after eligible retry behavior is exhausted and only when no partial streamed output or tool deltas have been emitted.

## Constraints / Non-goals

- Retry / candidate-advance control flow must operate on the ordered `candidates` abstraction from `tsq-g6ba4ega.1`.
- Overflow compaction remains the first recovery path and does not auto-advance in G3 default.
- No remote probes or persistence.
- No health registry implementation details beyond consuming its contracts.
- No automatic candidate advancement for auth/config/invalid-request/tool/cancel categories.
- No automatic candidate advancement after partial streamed output or tool deltas in G3 default.

## Interfaces (CLI/API)

```rust
pub enum RetryMode {
    Bounded { max_attempts: u32 },
    Persistent,
}

pub struct RetryHeartbeat {
    pub run_id: String,
    pub provider: String,
    pub model_id: String,
    pub candidate_index: usize,
    pub attempt: u32,
    pub retry_mode: RetryMode,
    pub failure_category: FailureCategory,
    pub sleep_ms: u64,
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

Heartbeat behavior:
- emit when a retry wait is scheduled
- emit periodically during long or persistent waits
- emit when retry resumes, is canceled, or exhausts into candidate advancement
- callback failures do not fail the run

## Data model / schema changes

- Rename heartbeat field `fallbacks_remaining` -> `candidates_remaining`.
- Define `candidate_index` as an index into the effective ordered `RunRequest.candidates` list.
- Resolve effective retry mode once per run from runtime/supervisor default plus optional per-run override; the override replaces the inherited mode for that run only.
- Keep retry counters candidate-local; reset them only when advancing to the next ordered candidate.
- Treat candidate advancement as movement to `candidates[i + 1]`, not as a separate primary/fallback abstraction.

Candidate-advance interaction contract:
- advancement requires transient eligible category + retry exhaustion + later candidate available + no partial output/tool deltas
- auth/config/invalid-request/tool/cancel categories must short-circuit without candidate advancement
- overflow compaction runs before generic retry / candidate-advance logic

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
3. Retry waits emit deterministic heartbeat payloads.
4. Cancellation during retry sleep or persistent wait terminates cleanly without candidate advancement.
5. Overflow compaction remains the first recovery path.
6. No candidate advancement occurs for auth/config/invalid-request/tool/cancel categories.
7. No candidate advancement occurs after partial streamed output or tool deltas in G3 default.
8. Candidate advancement resets candidate-local retry state and advances only to the next ordered candidate.

## Test plan

- Runner tests covering retry-before-advance ordering for rate-limit/server/network failures.
- Heartbeat tests for schedule, periodic wait, resume, cancel, and advancement exhaustion events.
- Cancellation tests proving waits remain interruptible and suppress automatic candidate advancement.
- Regression tests proving no candidate advancement after partial output/tool deltas and no bypass of overflow compaction.
- Supervisor/child tests confirming inherited retry defaults apply unless a run explicitly overrides `RetryMode`.

## Risks / open questions

- Persistent retry intentionally makes automatic candidate advancement unreachable unless a future hybrid policy is added.
- Confirm whether heartbeat cadence should be fixed or caller-configurable in G3.
