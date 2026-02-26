# Compaction + summarization pipeline (non-TUI)

## Goal
Implement pi-mono style compaction and branch summarization in roci core:
- Auto-compaction based on context window
- Manual compaction API
- Structured summaries with file ops
- Branch summary pipeline for tree navigation (non-TUI)

## Scope
- Core library + CLI API surfaces
- Settings for compaction/branch summary

## Dependencies
- Hook integration tracked in hooks epic (tsq-z6nvyqfh)
- Session tree/fork integration is deferred and not a blocker for this epic
