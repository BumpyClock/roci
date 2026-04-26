# Compaction utilities + serialization

## Scope
- Token estimation + context usage
- Cut-point detection (never cut at tool results)
- PrepareCompaction (messagesToSummarize, turnPrefixMessages, split-turn handling)
- Serialization to text for summarization
- File ops extraction (read/modified) from tool calls for built-in tools
- Summary format aligned to pi-mono (Goal/Constraints/Progress/Decisions/Next Steps/Critical Context + file lists)

## Acceptance
- Pure, testable functions in roci-core
- Unit tests for cut-point + split-turn + serialization
