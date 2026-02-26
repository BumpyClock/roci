# Integrate resource loader into AgentRuntime + CLI

## Scope
- Expose ResourceLoader/ResourceBundle in roci-core public API
- Build final system prompt:
  - Base: --system if provided, else SYSTEM.md if present
  - Append: APPEND_SYSTEM.md (if present)
  - Context files appended under a "Project Context" section
- Expand prompt templates for chat input when /<template>
- CLI uses DefaultResourceLoader with fixed paths; no CLI override flags

## Acceptance
- CLI uses DefaultResourceLoader by default
- Prompt template expansion applied before sending prompt
- System prompt includes append + context when available
- Unit tests for prompt assembly and template expansion
