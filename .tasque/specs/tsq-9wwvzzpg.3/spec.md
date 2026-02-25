# Extract built-in providers + OAuth flows to roci-providers

## Scope
- Move provider transports (openai/anthropic/google/etc.) and auth providers (codex/copilot/claude) into `roci-providers`.
- Keep `Token`, `TokenStore`, and `RociConfig` in core; providers depend on core.
- Expose `register_default_providers(registry, config)` or similar initializer.
- Preserve feature flags for provider selection.

## Acceptance criteria
1) `roci-core` has no provider-specific transport deps.
2) `roci-providers` builds and registers all current providers.
3) CLI (later) can depend on `roci-providers` to access OAuth flows.
