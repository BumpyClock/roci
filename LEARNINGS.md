# Learnings

## 2026-01-29: Initial scaffolding
- Renamed from TachikomaError → RociError for crate naming consistency.
- Directory-based modules (`mod.rs` pattern) to support multi-file modules.
- `thiserror` v2 requires edition 2021+; `rust-version = "1.75"` satisfies.
- `reqwest` with `default-features = false` + `rustls-tls` avoids native OpenSSL dep.
- Feature flags gate optional provider modules and capabilities (agent, audio, mcp, cli).
- `bon` v3 for builder pattern — replaces manual builders.
- `strum` 0.27 for enum Display/FromStr derivation.

## 2026-01-30: Provider parity notes
- GPT-5 sampling params only valid for gpt-5.2 with `reasoning_effort = none`; other GPT-5 models reject `temperature` and `top_p`.
- Gemini function calls may include `thoughtSignature`; preserve it on tool call round-trips.

## 2026-01-30: GPT-5 verbosity + Gemini tool role
- GPT-5 family supports Responses API `text.verbosity` via `GenerationSettings.text_verbosity`.
- Gemini tool responses should use role "tool" with `functionResponse` parts.

## 2026-01-30: Live tool coverage
- Live provider tests now include tool-call flows per provider.
