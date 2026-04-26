## Goal
Define the clean final safety contract for every tool.

## Chosen API shape
Use a domain-specific method returning a grouped struct:
- `fn safety(&self, args: &ToolArguments) -> ToolSafety`
- `ToolSafety { concurrency_safe, read_only, destructive }`

This replaces planning assumptions around separate scalar methods like `is_concurrency_safe()`, `is_read_only()`, and `is_destructive()` as the final public SDK shape.

## Scope
- Replace hard-coded `is_parallel_safe_tool()` name list and approval-kind name mapping.
- Recommended API: input-aware safety classification so shell/apply-patch can distinguish safe vs mutating calls.
- Cover `AgentTool`, builtins, and dynamic/deferred tool adapters.

## Decisions
- Fail closed by default: unspecified tools are not concurrency-safe, not read-only, not destructive unless explicitly classified.
- `destructive` is a separate bit from `read_only`; destructive means irreversible/high-impact, not merely mutating.
- Approval and hook payloads should receive the safety classification so callers stop re-deriving it from names.
- Do not keep legacy runner fallbacks once the new metadata exists.

## Acceptance
- Runner batches only tools marked concurrency-safe.
- Ask-policy approval logic uses tool safety instead of name tables.
- Builtins and adapters declare safety explicitly and tests cover mixed batches.