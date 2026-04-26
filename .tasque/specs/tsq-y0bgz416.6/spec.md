## Overview
Finalize validation/docs and mark parity closed.

## Constraints / Non-goals
- Non-goal: unrelated MCP extensions beyond defined scope.

## Interfaces (CLI/API)
- Public docs include library setup examples for stdio, SSE, and multi-server aggregation.

## Data model / schema changes
- Update parity tracking docs to reflect implemented capabilities and known non-goals.

## Acceptance criteria
1. E2E/integration tests cover stdio, SSE, multi-server routing, instruction merge.
2. `cargo test --features mcp,agent` path is green for relevant suites.
3. Docs and `feature-gap-analysis.md` updated to remove MCP “stub” status.
4. `tsq-y0bgz416` can be closed with verification evidence.

## Test plan
- Run MCP-specific tests and targeted agent/runtime regressions.
- Record command outputs in task notes.
