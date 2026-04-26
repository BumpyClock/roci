# Make transport preference effective in providers

## Objective
Ensure `ProviderRequest.transport` changes runtime behavior, not just plumbing.

## Acceptance Criteria
1. Define transport contract (allowed values + semantics).
2. Implement at least one meaningful transport branch OR explicit unsupported-value error.
3. Add provider + runner tests proving behavior change.
4. Update docs/config examples.
