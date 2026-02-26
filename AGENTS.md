# AGENTS.MD

READ ~/.agents/AGENTS.md BEFORE ANYTHING (skip if files missing).



roci notes:
- Default workflow: lint, format, and test before publishing.
- Docs are under `docs/`; read & update as needed.
- Use `.env` for sensitive keys; refer to `.env.example` for structure. Keep `.env.example` updated as needed.
- Tests validate new features and implementations against real endpoints using ` cargo test --test live_providers -- --ignored --nocapture`.

# Core
- use `tasque` for persistent task management
- use `rust-skills` for core rust language guidance. 
- use `clippy` for linting and `rustfmt` for formatting. Do so before committing code.
