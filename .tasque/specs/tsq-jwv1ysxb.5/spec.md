# Make core provider errors CLI-agnostic + add CLI error mapping

## Scope
- Remove CLI command strings from core errors.
- Introduce typed missing-credential/config errors that CLI can map.

## Implementation notes
- Add new `RociError` variants or structured error payloads:
  - `MissingCredential { provider: ProviderKey, hint: Option<&'static str> }`
  - `MissingConfiguration { key: &'static str, provider: ProviderKey }`
- Update providers (e.g., GitHub Copilot) to use typed errors.
- CLI converts typed errors to help text referencing **`roci-agent`**.

## Acceptance criteria
1) Core errors avoid CLI-specific guidance.
2) CLI maps typed errors to user-friendly instructions.
3) Provider tests updated for new error types.
