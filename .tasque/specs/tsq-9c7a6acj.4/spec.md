# Prompt template loader + expansion

## Scope
- Load from ~/.roci/agent/prompts/*.md and .roci/prompts/*.md (non-recursive)
- Load explicit prompt paths from settings (files or directories)
- Frontmatter parsing for description (fallback to first non-empty line)
- Path normalization (~, relative)
- Expand when input starts with /<name> and template exists; otherwise send raw
  - Supports $1..$n, $@, $ARGUMENTS, ${@:N}, ${@:N:L}
  - No recursive substitution

## Acceptance
- PromptTemplate struct (name, description, content, source, file_path)
- Collision behavior documented (first wins, diagnostics logged)
- Tests for discovery + substitution
