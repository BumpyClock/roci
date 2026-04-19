# AGENTS.MD

roci notes:
- Default workflow: lint, format, and test before publishing.
- Docs are under `docs/`; read & update as needed.
- Use `.env` for sensitive keys; refer to `.env.example` for structure. Keep `.env.example` updated as needed.
- Validate provider-facing changes with the relevant crate tests; this repo does not currently ship a `live_providers` integration target.
- Use parallel-subagents and agent-teams for all tasks.

# Core
- use `tasque` for persistent task management
- use `rust-skills` for core rust language guidance. 
- use `clippy` for linting and `rustfmt` for formatting. Do so before committing code.
