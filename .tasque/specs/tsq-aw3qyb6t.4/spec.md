# Add AgentRuntime mutability APIs

## Objective
Provide safe runtime mutators for long-lived orchestration.

## Acceptance Criteria
1. Add setters/replacers for system prompt, tools, conversation messages (and model if supported).
2. Define and test behavior when run is active.
3. Snapshot/state watchers reflect updates deterministically.
4. Public API contract and thread-safety semantics documented.
