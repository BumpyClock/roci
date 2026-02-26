# ADR: CLI / Core Separation of Concerns

## Status

Accepted

## Date

2026-02-25

## Context

Roci started as a single crate with CLI embedded via feature gates. As the SDK surface grew, the CLI coupling created problems:

- `clap` dependency leaked into the library API surface.
- `process::exit` calls in core auth flows prevented clean SDK consumption.
- CLI-specific error messages lived in provider code, making errors UI-opinionated.
- Built-in coding tools were tightly coupled to core, forcing all consumers to carry tool dependencies.

Non-CLI consumers (other Rust crates, WASM targets, embedded agents) could not use `roci` as a pure library without pulling in terminal concerns.

## Decision

Split into three crates with strict ownership boundaries:

1. **`roci`** -- pure SDK/library crate. No `clap`, no terminal I/O, no `process::exit`. Exposes only SDK modules.
2. **`roci-cli`** -- CLI consumer crate. Owns the `roci-agent` binary, all terminal I/O, exit codes, and user-facing error text.
3. **`roci-tools`** -- built-in coding tools (shell, read_file, write_file, list_directory, grep). Import path: `roci_tools::builtin`.

### Approved decisions (from tsq-jwv1ysxb.1)

| # | Decision | Detail |
|---|----------|--------|
| 1 | Breaking change policy | Remove `roci::cli` module and `cli` feature immediately. |
| 2 | Workspace layout | Root crate `roci` + `crates/roci-cli` member. |
| 3 | CLI crate name | `roci-cli`. |
| 4 | Binary name | `roci-agent` only (no `roci` alias). |
| 5 | Built-in tools extraction | Do now; new crate `roci-tools`. |
| 6 | Built-in tools import path | `roci_tools::builtin` (no compatibility shim in core). |
| 7 | Auth API shape | Low-level primitives + `AuthService` facade returning typed states (`AuthStep`, `AuthPollResult`, `AuthError`). |
| 8 | Error taxonomy | Typed `MissingCredential`/`MissingConfiguration` variants; CLI maps to help text. |
| 9 | Docs | Final-state docs only; no upgrade/migration notes (clean break). |

## Consequences

- **Binary renamed** from `roci` to `roci-agent`.
- **`roci::cli` module removed**; `cli` feature removed from core crate.
- **Auth flows** return typed `AuthStep`/`AuthPollResult`/`AuthError`; CLI maps these to interactive prompts and exit codes.
- **Core errors** use typed `MissingCredential`/`MissingConfiguration` variants; CLI maps to human-readable help text.
- **Built-in tools** accessed via `roci_tools::builtin::all_tools()`; no compatibility shim remains in core.
- **No upgrade/migration notes** -- this is a clean break, not a deprecation cycle.

## Ownership Boundaries

| Concern | `roci` (core) | `roci-cli` | `roci-tools` |
|---------|---------------|------------|---------------|
| Provider abstractions | Owns | Imports | -- |
| Agent loop / streaming | Owns | Imports | -- |
| Auth primitives (`AuthService`, typed states) | Owns | Maps to prompts + exit codes | -- |
| Error types (`MissingCredential`, `MissingConfiguration`) | Owns | Maps to help text | -- |
| CLI arg parsing (`clap`) | -- | Owns | -- |
| Terminal I/O (stdout, stderr, spinners) | -- | Owns | -- |
| Exit codes / `process::exit` | -- | Owns | -- |
| User-facing error messages | -- | Owns | -- |
| Binary (`roci-agent`) | -- | Owns | -- |
| Built-in tools (shell, read_file, etc.) | -- | -- | Owns |
| Tool registry (`builtin::all_tools()`) | -- | Calls | Owns |
| MCP server/client transport | Owns | Configures | -- |

## Related

- Parent epic: `tsq-jwv1ysxb` (Strict SoC migration)
- Decisions spec: `tsq-jwv1ysxb.1`
