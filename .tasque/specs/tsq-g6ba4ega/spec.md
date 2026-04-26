## Overview

G3 adds runtime model failover and resilience to `roci` around a single ordered runtime candidate abstraction. Persistent retry is configured as a runtime/supervisor default and may be overridden per run. Roci is in active development with no external SDK users, so breaking `AgentConfig` / `RunRequest` cleanup is acceptable and preferred when it simplifies long-term candidate failover/retry/health semantics.

## Decided context

- Runtime model selection uses one ordered `candidates` list on `AgentConfig` and `RunRequest`; runtime code does not reconstruct `model + fallback_chain`.
- Persistent retry scope lives at the runtime/supervisor layer by default, with explicit per-run override when a run needs different behavior.
- `tsq-tb6qdmqm.9` stops at launch-time profile loading/resolution and ordered candidate construction; G3 begins at runtime candidate failover/retry/health after `AgentConfig` is built.

Boundary with `tsq-tb6qdmqm.9`:
- `tsq-tb6qdmqm.9` owns launch-time `profile.models` loading/resolution and viability filtering.
- G3 owns runtime candidate retry / advance / health once an `AgentConfig` has been built.
- Child runtimes receive the same ordered candidate list directly from resolved `profile.models` and inherit retry/health defaults from supervisor base config unless a run explicitly overrides retry mode.

## Assumptions

- Breaking SDK changes are acceptable and preferred when they reduce long-term API drift.
- Roci currently has no external SDK consumers, so G3 should optimize for the cleanest long-term runtime model-selection contract.
- Existing error classification remains the main input to retry / candidate-advance policy.
- Cross-provider candidate advancement uses already-resolved credentials/config and does not add new remote discovery.

## Constraints / Non-goals

- Prefer a single ordered `candidates` abstraction over runtime compatibility layering around `model + fallback_chain`.
- Retry mode defaults are owned by runtime/supervisor config; per-run override changes only the effective run policy.
- Retry / fallback / health logic must operate on ordered candidates as the core abstraction.
- Keep the existing failure taxonomy choices unless a choice depended on compatibility assumptions.
- Keep overflow compaction as the first recovery lane; G3 does not replace it.
- Do not add remote probes, daemons, or disk-backed health state.
- Do not move launch-time profile loading/resolution out of `tsq-tb6qdmqm.9`.
- Provider modules and `subagents/profiles` must not own runtime candidate-advance policy.

## Interfaces (CLI/API)

```rust
pub struct AgentConfig {
    pub candidates: Vec<LanguageModel>,
    pub retry_mode: RetryMode,
    pub retry_backoff: RetryBackoffPolicy,
    pub max_retry_delay_ms: Option<u64>,
    pub retry_heartbeat: Option<RetryHeartbeatFn>,
    pub health_registry: Option<std::sync::Arc<HealthRegistry>>,
}

pub struct RunRequest {
    pub candidates: Vec<LanguageModel>,
    pub retry_mode: RetryMode,
    pub retry_backoff: RetryBackoffPolicy,
    pub max_retry_delay_ms: Option<u64>,
    pub retry_heartbeat: Option<RetryHeartbeatFn>,
    pub health_registry: Option<std::sync::Arc<HealthRegistry>>,
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

pub type RetryHeartbeatFn = std::sync::Arc<dyn Fn(&RetryHeartbeat) + Send + Sync>;
```

Retry scope contract:
- runtime/supervisor config establishes the default `RetryMode` for descendant runs
- an individual run may override that default before execution without mutating the supervisor baseline
- retry / heartbeat / health logic reads the run's effective ordered `candidates` list and effective `retry_mode`

Typed host-facing events should align with ordered candidates:
- `run.retry_scheduled`
- `run.retry_heartbeat`
- `run.candidate_advanced`
- `run.candidate_advance_suppressed`
- `run.health_updated`

## Data model / schema changes

- Replace runtime `model` + `fallback_chain` fields with `candidates: Vec<LanguageModel>` on `AgentConfig` and `RunRequest`.
- Validate `candidates` as non-empty, normalize once, and deduplicate while preserving first-seen order.
- Treat request-level candidate overrides as whole-list replacement, not per-field merging.
- Resolve each run's effective retry mode from runtime/supervisor default plus optional per-run override before execution starts; the override replaces the inherited mode for that run only.
- Rename retry heartbeat field `fallbacks_remaining` -> `candidates_remaining`.
- Record health candidate movement with a `CandidateAdvanced` signal instead of binary primary/fallback terminology.
- Pass resolved `profile.models` directly into child `AgentConfig.candidates` with inherited retry/health defaults and no adapter layer.

## Current baseline in roci

- `AgentConfig` has a single `model` and no runtime ordered candidate list.
- Retry settings are bounded today via `retry_backoff` and `max_retry_delay_ms`.
- Provider instances are created once per run.
- Retry reporting is currently unstructured string output.
- Overflow compaction already exists as a dedicated first recovery lane.
- Sub-agent `profile.models` are launch-time only and do not drive mid-run failover today.

## ADR 1: Runtime candidate representation

### Context
Roci already has launch-time ordered candidates in sub-agent profiles. Keeping a separate runtime `model + fallback_chain` shape would preserve compatibility with an SDK surface that has no external consumers, but it would also force runtime failover/retry/health logic to bridge between two parallel abstractions.

### Decision
Adopt a single ordered runtime candidate abstraction on both `AgentConfig` and `RunRequest`:
- `candidates: Vec<LanguageModel>` is the only runtime model-selection field.
- The list is validated as non-empty, normalized, and deduplicated while preserving first-seen order.
- `candidates[0]` is the initial selection; later workstreams decide when advancing to `candidates[i + 1]` is allowed.
- Child runtimes receive the ordered candidates directly from resolved `profile.models` with no `model + fallback_chain` adapter layer.
- Breaking `AgentConfig` / `RunRequest` changes are explicitly in scope for G3 if that yields the cleaner long-term contract.

### Consequences
- one model-selection abstraction across root runs and sub-agent runs
- retry, failover, and health logic can operate on stable candidate indices
- host call sites must migrate once, but avoid long-lived compatibility shims
- future per-candidate metadata can be added as runtime-owned data without re-introducing a primary+fallback split

## ADR 2: Retry-before-advance policy

### Context
Immediate candidate advancement can hide transient provider issues and makes behavior harder to reason about. Roci already has bounded retry and a separate overflow compaction lane.

### Decision
Persistent retry is configured at runtime/supervisor scope by default and may be overridden per run. Retry the same candidate first. Advance only after the retry path is exhausted for transient capacity/server/network failures that occur before any streamed output or tool delta is emitted. Keep overflow compaction as the first recovery path. Do not auto-advance for auth/config/invalid-request/tool/cancel categories.

### Consequences
- behavior stays deterministic and aligned with configured candidate order
- supervisors can set one default retry posture while individual runs retain an explicit escape hatch
- transient failures may take longer to recover than immediate failover
- persistent retry mode must remain explicit and observable

## ADR 3: Two-tier in-process health tracking

### Context
Per-run state is needed to avoid repeated bad choices in one run, while shared in-process memory helps later runs observe recent degradation without adding infrastructure.

### Decision
Track health at `provider+model` granularity using session-local counters plus a shared in-process global snapshot. Health is advisory in G3 default; it does not reorder the configured candidate list or pre-skip candidate `0` at run start.

### Consequences
- better observability with minimal operational complexity
- requires clear merge rules between local and global views
- avoids remote probes, background daemons, and disk persistence

## Failure taxonomy

| Category | Examples | Retry same candidate? | Advance to next candidate after retries? | Must not auto-advance? | Notes |
|---|---|---:|---:|---:|---|
| Context overflow | typed context overflow, prompt too large | Yes, through existing compaction lane | No in G3 default | Yes | Compaction remains first recovery path |
| Rate limit / capacity | `429`, provider busy | Yes | Yes, if no partial output | No | Count as transient health failure |
| Server | `5xx`, service unavailable | Yes | Yes, if no partial output | No | Count as transient health failure |
| Network / timeout | connect/read timeout, transport reset before deltas | Yes | Yes, if no partial output | No | Count as transient health failure |
| Auth | missing/invalid credential, `401/403` | No | No | Yes | Stop immediately |
| Config / model wiring | bad provider config, model not found | No | No | Yes | Stop immediately |
| Invalid request | malformed prompt/tool schema, non-overflow `400` | No | No | Yes | Stop immediately |
| Tool failure | tool execution/runtime error | No | No | Yes | Tool layer owns recovery |
| Cancel | user/supervisor cancellation | No | No | Yes | Cancellation wins over retry/advancement |
| Mid-stream failure after partial text/tool delta | stream error after visible output | No auto-retry in G3 default | No auto-advance in G3 default | Yes | Avoid duplicated visible output or tool effects |

Trigger rules:
- honor provider retry hints when present, still bounded by local policy caps
- overflow stays isolated from generic retry / candidate-advance logic
- candidate advancement eligibility requires: transient category, retry path exhausted, later candidate available, and no emitted text/tool deltas

## Module boundaries

### Owned in G3
- runtime config surface for ordered `candidates` plus runtime/supervisor retry defaults
- runner logic for retry / candidate-advance orchestration
- new in-process health module for provider+model observations
- sub-agent launcher/supervisor wiring that passes resolved candidates directly into child runtime config and propagates retry/health defaults unless a run explicitly overrides retry mode

### Explicitly out of scope for G3
- provider modules owning candidate-advance policy or health orchestration
- `subagents/profiles` owning mid-run retry / candidate-advance behavior
- runtime layers depending on profile-owned metadata after `AgentConfig` construction
- remote probes, daemons, or disk-backed health services

## Workstreams

### `tsq-g6ba4ega.1` — unified ordered runtime candidate abstraction
- own the shared ordered `candidates` abstraction on `AgentConfig` / `RunRequest`
- replace primary+fallback layering with shared ordered `candidates`
- normalize candidate order and dedupe once
- define boundary with `tsq-tb6qdmqm.9`
- map resolved `profile.models` directly into child `candidates`

### `tsq-g6ba4ega.2` — retry defaults, per-run override, and retry heartbeat callbacks
- define runtime/supervisor retry defaults plus per-run override semantics
- retry same candidate first for eligible transient failures
- gate candidate advancement on retry exhaustion, eligibility, and no partial output/tool deltas
- keep cancellation authoritative during retry sleeps and persistent waits

### `tsq-g6ba4ega.3` — provider/model health registry over ordered candidates
- implement session-local tracker plus shared in-process global snapshot
- record success/transient failure/candidate exhaustion/candidate-advance signals across ordered candidate indices
- expose advisory health views without reordering configured candidates

Integration thread across all workstreams:
- switch provider acquisition from once-per-run to once-per-active-candidate
- emit structured retry / candidate-advance / health events
- add regression coverage for supervisor-child inheritance and runtime failover rules

## Acceptance Criteria

1. `AgentConfig` and `RunRequest` use a shared ordered `candidates` abstraction with no runtime `model + fallback_chain` compatibility layering.
2. Retry policy supports bounded and persistent modes, with runtime/supervisor defaults and explicit per-run override; persistent retry remains explicit and observable.
3. Same-candidate retry happens before candidate advancement for transient capacity/server/network failures.
4. Overflow compaction remains the first recovery path and does not auto-advance in G3 default.
5. Auth/config/invalid-request/tool/cancel categories never trigger candidate advancement.
6. No automatic candidate advancement occurs after partial streamed output or tool deltas in G3 default.
7. Health is tracked at `provider+model` granularity with session-local counters plus a shared in-process global snapshot.
8. No remote probes, daemons, or disk persistence are introduced.
9. Child runtimes receive ordered `candidates` directly from resolved `profile.models` and inherit retry/health defaults from supervisor base config unless an explicit per-run retry override is supplied.
10. Structured retry / candidate-advance / health events exist for hosts and tests.

## Test plan

- Runner tests for same-candidate retry before advancement across rate-limit/server/network failures.
- Regression tests proving no auto-advance after partial streamed output or tool deltas.
- Tests that overflow compaction remains the first recovery lane and stays isolated from generic candidate advancement.
- Health tests covering session-local + shared in-process snapshots and `CandidateAdvanced` signaling.
- Supervisor/child tests confirming resolved `profile.models` map directly to child `candidates`, inherited retry defaults are applied, and explicit per-run overrides win without violating the `tsq-tb6qdmqm.9` boundary.

## Risks and mitigations

| Risk | Why it matters | Mitigation |
|---|---|---|
| Cross-provider credential ambiguity | candidate advancement may pick a provider without valid runtime credentials | keep resolution scoped to already-configured providers; surface credential failures as non-advancing auth/config errors |
| Duplicate output after partial stream failure | retry/advancement could duplicate visible text or tool side effects | hard-stop auto advancement after any emitted text/tool delta in G3 default |
| Health poisoning from non-transient errors | one bad request could incorrectly mark a model unhealthy | count only transient retry/advance-eligible failures toward degraded health |
| Hidden infinite waits | persistent retry can stall without visibility | require explicit persistent mode plus heartbeat events/callbacks |
| Nondeterministic health-driven behavior | global state could implicitly reorder candidates | keep configured order authoritative; treat health as advisory only |

## Open questions

1. Should provider-specific credential lookup stay as-is and rely on already-resolved config, or become more explicitly provider-aware for cross-provider candidate advancement?
2. Should persistent retry require a configured heartbeat sink, or only recommend one?
3. Should the global health snapshot remain purely advisory in G3 default, or ever pre-skip candidate `0` in a future opt-in mode?
4. Should run snapshots/results expose the final active candidate and candidate-advance history directly, or keep that information event-only for now?
