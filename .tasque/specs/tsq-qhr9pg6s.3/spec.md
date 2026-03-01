# Goal
Implement robust transient error retry behavior with bounded exponential backoff and lifecycle visibility.

# Scope
- provider call failure classification for transient errors
- retry policy (max attempts, jitter/backoff, cancellation-aware)
- retry lifecycle events/telemetry

# Acceptance Criteria
- Retries occur for retryable transport/provider failures beyond explicit rate-limit path.
- Backoff policy is configurable and bounded.
- Cancellation interrupts retry sleep promptly.
- Unit/integration tests cover retryable vs non-retryable cases and max-attempt exhaustion behavior.

# Non-Goals
- Provider-specific bespoke retry heuristics for every backend in first pass.
