## Summary
Plan the next Roci SDK tool-system revision as a clean breaking change. Replace runner heuristics based on hard-coded tool names with first-class tool metadata and catalog/search plumbing that works for builtins, custom SDK tools, and deferred/dynamic tool sources.

## Product stance
- Roci is still in active development.
- There are no external SDK users to preserve.
- Prefer the clean final trait/API shape over compatibility wrappers, deprecation layers, or transitional adapter APIs.

## Chosen tool contract direction
Use **domain-specific methods returning grouped structs**.

Recommended direction:
```rust
trait Tool {
  fn name(&self) -> &str;
  fn aliases(&self) -> &[String];
  fn description(&self) -> &str;
  fn parameters(&self) -> &AgentToolParameters;
  fn safety(&self, args: &ToolArguments) -> ToolSafety;
  fn result_policy(&self) -> ToolResultPolicy;
  fn prompt_info(&self) -> Option<ToolPromptInfo>;
  fn loading_policy(&self) -> ToolLoadingPolicy;
  async fn execute(...);
  async fn execute_ext(...);
}
```

Where:
- `ToolSafety { concurrency_safe, read_only, destructive }`
- `ToolResultPolicy { max_result_size_bytes }`
- `ToolPromptInfo { snippet, guidelines, search_hints }`
- `ToolLoadingPolicy::{Eager, Deferred { always_load, search_hints }}`

This is the chosen direction over either:
- many small scalar metadata methods, or
- a single monolithic metadata blob.

## Migration strategy
- Treat this as a deliberate SDK break.
- Update built-in tools, `AgentTool`, and dynamic/deferred adapters directly to the new contract.
- Remove runner hard-coded lists/maps for parallel-safe tools and approval kinds rather than keeping legacy fallback logic.
- Keep aliases only where they serve canonical naming and transcript stability; do not build a broad compatibility layer around old trait shapes.
- Do NOT design generic UI affordances here; SDK/runtime behavior only.

## Runtime behavior changes
- Tool batching: parallelize only calls whose `safety(...).concurrency_safe == true`; preserve result append order.
- Approval routing: default approval kind derives from tool safety (`read_only` => auto-allow under Ask, otherwise ask; `destructive` escalates reason text/hooks).
- Result handling: central truncation/overflow envelope uses `max_result_size_bytes` on serialized JSON output.
- Prompt assembly: system prompt/tool catalog includes structured tool prompt metadata instead of only description strings.
- Deferred loading: agent loop can advertise deferred tool stubs from any source and materialize them via search/provider lookup.

## apply-patch decision and limits
- Add `apply_patch` in `roci-tools` using the Codex/Copilot grammar (`*** Begin Patch` / Add / Delete / Update / Move / End of File / End Patch`).
- Keep grammar parsing tool-local for now: no generic freeform-grammar tool abstraction in this epic.
- Recommended model-facing schema for v1: JSON object with one required `patch` string field.
- Parse -> verify paths/ops -> classify safety -> execute. No binary edits, chmod, directory moves, or paths outside workspace in v1.
- Idempotency must be documented: duplicate patch application may fail; executor should return deterministic error payloads.

## Deferred loading boundaries
- Scope includes builtins, custom SDK tools, and dynamic/MCP-like tools.
- Separate `ToolCatalogEntry`/stub metadata from loaded executable `Tool` instances.
- `ToolSearchProvider` owns discovery/search/materialization, not prompt assembly.
- Fallback path: environments without deferred-loading support may eagerly materialize the same catalog entries.

## Acceptance criteria
- Tool metadata fully replaces hard-coded runner safety and approval heuristics.
- Alias dispatch works for execution and transcript/replay lookup.
- Prompt metadata supports snippet + guideline + search-hint use cases.
- Deferred loading works across all tool sources, not only dynamic providers.
- `apply_patch` parses the adopted grammar, verifies paths, surfaces safety classification, and participates in approval hooks.
- Existing builtins have explicit metadata/tests for safety/result policy/prompt info/loading policy.

## Open questions
- Result budget unit: bytes of serialized JSON (recommended) vs chars.
- Whether tool search should support model-side references only, or also runtime/local search fallbacks in the same API.