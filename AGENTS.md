# AGENTS.MD

READ ~/Projects/dotfiles/.ai_agents/{AGENTS.MD,TOOLS.MD} BEFORE ANYTHING (skip if files missing).

## Porting Tachikoma to Roci
Roci is a port of Tachikoma in rust using rust native patterns , idioms and optimizations.
Port is in progress but partity not achieved yet. Continue iterating and testing until full parity is reached.

## Flow
0. Read feature-gap-analysis.md for current gaps. If it doesn't exist, create it by comparing Tachikoma and Roci. Do a thorough analysis. Write down all gaps in detail.
1. Identify the feature / implementation gap between Tachikoma and Roci.
2. Implement feature / fix gap in Roci.
3. Write test against real providers to validate implementation.
4. Run tests to ensure parity.
5. Ensure test completes successfully. If not, iterate from step 2. Use Tachikoma as reference.
6. Update docs if needed.
7. Commit and push changes.
7. update `LEARNINGS.md` with any new learnings from the implementation.
8. Update `feature-gap-analysis.md` to reflect closed gaps.
9. Validate that `feature-gap-analysis.md` is accurate and no new gaps were introduced. If they were update feature-gap-analysis.md.
10. If all gaps are closed, output `Task Complete` and nothing else.

roci notes:
- Default workflow: lint, format, and test before publishing.
- Adapters live under `src/providers`; keep new providers consistent with existing patterns.
- Docs are under `docs/`; read & update as needed.
- Use `.env` for sensitive keys; refer to `.env.example` for structure. Keep `.env.example` updated as needed.
- Tests validate new features and implementations against real endpoints using ` cargo test --test live_providers -- --ignored --nocapture`.
- After each iteration run tests to ensure parity with Tachikoma.
- Run Tachikoma from ~/Projects/references/Tachikoma and it's associated tests to cross compare behavior and outputs.


Tachikoma resides in `~/Projects/references/Tachikoma`.
