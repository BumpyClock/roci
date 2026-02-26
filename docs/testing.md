# Roci Testing Guide

## Default suite (hermetic)

- Command: `cargo test`
- Network: no external calls
- Runs tests across all workspace crates

### Per-crate testing

```bash
cargo test -p roci-core       # Core SDK kernel (traits, types, config, auth)
cargo test -p roci-providers  # Provider transports
cargo test -p roci            # Meta-crate integration tests
cargo test -p roci-cli        # CLI tests (arg parsing, error formatting)
cargo test -p roci-tools      # Tool tests (25 tests covering all tools)
```

## MCP integration tests

- Command: `cargo test --test mcp_integration_tests --features mcp`
- To inspect request payloads:
  - `cargo test --test mcp_integration_tests --features mcp -- --nocapture`

## Live provider smoke tests

Tests in `tests/live_providers.rs` are ignored by default.

- Command: `cargo test --test live_providers -- --ignored`
- With all providers: `cargo test --all-features --test live_providers -- --ignored`

Required environment variables:
- `OPENAI_API_KEY`
- `GEMINI_API_KEY` (or `GOOGLE_API_KEY`)
- `OPENAI_COMPAT_API_KEY`, `OPENAI_COMPAT_BASE_URL`, `OPENAI_COMPAT_MODEL` (for OpenAI-compatible test)

Optional environment variables:
- `OPENAI_COMPAT_SUPPORTS_JSON_SCHEMA=true` to enable OpenAI-compatible JSON schema live test.

## Environment

Use `.env` for local secrets and `.env.example` as the template.
