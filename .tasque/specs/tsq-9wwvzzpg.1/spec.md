# Define provider SoC boundaries + public API

## Scope
- Decide final crate names (`roci-core`, `roci-providers`, `roci`).
- Define what stays in core (types, runtime, registry, config) vs providers (transports, OAuth flows, provider factories).
- Define feature flags and re-export strategy.
- Record ADR in `docs/architecture/providers-soc.md` + add `read_when` entry.

## Acceptance criteria
1) ADR created and linked in `docs/learned/LEARNINGS.md`.
2) Public API plan includes how users import core + providers.
3) Custom provider extension points are explicitly documented.
