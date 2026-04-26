# Skill prompt format + injection hooks

## Scope
- Format skills into system prompt block (pi-mono style XML):
  - include name, description, location.
  - exclude skills with `disable_model_invocation=true`.
- Add injection hook in agent loop / system prompt builder.
- Ensure skills list is appended after base system prompt and before user content.

## Acceptance
- Unit test: formatting output and exclusion rules.
- Integration test: system prompt includes skills block when skills loaded.

## Non-goals
- Full skill body loading or execution (handled at runtime by skill loader).
