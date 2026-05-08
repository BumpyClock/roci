## Overview

Replace the runtime `model + fallback_chain` representation with a single ordered `candidates` abstraction on `AgentConfig` and `RunRequest`. This workstream owns that unified ordered runtime candidate abstraction for G3. Define the ownership boundary with `tsq-tb6qdmqm.9` so launch-time profile resolution ends at ordered candidate construction and runtime candidate behavior begins only after that point.

## Decided context

- This workstream owns the runtime `candidates` abstraction consumed by the rest of G3.
- `tsq-tb6qdmqm.9` owns launch-time `profile.models` loading/resolution and viability filtering.
- G3 runtime layers consume the ordered candidate list after launch-time resolution and never reconstruct `model + fallback_chain`.

## Constraints / Non-goals

- Breaking `AgentConfig` / `RunRequest` changes are acceptable and preferred in G3 when they remove compatibility shims.
- Runtime code consumes normalized `LanguageModel` candidates only, not profile-specific metadata.
- This task defines ordered runtime candidate identity and ordering semantics for later retry/health workstreams.
- `tsq-tb6qdmqm.9` continues to own launch-time `profile.models` loading/resolution and viability filtering.
- No runtime retry / candidate-advance state machine here.
- No provider/model health tracking here.
- No remote provider probing or launch-time viability work beyond `tsq-tb6qdmqm.9`.

## Interfaces (CLI/API)

```rust
pub struct AgentConfig {
    pub candidates: Vec<LanguageModel>,
    // existing non-model fields preserved
}

pub struct RunRequest {
    pub candidates: Vec<LanguageModel>,
    // existing non-model fields preserved
}
```

Contract:
- `candidates[0]` is always tried first.
- `candidates[i + 1]` is the next candidate after candidate `i` becomes eligible for advancement.
- Duplicates are removed while preserving first-seen order.
- The normalized candidate list is never empty.
- Explicit empty `candidates` returns `RociError::Configuration` before provider creation.
- Migration constructors from old single `model` produce `candidates = [model]`.
- Dedup key is normalized `(provider, model_id)` from `LanguageModel`; first occurrence wins.

Sub-agent mapping contract:

```text
resolved profile.models
-> ordered viable candidates
-> child AgentConfig.candidates = ordered viable candidates
```

Child inheritance contract:
- child runtimes inherit retry defaults from supervisor base config
- child runtimes inherit health defaults from supervisor base config
- only ordered candidate selection changes per resolved profile

## Data model / schema changes

- Replace runtime `model` / `fallback_chain` fields with `candidates: Vec<LanguageModel>`.
- Normalize candidate lists once at runtime config construction and reuse the ordered list unchanged for the life of the run.
- Treat single-model callers as `candidates = [model]` during migration instead of maintaining a compatibility bridge.
- Pass resolved `profile.models` directly into child runtime config with no pairwise primary/fallback translation.

## Boundary with `tsq-tb6qdmqm.9`

`tsq-tb6qdmqm.9` owns:
- loading built-in + TOML profiles
- resolving inheritance/overrides
- producing ordered viable `profile.models` at launch time

`tsq-g6ba4ega.1` owns:
- representing runtime candidates on `AgentConfig` / `RunRequest`
- normalizing and deduping ordered candidate lists
- ensuring runtime layers do not need to inspect profile TOML after config construction
- avoiding an adapter layer that reconstructs `model + fallback_chain`

## Acceptance Criteria

1. `AgentConfig` and `RunRequest` use the shared ordered `candidates` abstraction defined by this workstream.
2. Existing single-model callers can be migrated directly to `candidates = [model]` with no runtime compatibility shim requirement.
3. Resolved sub-agent `profile.models` map deterministically to child `candidates` with no pairwise primary/fallback translation.
4. The normalized candidate list is never empty and never re-ordered after runtime construction.
5. Runtime layers no longer depend on profile-resolution types after `AgentConfig` is built.
6. Child inheritance of retry/health defaults remains explicit.

## Test plan

- Validation tests for candidate normalization, dedupe, and non-empty enforcement.
- Mapping tests proving resolved `profile.models` become child `candidates` without adapter logic.
- Regression tests covering single-model migration to `candidates = [model]`.
- Boundary tests proving runtime layers do not depend on profile-resolution types after config construction.

## Risks / open questions

- If future runtime behavior needs per-candidate metadata beyond `LanguageModel`, add a runtime-owned metadata shape rather than leaking profile-owned types.
- Breaking the old `model` / `fallback_chain` surface touches host call sites, so rollout should happen as a single coordinated refactor rather than a compatibility bridge.
