## Goal
Add a fragment registration mechanism so tools, skills, environment adapters, and context-file integrations can contribute prompt fragments without hand-editing string concatenation paths.

## Deliver
- `PromptFragmentProvider`/equivalent registration contract.
- Dedupe/provenance rules.
- Integration adapters for common SDK-owned sources (skills, environment, context files, tool catalogs).
- Tests/specs for provider merge order, collision handling, and source attribution.

## Decision guardrails
- Keep execution traits (`Tool`) decoupled from prompt-contribution concerns in v1.
- Avoid app-specific registries or hard-coded roci-cli assumptions.
- Coordinate with `tsq-wag94sf4.2`; future planning-tool prompt text should fit this mechanism without changing core types.

## Acceptance
- Multiple providers can target the same section safely.
- Duplicate fragment IDs/keys resolve deterministically with diagnostics.
- Skills/tools/environment can be supplied as typed fragment providers, not ad hoc text blobs.