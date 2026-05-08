## Overview

Add an in-memory provider/model health registry that records runtime observations over the shared ordered candidate abstraction and exposes session-local plus shared in-process snapshots. The registry must not perform remote probes, run daemons, or persist to disk. It provides standardized health and candidate-advance signals to the runtime, but does not own retry / candidate-advance state transitions.

## Constraints / Non-goals

- Health granularity is exactly `provider + model`.
- Health is advisory only in G3 default and must not reorder the configured candidate list.
- Candidate indices and advance signals always refer to positions in the effective ordered `RunRequest.candidates` list from `tsq-g6ba4ega.1`.
- Only real runtime observations feed health state.
- No remote probes or active health checks.
- No background workers, daemons, or disk persistence.
- No launch-time provider/model resolution.
- No automatic candidate-list reordering in G3 default.

## Interfaces (CLI/API)

```rust
pub struct ModelHealthKey {
    pub provider: String,
    pub model_id: String,
}

pub enum HealthSignal {
    Success,
    TransientFailure { category: FailureCategory },
    NonRetryableFailure { category: FailureCategory },
    RetryExhausted {
        candidate_index: usize,
        key: ModelHealthKey,
        category: FailureCategory,
    },
    CandidateAdvanced {
        from_index: usize,
        to_index: usize,
        from: ModelHealthKey,
        to: ModelHealthKey,
        reason: FailureCategory,
    },
    Canceled,
}

pub struct ModelHealthSnapshot {
    pub key: ModelHealthKey,
    pub status: ModelHealthStatus,
    pub consecutive_transient_failures: u32,
    pub last_failure_category: Option<FailureCategory>,
    pub last_failure_at_ms: Option<u64>,
    pub last_success_at_ms: Option<u64>,
}
```

Derived state guidance:
- `unknown`: no observations yet
- `healthy`: latest meaningful signal was success
- `degraded`: one or two consecutive transient failures
- `unhealthy`: repeated transient failures or recent candidate exhaustion for the same key

Health status rules:
- no observations -> `Unknown`
- `Success` -> `Healthy` and resets transient count
- 1-2 consecutive transient failures -> `Degraded`
- >=3 consecutive transient failures or transient `RetryExhausted` -> `Unhealthy`
- non-retryable failures and cancellation are recorded but do not degrade provider/model health

## Data model / schema changes

- Record candidate movement with `HealthSignal::CandidateAdvanced` instead of binary primary/fallback terminology.
- Record retry exhaustion with `HealthSignal::RetryExhausted { candidate_index, key, category }`; runtime emits it when retry control flow exhausts retry for a candidate.
- Include source/target candidate indices so health observations line up with the shared ordered candidate abstraction.
- Keep session-local mutable state plus shared in-process global snapshots keyed by `provider + model`.
- Treat recent signal history as observational data only; the registry does not trigger candidate advancement itself.
- Shared snapshot merge is last-observation-wins by observation timestamp.
- Current-run session-local state overlays the shared global snapshot for reads in the same run.

Candidate-advance signal contract:
- runtime writes `CandidateAdvanced` when advancing from one ordered candidate to the next
- registry exposes snapshots and recent signal history but does not trigger candidate advancement itself

## Dependency notes

- Starts after `tsq-g6ba4ega.1` because it depends on stable ordered runtime candidate identity.
- Works alongside `tsq-g6ba4ega.2`; `.2` owns retry / candidate-advance transitions while `.3` owns shared health state and observation APIs.
- `tsq-tb6qdmqm.9` remains out of scope except as the producer of launch-time resolved candidates.

## Acceptance Criteria

1. Runtime can record health observations keyed by provider and model.
2. Each run has a session-local tracker.
3. Later runs in the same process can consult a shared in-process global snapshot.
4. No network calls, daemons, or disk writes are introduced.
5. Successful runs improve/reset degraded state for the same key.
6. Candidate advancement over the ordered candidate list is captured as an observable signal with source and target identity.
7. Tests can deterministically validate session-local and global behavior.

## Test plan

- Unit tests for session-local tracker updates across success, transient failure, non-retryable failure, candidate advancement, and cancellation.
- Snapshot tests covering merge behavior between session-local and shared in-process state.
- Regression tests proving the registry never reorders candidates or performs remote probing/persistence.
- Interface tests ensuring `CandidateAdvanced` signals preserve ordered candidate identity.

## Risks / open questions

- Shared in-process state can become stale across long-lived processes, but restart naturally clears it.
- Confirm whether hosts need direct read access to signal history or only aggregated snapshots in G3.
