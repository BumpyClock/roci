# Provider registry + roci-providers split

## Goal
Provide a clean separation where `roci-core` is provider-agnostic and `roci-providers` supplies built-in transports + OAuth flows, while keeping a simple DX via a top-level `roci` crate that re-exports defaults.

## Scope
- Define provider registry API for custom providers.
- Split provider implementations and OAuth flows into `roci-providers`.
- Keep `roci-core` focused on agent loop, runtime, types, config, and the registry interface.
- Provide a “batteries-included” `roci` meta-crate to preserve current DX.

## Non-goals
- No behavior changes to provider request/response semantics beyond the split.
- No new providers added in this epic.

## Acceptance criteria
1) `roci-core` builds without provider-specific dependencies.
2) `roci-providers` registers all built-in providers behind feature flags.
3) `roci` crate re-exports `roci-core` plus default providers for the existing DX.
4) Custom provider registration is documented and tested.
