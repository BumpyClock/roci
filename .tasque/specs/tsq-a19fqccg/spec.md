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
- Branch summarization depends on session tree epic (tsq-jhvzt78z)
- Hook integration depends on extension system epic (tsq-pb2dxvaa)
