# Learnings

## 2026-01-29: Initial scaffolding
- Renamed from TachikomaError → RociError for crate naming consistency.
- Directory-based modules (`mod.rs` pattern) to support multi-file modules.
- `thiserror` v2 requires edition 2021+; `rust-version = "1.75"` satisfies.
- `reqwest` with `default-features = false` + `rustls-tls` avoids native OpenSSL dep.
- Feature flags gate optional provider modules and capabilities (agent, audio, mcp, cli).
- `bon` v3 for builder pattern — replaces manual builders.
- `strum` 0.27 for enum Display/FromStr derivation.
