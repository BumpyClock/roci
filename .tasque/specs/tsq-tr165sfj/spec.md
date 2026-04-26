## Context
Roci currently assembles one flat `system_prompt` string in `crates/roci-cli/src/chat.rs` by merging `SYSTEM.md`/CLI base text, `APPEND_SYSTEM.md`, rendered AGENTS/CLAUDE context files, visible skills, and MCP instructions. The SDK surface still exposes only `Option<String>` on `AgentConfig`, so prompt assembly is not yet modeled as a first-class SDK concern.

Prior art review:
- Roci today: resource discovery is already typed (`ResourceLoader`, `ContextPromptResources`, `LoadSkillsResult`) but final prompt assembly is string concatenation in the CLI.
- Pi coding-agent: `buildSystemPrompt()` accepts typed inputs (`selectedTools`, `toolSnippets`, `contextFiles`, `skills`, `appendSystemPrompt`) but still returns one string; resource loading is separate.
- Pi mom: one large builder function mixes environment, workspace layout, skill discovery, and operating policy into a single prompt.
- Codex CLI: keeps base instructions distinct from user instructions, uses explicit AGENTS fragments/tags, and enforces byte-budgeted AGENTS loading before concatenation.
- Claude Code: separates `defaultSystemPrompt[]`, `userContext`, and `systemContext`, uses fragment-like prompt sections/tool prompts, and applies targeted budgets for skill listings.

## Decision
Introduce an SDK-side `SystemPromptBuilder` that assembles a single rendered system prompt from typed sections and typed fragments, while keeping message-level itemization out of scope for this epic.

This epic should deliver:
- a stable section model and ordering,
- a fragment registration mechanism for tools/skills/environment/context integrations,
- token-budget-aware trimming with diagnostics,
- a migration path that preserves the existing `AgentConfig.system_prompt: Option<String>` API.

## Section model and ordering
Required v1 section kinds:
1. `instructions`
2. `tools`
3. `skills`
4. `environment`
5. `context_files`
6. `custom` (escape hatch for app-level additions)

Default render order:
1. instructions
2. tools
3. skills
4. environment
5. context_files
6. custom

Rules:
- order is deterministic and SDK-owned;
- each section is a list of fragments, not a single string;
- empty sections are omitted;
- sections render to one final string by default;
- builder metadata must preserve per-section/per-fragment provenance for debugging and future provider itemization.

## Recommended API
```text
SystemPromptBuilder
  .with_base_text(...)
  .with_budget(...)
  .register(provider)
  .push_fragment(fragment)
  .build() -> BuiltSystemPrompt

BuiltSystemPrompt {
  text: String,
  estimated_tokens: usize,
  sections: Vec<RenderedSection>,
  dropped_fragments: Vec<DroppedFragment>,
  diagnostics: Vec<PromptBuildDiagnostic>,
}
```

Supporting contracts:
- `PromptSectionKind` enum for the typed section list above.
- `PromptFragment` with `id`, `section`, `source`, `required`, `priority`, `body/full`, optional `body_compact`, and optional `dedupe_key`.
- `PromptFragmentProvider` trait so tools, skill catalogs, environment adapters, and app-owned integrations can contribute fragments without mutating core builder code.
- `TokenEstimator` trait with a default heuristic estimator and optional model-specific override.

## SDK vs app boundary
Belongs in SDK:
- section/fragment types,
- deterministic ordering,
- fragment registration/merging,
- token estimation and trimming,
- render diagnostics,
- helpers for formatting common section types.

Belongs outside the builder (resource/app layer):
- filesystem discovery of AGENTS.md / CLAUDE.md / SYSTEM.md / APPEND_SYSTEM.md,
- prompt-template expansion,
- skill discovery,
- MCP connection discovery,
- app-specific environment snapshots,
- policy decisions about which context files or skills to load in the first place.

The builder should consume already-loaded typed inputs, not walk the filesystem.

## Fragment registration mechanism
Recommended v1 approach:
- keep the existing `Tool` trait unchanged;
- add a separate prompt-contribution trait / provider interface rather than coupling execution and prompt text;
- allow multiple providers to target the same section;
- dedupe by explicit key, not by raw text equality;
- preserve source metadata (`tool:<name>`, `skill:<name>`, `resource:<path>`, `app:<name>`).

This keeps the SDK generic and avoids overfitting to roci-cli or any single app.

## Trimming strategy under token pressure
Budget policy:
- builder accepts `max_prompt_tokens` and `reserve_tokens`;
- build computes an estimate before render completion;
- trimming is deterministic and reported.

Trim order:
1. drop empty/disabled fragments;
2. switch fragments with compact variants from `full` to `compact`;
3. shorten repeated catalogs (tools/skills) before touching instructions;
4. prune optional low-priority fragments;
5. truncate long context-file fragments with an explicit truncation marker;
6. if required fragments alone exceed budget, return an overflow diagnostic instead of silently mangling the prompt.

Section priorities:
- `instructions`: last to trim; required by default.
- `tools`: mostly required, but verbose descriptions are optional.
- `skills`: optional catalog; easiest to compress.
- `environment`: required summary, optional verbose details.
- `context_files`: optional per-file bodies after higher-priority files.
- `custom`: caller decides priority/requiredness.

## Context file integration boundaries
The builder should render supplied context files as fragments with path/source metadata, but should not decide discovery rules. Existing `ResourceLoader` behavior remains the source of truth for AGENTS/CLAUDE precedence and ordering.

Important v1 behavior:
- preserve incoming file order from the loader;
- support per-file trim priority so nearer/higher-priority files survive longer;
- expose diagnostics when files are truncated/dropped;
- do not add new include/import semantics in this epic.

## Compatibility and migration path
- Preserve `AgentConfig.system_prompt: Option<String>` in v1.
- First adopter should be roci-cli chat assembly.
- Existing helper functions can be reimplemented on top of the builder with output-compatible rendering.
- Do not split prompts into provider-specific system/developer/user items in this epic; that overlaps with `tsq-wag94sf4.1`.
- Ensure future built-in planning tool work (`tsq-wag94sf4.2`) can register tool fragments without redesigning the builder.

## Acceptance criteria
- `SystemPromptBuilder` API and typed section model are documented in SDK docs/spec.
- Tool/skill/environment/context integrations can register prompt fragments without editing builder internals.
- A build returns rendered text plus trim/debug metadata.
- Trimming behavior is deterministic and covered by tests.
- roci-cli can migrate from ad hoc concatenation to the builder without changing the external `AgentConfig` contract.
- Scope remains SDK-only; no provider itemization or CLI-only policy leakage.

## Open questions
- Should the SDK expose only approximate token estimation in v1, or also a provider/model-specific tokenizer hook?
- Should builtin tools ship prompt-fragment adapters in `roci-tools`, or should apps provide tool prompt text explicitly?
- Do we need per-fragment visibility modes for future provider itemization, or is provenance metadata enough for now?
- Should context-file truncation preserve whole files only, or allow per-file body truncation with markers?