# Extension runtime + loader (roci-core)

## Scope
- Implement extension runtime per chosen ABI.
- Load extensions from configured paths; produce diagnostics on failure.
- Extension can register tools/commands and lifecycle hooks; hooks plumbed into core agent loop (non-TUI only).

## Acceptance
- Unit tests: load valid extension, handle load error.
- No hard-coded paths in roci-core; loader accepts explicit roots/paths.

## Non-goals
- TUI bindings.
