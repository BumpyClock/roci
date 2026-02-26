# Skill model + frontmatter parsing (roci-core)

## Scope
- Parse YAML frontmatter from SKILL.md only; do not load full body.
- Produce `SkillMetadata` with `name`, `description`, `path`, `base_dir`, `disable_model_invocation`.
- Validation aligned with pi-mono:
  - name <=64 chars, lowercase a-z0-9-, no leading/trailing hyphen, no `--`.
  - description required, <=1024 chars.
  - if `name` missing, default to parent dir name.
  - warn on mismatches (name != dir), but still load if description present.

## Acceptance
- Invalid frontmatter yields diagnostic and skill skipped only when description missing or YAML invalid.
- Unit tests for valid/invalid frontmatter.
- API surface in roci-core with clear error/diagnostic type.

## Non-goals
- Parsing extra metadata files (openai.yaml) or embedding full skill content.
