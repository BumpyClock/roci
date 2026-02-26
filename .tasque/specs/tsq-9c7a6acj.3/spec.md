# Context files + system/append prompt loader

## Scope
- Discover AGENTS.md / CLAUDE.md
  - Global: ~/.roci/agent
  - Project: walk up from cwd to root
  - De-dup by path
  - Order: global first, then ancestors from root -> cwd
- Discover system prompt file
  - Prefer .roci/SYSTEM.md, fallback to ~/.roci/agent/SYSTEM.md
- Discover append system prompt
  - Prefer .roci/APPEND_SYSTEM.md, fallback to ~/.roci/agent/APPEND_SYSTEM.md

## API
Expose a ResourceLoader (or ResourceBundle) with:
- get_context_files() -> [{path, content}]
- get_system_prompt() -> Option<String>
- get_append_system_prompts() -> Vec<String>
- constructor/options to override agent_dir/project_dir and provide explicit context/system/append for library use

## Acceptance
- Tests for discovery order + de-dupe
- Overrides honored when passed via roci-core API
