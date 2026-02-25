# Task Spec: AuthManager credentials.json persistence

## Deliverables
- Implement load/save for `credentials.json`.
- Atomic write flow and secure file permissions.
- Validation + corruption recovery behavior.

## Acceptance Criteria
- Round-trip persistence for all `AuthValue` variants.
- Corrupt file is handled with clear error path.
- File permissions are restricted on supported platforms.

## Tests
- Round-trip + overwrite tests.
- Corrupt JSON and missing file behavior tests.
