## Overview
Handle design warnings around large function signatures and complex types without changing behavior.

## Constraints / Non-goals
- No broad architectural rewrites.
- Keep public API changes minimal and justified.

## Interfaces (CLI/API)
- Target warnings: `clippy::too_many_arguments`, `clippy::type_complexity`
- Primary files include runner phases and auth/service type-return signatures.

## Data model / schema changes
No persisted schema changes expected.

## Acceptance criteria
- Each signature/type warning is either fixed via local refactor (type aliases/parameter grouping) or explicitly deferred with rationale.
- Refactors preserve call-site behavior and readability.

## Test plan
- Run crate-scoped clippy on touched crates.
- Run relevant unit/integration tests for modified modules.
