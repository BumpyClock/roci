# Tool Execution + Validation Tests

## Goal
Validate tool invocation paths and pre-execution argument validation failures.

## Scope
- Parallel/sequential tool success/failure paths.
- JSON-schema validation failures return structured errors.
- Ensure invalid args do not invoke tool handler.

## Acceptance Criteria
- Coverage includes invalid types, missing required fields, and unexpected fields.
- Tool-result and event payloads remain stable.
