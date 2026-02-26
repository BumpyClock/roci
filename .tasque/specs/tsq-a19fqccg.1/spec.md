# Compaction settings + schema

## Scope
- Add Settings loader in roci-core
  - Global: ~/.roci/agent/settings.json
  - Project: .roci/settings.json
  - Deep merge (project overrides global)
  - Override agent_dir/project_dir via roci-core API
- Include settings:
  - compaction.enabled (default true)
  - compaction.reserve_tokens (default 16384)
  - compaction.keep_recent_tokens (default 20000)
  - compaction.model (optional provider:model for summaries)
  - branch_summary.reserve_tokens (default 16384)
  - branch_summary.model (optional provider:model for summaries)

## Acceptance
- Read-only settings API exposed
- Defaults applied when files missing
- Tests for merge + defaults
