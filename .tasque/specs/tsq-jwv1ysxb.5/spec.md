# Make core provider errors CLI-agnostic + add CLI error mapping

## Scope
- Add typed error variants for missing credentials/config so CLI can map help text.
- Ensure provider errors remain UI-agnostic (no CLI-specific instructions in core).

## Implementation notes
- Add `RociError::MissingCredential { provider }` and `RociError::MissingConfiguration { key, provider }` (or equivalent).
- Map these in CLI layer to user-facing guidance referencing `roci-agent`.

## Acceptance criteria
1) Core errors avoid CLI-specific guidance strings.
2) Missing credential/config errors are typed and machine-mappable.
3) CLI maps typed errors to actionable help text.
4) Provider tests updated for new error types.
