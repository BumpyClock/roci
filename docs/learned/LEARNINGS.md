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

## Sub-Agent Supervisor

- `read_when`: implementing sub-agent features or extending supervisor
- Sub-agent supervisor uses `CancellationToken` (not oneshot) for abort -- idempotent and shareable between handle and supervisor. Both hold clones of the same token.
- `AgentRuntime` is not `Clone` -- child runtimes are owned by background tokio tasks. Handles communicate via channels (oneshot for completion, watch for snapshots, broadcast for events).
- Model fallback is launch-time only. Candidate selection requires a registered provider plus usable auth: no-credential provider, `RociConfig` credential, or inherited parent `AgentConfig` request auth (`api_key_override` / `get_api_key`). There is no mid-run failover.
- Profile inheritance is single-parent only. Child scalars replace parent scalars. `models` replaces wholesale (no merge). `tools` uses `ToolPolicy` (Inherit / Replace / InheritWithOverrides).
- TOML profiles support both single-file (top-level fields) and multi-profile (`[[profiles]]`) formats. `TomlProfileFile::parse()` tries multi first, falls back to single.
- Human interaction routing across multiple children uses the same `HumanInteractionCoordinator` -- `request_id` correlation handles multi-child dispatch without a generic bus. `ask_user` is the current model-tool payload.
- Supervisor concurrency is bounded by `Semaphore` (max_concurrent, default 4). `max_active_children` is a separate hard cap that rejects spawns.
- `abort_on_drop` (default true) cancels all children via `CancellationToken` in the `Drop` impl. Uses `try_lock()` since `Drop` is synchronous.
- Launcher seam (`SubagentLauncher` trait) is `pub(super)` -- not part of the public API. Exists for testability and future out-of-process extensibility.
- `wait()` on a handle consumes the oneshot receiver. Second call returns a synthetic error, not a panic.
- Event forwarding uses `broadcast::channel(256)`. Lagged receivers trigger a status check fallback to avoid missing terminal events.

## Provider / Core Split

- `read_when`: adding custom providers, understanding crate boundaries, or debugging trait registration
- `LanguageModel` is string-based (`Known { provider_key, model_id }` / `Custom { provider, model_id }`). Provider-specific model enums (e.g., `OpenAiModel`) live in `roci-providers` only, not in the core public API.
- `ProviderRegistry` maps string keys to `Arc<dyn ProviderFactory>`. Validation happens at `create_provider()` time, not at parse time. This keeps `roci-core` provider-agnostic.
- `AuthService` is a generic orchestrator over `AuthBackend` trait objects. Hardcoded `ProviderKind` enum is gone; backend lookup by alias replaces it.
- `roci-core` has zero provider feature flags. All provider feature flags live in `roci-providers` and pass through from the `roci` meta-crate.
- Two usage paths: (a) depend on `roci` for batteries-included with `default_registry()`, (b) depend on `roci-core` + `roci-providers` directly for explicit wiring.
- Custom provider example: `examples/custom_provider.rs` demonstrates `ModelProvider` + `ProviderFactory` impl, registry registration, and both usage patterns.
- The `roci` meta-crate re-exports `roci_core::*`, so most import paths (e.g., `roci::prelude::*`) are unchanged from before the split.
