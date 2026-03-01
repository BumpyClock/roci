# Goal
Match pi-mono semantics by applying `transform_context` before `convert_to_llm`.

# Scope
- runner ordering in LLM phase
- hook signatures as needed for cancellation/token context
- tests for transformed context input/output invariants

# Acceptance Criteria
- `transform_context` executes before message conversion.
- Converted payload reflects transformed context exactly.
- Cancellation behavior remains correct during transform stage.
- Regression tests updated for ordering-sensitive behavior.
