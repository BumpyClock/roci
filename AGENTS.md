# AGENTS.MD

roci notes:
- Default workflow: lint, format, and test before publishing.
- Docs are under `docs/`; read & update as needed.
- Use `.env` for sensitive keys; refer to `.env.example` for structure. Keep `.env.example` updated as needed.
- Validate provider-facing changes with the relevant crate tests; this repo does not currently ship a `live_providers` integration target.
- Use parallel-subagents and agent-teams for all tasks.
- keep roci-cli updated with the changes we make in roci so we can run live tests.
- roci-cli is the example app. roci is the core agent sdk that makes it easy to develop agents. resposibility boundaries for features should be thought of accordingly.
- We are in active development. no users of roci sdk, breaking changes ok and encouraged to get into the right shape. 

# Core
- use `tasque` for persistent task management
- use `rust-skills` for core rust language guidance. 
- use `clippy` for linting and `rustfmt` for formatting. Do so before committing code.

# Testing
- Along with automated tests always run live tests in an interactive tmux terminal to ensure everything is working correctly end to end.
- use local models running at `http://127.0.0.1:1234` if not available, inform user.
- test configured providers as well. work with user to trigger and validate auth flows and make sure calls can be made successfully for OpenAI, OpenAI Codex, Gemini, Anthropic Claude Code, Github Copilot.
- When running test with tmux, always show user the tmux attach command so they can attach to the same tmux session to interact/watch/co-develop or debug
