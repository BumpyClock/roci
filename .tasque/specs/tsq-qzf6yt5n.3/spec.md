## Goal
Let tools describe how they should appear in prompts/search without overloading `description()`.

## Scope
- Replace plain `prompt_snippet()` planning with structured prompt metadata.
- Include: capability snippet, optional guideline bullets, optional search hints.
- Keep SDK boundary at metadata; UI-specific rendering remains out of scope.

## Decisions
- Prompt metadata should be optional and stable enough for prompt caching.
- Search hints feed deferred-loading discovery; they are not aliases.
- System prompt assembly should consume metadata from active/eager tools and deferred stubs consistently.

## Acceptance
- Prompt assembly can render snippets + guideline bullets from tools.
- Deferred tool catalog can expose search hints without loading full tools.
- Tests cover omission/default behavior and deterministic ordering.