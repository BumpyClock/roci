# Goal
Add overflow recovery behavior: compact context then auto-continue when token-window overflow is detected.

# Scope
- overflow detection path in runner/LLM phase
- compaction invocation + resume logic
- loop guards to prevent infinite compact/retry cycles

# Acceptance Criteria
- On overflow/too-large prompt error, runner triggers compaction and retries turn automatically.
- If compaction cannot recover, run fails with clear error after bounded attempts.
- Behavior covered by tests for recoverable and unrecoverable overflow scenarios.
- Docs clarify trigger precedence vs pre-LLM compaction threshold.
