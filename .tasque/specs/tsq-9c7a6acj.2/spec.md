# Resource settings + directory conventions

## Goal
Define resource locations and settings schema used by the resource loader.

## Decisions
- Global dir: ~/.roci/agent
- Project dir: .roci
- Settings: <dir>/settings.json with deep-merge (project overrides global)
- CLI uses fixed paths; overrides only via roci-core APIs for library users

## Settings (minimal)
- prompts: string[] (files or directories)
- no_prompt_templates: boolean (default false)
- no_context_files: boolean (default false)

## Acceptance
- Settings struct + read-only loader in roci-core
- Path resolution supports ~ and relative paths (relative to scope dir)
- roci-core API allows overriding agent_dir/project_dir
- Unit tests for merge + path resolution
