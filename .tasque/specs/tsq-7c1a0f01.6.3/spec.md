## Overview
Resolve warnings tied to MSRV compatibility and module layout policy.

## Constraints / Non-goals
- Do not silently raise MSRV without explicit project agreement.
- Keep module naming changes low-risk and minimally disruptive.

## Interfaces (CLI/API)
- Target warnings: `clippy::incompatible_msrv`, `clippy::module_inception`
- Primary files include skills/frontmatter and agent module structure.

## Data model / schema changes
No schema changes.

## Acceptance criteria
- MSRV-sensitive code paths are made compliant or intentionally allowed with rationale.
- Module structure warning is resolved or explicitly policy-allowed.

## Test plan
- Re-run clippy for affected crates.
- Run compile/tests for modules impacted by renames or compatibility changes.
