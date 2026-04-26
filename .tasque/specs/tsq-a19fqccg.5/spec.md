# Hook integration for compaction/tree

## Scope
- Hook interfaces:
  - session_before_compact (cancel or override summary)
  - session_before_tree (cancel or override branch summary)
- Hook payload includes preparation data (messages, tokens, file ops, settings)

## Dependencies
- Requires extension runtime (tsq-pb2dxvaa)

## Acceptance
- Hook interfaces wired into compaction + branch summary paths
- Tests with mock hooks
