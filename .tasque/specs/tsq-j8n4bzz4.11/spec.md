# Task Spec: config/auth value tests

## Deliverables
- Test `RociConfig::from_env` mapping and precedence behavior.
- Test `AuthValue::resolve` for all variants + failures.

## Acceptance Criteria
- Environment mapping behavior is explicit and regression-safe.
- Auth resolution failures return expected error forms.

## Tests
- Unit tests with temporary env setup/cleanup.
