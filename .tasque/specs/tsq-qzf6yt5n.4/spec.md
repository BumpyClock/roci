## Goal
Support stable canonical tool identity while keeping the public SDK shape clean.

## Scope
- Add `aliases` alongside canonical primary names.
- Lookup by alias should resolve to one canonical tool.
- Collision handling must be explicit for registry/search/deferred catalogs.

## Decisions
- Canonical name remains the single real identity for schema/export/registry purposes.
- Aliases are lightweight name indirection for rename stability and internal transcript handling, not a general SDK compatibility layer.
- New writes should persist canonical names; alias resolution happens at lookup/materialization boundaries only.
- Do not add legacy tool wrapper types just to preserve pre-break naming behavior.

## Acceptance
- Registry rejects ambiguous alias collisions.
- Execution, replay lookup, and deferred materialization honor aliases.
- Tests cover canonical-name persistence and alias lookup.