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

## Integration and feature-gated tests

- Root integration tests: `cargo test --test meta_crate_integration`
- Core integration tests: `cargo test -p roci-core --test registry_integration`
- MCP tests (feature-gated in `roci-core`): `cargo test -p roci-core --features mcp`
- To inspect test output: append `-- --nocapture`

## Environment

Use `.env` for local secrets and `.env.example` as the template.
