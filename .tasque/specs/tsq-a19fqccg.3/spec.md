# Compaction execution + auto-trigger

## Scope
- Auto-compaction before provider call when context exceeds window - reserveTokens
- Manual compaction API (AgentRuntime::compact)
- Async compaction execution (LLM call) with structured summary
- Replace message history with summary + kept messages
- Compaction failure: fail the run and surface error (no silent skip)
- Summary model selection: compaction.model if set, else run model

## Acceptance
- Runner supports async compaction hook
- Manual compaction callable from API
- Tests for auto-compaction trigger and message replacement
