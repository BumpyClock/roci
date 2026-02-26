# Learned Index

- [Agent Message Conversion Path](./agent-message-conversion.md)
  - read_when: filtering/converting non-LLM agent messages before provider sanitize.
- [Agent Event Contracts: Message Lifecycle + Tool Updates](./agent-events-tool-updates.md)
  - read_when: wiring `AgentEvent` streaming boundaries, tool updates, cancel/fail semantics.
- [OpenAI/Gemini API Shapes](./openai-gemini-api-shapes.md)
- [OpenAI Responses Options](./openai-responses-options.md)
- [Ralph Loop Parallel](./ralph-loop-parallel.md)
- [MCP Protocol + Integration Notes](./mcp.md)
  - read_when: validate MCP transport parity (stdio/SSE/multi-server) and OpenAI instruction merge behavior.

## Architecture Decisions
- `read_when`: modifying crate boundaries, adding new crates, or changing public API surface
- See: `docs/architecture/cli-soc.md` -- CLI/core separation of concerns
- `read_when`: splitting provider transports from core, designing ProviderFactory/AuthBackend traits, adding new providers
- See: `docs/architecture/providers-soc.md` -- Provider/core separation of concerns

## Provider / Core Split

- `read_when`: adding custom providers, understanding crate boundaries, or debugging trait registration
- `LanguageModel` is string-based (`Known { provider_key, model_id }` / `Custom { provider, model_id }`). Provider-specific model enums (e.g., `OpenAiModel`) live in `roci-providers` only, not in the core public API.
- `ProviderRegistry` maps string keys to `Arc<dyn ProviderFactory>`. Validation happens at `create_provider()` time, not at parse time. This keeps `roci-core` provider-agnostic.
- `AuthService` is a generic orchestrator over `AuthBackend` trait objects. Hardcoded `ProviderKind` enum is gone; backend lookup by alias replaces it.
- `roci-core` has zero provider feature flags. All provider feature flags live in `roci-providers` and pass through from the `roci` meta-crate.
- Two usage paths: (a) depend on `roci` for batteries-included with `default_registry()`, (b) depend on `roci-core` + `roci-providers` directly for explicit wiring.
- Custom provider example: `examples/custom_provider.rs` demonstrates `ModelProvider` + `ProviderFactory` impl, registry registration, and both usage patterns.
- The `roci` meta-crate re-exports `roci_core::*`, so most import paths (e.g., `roci::prelude::*`) are unchanged from before the split.
