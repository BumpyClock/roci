# Harden model selector codex routing heuristic

## Problem
`openai:*` model IDs containing `codex` currently auto-route to `openai-codex`, which can misroute custom or fine-tuned models.

## Acceptance Criteria
1. Only explicit codex providers/aliases (`openai-codex`, `openai_codex`, `codex`) route to `OpenAiCodex` by default.
2. Any heuristic exception is explicit, documented, and covered by tests for false positives.
3. Selector tests cover routing matrix and preserve existing valid routes.
