# Implement ProviderRegistry in roci-core

## Scope
- Add a registry type that maps provider keys (string/ProviderKey) to factory closures.
- Update runtime/provider creation to consult registry first (including `LanguageModel::Custom`).
- Keep `ModelProvider` as the core trait; no breaking changes to the trait.

## Acceptance criteria
1) Registry supports registration + lookup by provider name.
2) Custom provider can be registered and used for `somecloud:my-model` selectors.
3) Tests cover registry selection and error on missing provider.
