# Roci Testing Guide

## MCP integration tests

- Command: `cargo test --test mcp_integration_tests --features mcp`
- To inspect request payloads:
  - `cargo test --test mcp_integration_tests --features mcp -- --nocapture`

## Default suite (hermetic)

- Command: `cargo test`
- Network: no external calls

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
