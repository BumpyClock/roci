# Goal
Bring event payloads to parity for downstream consumers by including message context on turn and run end events.

# Scope
- `TurnEnd` payload shape
- `AgentEnd` payload shape
- emitter + consumer updates in roci-core and demo CLI formatting

# Acceptance Criteria
- `turn_end` includes assistant message and tool results (or explicit null/empty semantics).
- `agent_end` includes final message list snapshot (or equivalent documented payload guaranteeing same capability).
- Serialization compatibility documented; tests updated for new schema.
- Existing subscribers continue working or migration notes added.

# Dependencies
- Depends on assistant-message persistence task.
