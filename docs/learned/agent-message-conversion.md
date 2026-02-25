# Agent Message Conversion Path

read_when:
- You need to include non-LLM agent messages (artifacts/metadata) in runtime context while controlling what reaches providers.
- You are wiring custom context filtering/conversion before provider sanitize.

## Decision
- Added `convert_to_llm` as an explicit pre-provider hook in `RunRequest` and `AgentConfig`.
- Execution order in runner:
1. Build loop context (`Vec<ModelMessage>`)
2. Optional `convert_to_llm(Vec<AgentMessage>) -> Vec<ModelMessage>`
3. Optional `transform_context(Vec<ModelMessage>) -> Vec<ModelMessage>`
4. Provider sanitize + request

## Rationale
- Keeps backward compatibility (`convert_to_llm` unset preserves existing behavior).
- Separates concerns:
  - `convert_to_llm`: message-type filtering/translation boundary.
  - `transform_context`: last-mile LLM context rewrite.
- Enables custom-message filtering by using existing `agent::message::{AgentMessage, convert_to_llm}` utilities.

## Notes
- Current runtime loop still stores canonical LLM messages (`ModelMessage`) for persistence and tool-loop stability.
- If deeper parity is needed later, extend runtime storage to a richer agent-message timeline and use `convert_to_llm` for deterministic projection per turn.
