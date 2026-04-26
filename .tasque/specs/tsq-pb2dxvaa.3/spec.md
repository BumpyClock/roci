# CLI/config wiring for extensions

## Scope
- CLI/config resolves default extension roots (user + project) and optional explicit paths.
- Pass resolved paths + config into roci-core loader.
- Add flags to disable extensions, and to add extra paths.

## Acceptance
- CLI can run with extensions disabled or custom paths.
- roci-core remains path-agnostic.
