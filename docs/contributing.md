# Contributing

Roci is a Rust workspace with five crates. See `docs/ARCHITECTURE.md` for the
full crate layout and dependency graph.

## Dev Setup

```bash
git clone <repo-url>
cd roci

cargo build
cargo test
```

## Opt-in push checks

`rust-toolchain.toml` pins the workspace MSRV, Rust 1.96.1, with rustfmt and
Clippy. Prepare explicitly with
`rustup toolchain install 1.96.1 --profile minimal --component rustfmt --component clippy`
and `cargo +1.96.1 fetch --locked` before using offline push checks.

Run `scripts/install-hooks.sh` explicitly to select the tracked `.githooks`
directory. It is idempotent and refuses custom hooksPath settings or existing
active hooks. It does not install tools, run validation, or copy hooks into
shared Git directories. No hooks install automatically.

The hook requires a clean checkout and all nondeleted pushed refs to equal
checked-out HEAD. It never stashes or switches refs. Every push runs full
workspace rustfmt, locked offline default-feature `cargo check`, and locked
offline all-feature Clippy. No changed-file pruning is used, so new branches,
unknown bases, empty ranges, renamed/deleted files and shared Cargo changes
cannot skip workspace dependents. Deletion-only pushes skip checks. Missing
prepared tools or offline dependencies fail, without downloads.

`python3 scripts/test_hooks.py` exercises executable hooks and installers in
disposable Git repos with stub Rust commands. Linux real-tool validation is
separate from this harness. Windows/macOS hook execution remains unqualified.
No latency budget is promised. Default/all-feature hermetic tests and live
provider qualification remain separate gates in [testing.md](testing.md).

## Workspace Crates

| Crate | Path | Purpose |
|-------|------|---------|
| `roci` | `src/lib.rs` | Meta-crate: re-exports + default wiring |
| `roci-core` | `crates/roci-core/` | Provider-agnostic SDK kernel |
| `roci-providers` | `crates/roci-providers/` | Built-in provider transports + OAuth |
| `roci-cli` | `crates/roci-cli/` | CLI binary (`roci-agent`) |
| `roci-tools` | `crates/roci-tools/` | Built-in coding tools |

## Adding a New Provider

1. Add the transport implementation in `crates/roci-providers/src/` behind a
   feature flag. Follow the patterns of existing providers (e.g., `openai.rs`).
2. Implement `ProviderFactory` for your provider.
3. Register the factory in `register_default_providers()`.
4. Add the feature flag to `crates/roci-providers/Cargo.toml` and pass it
   through in the root `Cargo.toml`.
5. Add tests in the relevant crate/module (for example, root integration tests in
   `tests/meta_crate_integration.rs` or provider/kernel tests in crate test files).
6. Update `docs/ARCHITECTURE.md` provider table.

## Writing Tests

```bash
cargo test                    # Full workspace (hermetic)
cargo test -p roci-core       # Core SDK only
cargo test -p roci-core --features agent "agent::runtime::tests::"  # AgentRuntime module tests (runtime_tests/*)
cargo test -p roci-providers  # Provider transports only
cargo test -p roci-cli        # CLI tests
cargo test -p roci-tools      # Tool tests
cargo test --test meta_crate_integration     # Root integration tests
cargo test -p roci-core --test registry_integration  # Core integration tests
cargo test -p roci-core --features mcp       # MCP feature-gated tests
```

## Code Style

- Conventional Commits: `feat|fix|refactor|build|ci|chore|docs|style|perf|test`
- Keep files under ~500 LOC; split and refactor proactively.
- Run `cargo fmt` and `cargo clippy` before committing.

## Boundary Rules

- `roci-core`: no provider-specific code, no `clap`, no terminal I/O, no `process::exit`.
- `roci-providers`: depends only on `roci-core`. No CLI concerns.
- `roci-cli`: owns all terminal I/O, exit codes, and user-facing error messages.
- `roci-tools`: standalone tool implementations; depends on `roci` for trait definitions.
