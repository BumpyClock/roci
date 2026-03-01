# Agent Message Conversion Path

read_when:
- You need to include non-LLM agent messages (artifacts/metadata) in runtime context while controlling what reaches providers.
- You are wiring custom context filtering/conversion before provider sanitize.

## Decision
- Added `convert_to_llm` as an explicit pre-provider hook in `RunRequest` and `AgentConfig`.
- Execution order in runner:
1. Build loop context (`Vec<ModelMessage>`)
2. Optional `transform_context(TransformContextHookPayload) -> Result<TransformContextHookResult, RociError>`
3. Optional `convert_to_llm(ConvertToLlmHookPayload) -> Result<ConvertToLlmHookResult, RociError>`
4. Provider sanitize + request

## Rationale
- Keeps backward compatibility (`convert_to_llm` unset preserves existing behavior).
- Separates concerns:
  - `transform_context`: pre-conversion context mutation/cancellation boundary.
  - `convert_to_llm`: post-transform message-type filtering/translation boundary.
- Enables custom-message filtering by using existing `agent::message::{AgentMessage, convert_to_llm}` utilities.
- Both hooks receive typed payloads (run/model/context + cancellation token) and support `Continue` / `Cancel` / `ReplaceMessages`.

## Notes
- Current runtime loop still stores canonical LLM messages (`ModelMessage`) for persistence and tool-loop stability.
- If deeper parity is needed later, extend runtime storage to a richer agent-message timeline and use `convert_to_llm` for deterministic projection per turn.
