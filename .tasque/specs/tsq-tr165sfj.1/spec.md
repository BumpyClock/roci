## Goal
Define the typed section/document model for system prompt assembly in `roci-core` and the canonical ordering rules used by all callers.

## Deliver
- `PromptSectionKind` / equivalent typed section model.
- `PromptFragment` base contract and provenance fields.
- `SystemPromptBuilder`/`BuiltSystemPrompt` shape.
- Canonical ordering and merge rules.
- Tests/specs for empty-section omission, ordering, and deterministic rendering.

## Scope guardrails
- No provider message splitting in this task.
- No filesystem discovery logic here.
- Keep `AgentConfig.system_prompt` compatibility.

## Acceptance
- The API is generic enough for roci-cli and future SDK embedders.
- Rendering order is deterministic and documented.
- Output remains renderable as a single string.
- Section/fragment metadata is sufficient for future prompt-debug tooling.