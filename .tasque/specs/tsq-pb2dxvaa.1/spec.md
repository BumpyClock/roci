# Extension ABI + config design

## Scope
- Decide Rust-native plugin strategy:
  - Option A: `abi_stable` (Rust ABI-stable dynlib).
  - Option B: `libloading` + C ABI shim.
  - Option C: static registration (compile-time only).
- Define extension manifest/config shape and storage location.
- Define SoC: roci-core exposes ExtensionLoader API with injected roots/config; CLI resolves default paths and reads config files.

## Deliverables
- Written decision (pros/cons + chosen option).
- Minimal interface sketch for extension init + tool/command registration + lifecycle hooks.

## Open
- Config location and precedence (project vs user vs dotfiles).
- Whether extension-specific config lives next to extension or centralized.
