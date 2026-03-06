## Context
Source note: /Users/adityasharma/Projects/roci/.ai_agents/codex_agent_loop.md

## Scope
This epic is not about 1:1 Codex parity.
Adopt Codex loop ideas only where they improve roci as a provider-agnostic SDK/runtime.
Do not force provider-specific behavior where it conflicts with provider-agnostic design.

## Current findings to track
- Prompt assembly is collapsed into one system prompt instead of separate developer/user/environment items.
- No built-in update_plan parity.
- shell approval semantics are weaker than Codex-style sandbox/approval framing.
- OpenAI Responses path can reuse previous_response_id via session_id/session cache, so behavior is not strictly stateless.
- Provider-agnostic compaction is acceptable, but reasoning persistence and prompt-prefix stability still need explicit design decisions.

## Acceptance
- Child tasks are prioritized by impact on roci.
- Provider-agnostic constraints are documented in child specs.
- Sequencing reflects real dependencies, not parity for parity's sake.
