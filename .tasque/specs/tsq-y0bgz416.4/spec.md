## Overview
Integrate MCP server `instructions` at library level.

## Constraints / Non-goals
- Non-goal: auto-mutating user prompts in CLI.
- Caller must retain control over final prompt assembly.

## Interfaces (CLI/API)
- Expose instruction retrieval APIs per server and aggregated.
- Provide deterministic merge helper for combining instructions with existing system prompt.
- Default merge policy: append MCP instruction block after existing system prompt.
- Merged instruction text must expose model-visible server labels in the form `[server:<id>]`.

## Data model / schema changes
- Instruction payload model with server provenance.
- Merge policy enum/options.

## Acceptance criteria
1. Instructions are retrievable for single server and aggregated server sets.
2. Merge helper outputs deterministic text/order and follows append-default policy.
3. Provenance is preserved for debugging and audits.
4. Merged prompt content includes explicit `[server:<id>]` labels for each instruction source.

## Test plan
- Unit tests for merge policy/order/dedup behavior.
- Integration tests validating instructions flow through aggregator.
