# Implement message conversion integration in runner/runtime

## Objective
Implement approved conversion design.

## Acceptance Criteria
1. Conversion hook/path supports filtering/transforming non-LLM messages before provider call.
2. Existing behavior unchanged when hook is unset.
3. Tests validate conversion correctness, skipped messages, and tool-loop stability.
4. Docs/examples updated.
