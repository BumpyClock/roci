## Goal
Support large tool sets without forcing all tools into the initial prompt.

## Scope
- Generalize deferred loading beyond dynamic/MCP tools to builtins and custom tools.
- Introduce catalog/search/materialization boundaries.
- Tool search should use canonical name, aliases, snippet, and search hints from metadata.

## Decisions
- Model-facing deferred stubs should be data-only catalog entries, not loaded `Tool` instances.
- Materialization should return the canonical loaded tool on demand.
- Provide eager fallback so environments/models without deferred-loading still work.

## Acceptance
- One search/provider abstraction can serve builtins, SDK custom tools, and dynamic tools.
- Deferred catalogs do not require loading every executable tool upfront.
- Tests cover search hits, alias resolution, and eager fallback behavior.