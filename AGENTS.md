# AGENTS.MD

READ /Users/adityasharma/Projects/dotfiles/AGENTS.md BEFORE ANYTHING (skip if files missing).



roci notes:
- Default workflow: lint, format, and test before publishing.
- Adapters live under `src/providers`; keep new providers consistent with existing patterns.
- Docs are under `docs/`; read & update as needed.
- Use `.env` for sensitive keys; refer to `.env.example` for structure. Keep `.env.example` updated as needed.
- Tests validate new features and implementations against real endpoints using ` cargo test --test live_providers -- --ignored --nocapture`.
- After each iteration run tests to ensure parity with Tachikoma.
