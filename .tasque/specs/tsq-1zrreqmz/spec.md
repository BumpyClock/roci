## Overview
Implement skill package management in the roci library + demo CLI, aligned with pi-mono package command intent while deferring extension package execution.

Scope for this epic:
- Add core library APIs to manage install/remove/update/list lifecycle for skill packages.
- Add CLI command surface to call those core APIs.
- Keep extensions deferred; do not add extension runtime loading in this epic.

## Constraints / Non-goals
- Non-goal: extension runtime/ABI/package loading (tracked in deferred extension epic).
- Non-goal: TUI alignment.
- Core capability must live in roci-core; roci-cli remains a demo consumer.
- No hard-coded personal paths; rely on ResourceDirectories resolution.

## Interfaces (CLI/API)
CLI:
- `roci-agent skills install <source> [--local]`
- `roci-agent skills remove <name> [--local]`
- `roci-agent skills update [name] [--local]`
- `roci-agent skills list`

Core API:
- `skills::manager` module exposing:
  - install from source into project/global roots
  - remove installed skill
  - update one/all installed skills
  - list discovered + managed skills metadata

## Data model / schema changes
- Add managed-skill manifest file per scope under skill roots (machine-owned JSON) to retain source -> installed skill mapping for update/remove.
- Do not require changing existing settings schema for this epic.

## Acceptance criteria
- Install command can ingest at least local path and git URL sources, install one or more discovered skills into selected scope, and persist manifest metadata.
- Remove command deletes installed skill directory and updates manifest.
- Update command refreshes one/all managed skills using persisted source.
- List command prints discovered skills with scope/source/path context.
- Commands route through roci-core APIs (CLI is thin orchestration).
- Extensions remain untouched/deferred.

## Test plan
- roci-core unit tests for:
  - install local source with one/multiple skills
  - install from git source (fixture/local git repo)
  - remove updates manifest and filesystem
  - update one/all re-sync behavior
  - list includes managed + unmanaged skills
- roci-cli parser tests for new `skills` subcommands.
- roci-cli integration-style tests for happy path + error path messaging where practical.
