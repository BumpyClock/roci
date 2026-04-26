# Create roci meta-crate for batteries-included DX

## Scope
- Introduce `crates/roci` as a thin re-export over `roci-core` + `roci-providers`.
- Keep current public API surface available from `roci` with minimal path changes.
- Provide a default registry initializer so `roci` behaves like today.

## Acceptance criteria
1) `roci` crate compiles and mirrors existing DX.
2) Users can opt into `roci-core + roci-providers` for explicit wiring.
3) No duplicate provider logic between crates.
