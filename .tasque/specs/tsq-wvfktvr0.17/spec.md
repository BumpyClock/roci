## Overview
Document ask_user behavior and architecture seam for future peer bus.

Primary docs:
- `docs/ARCHITECTURE.md`
- `docs/architecture/ask-user-v1-peer-bus-seam.md`
- any built-in tool list docs that surface available tools

## Constraints / Non-goals
- Keep docs aligned with shipped behavior only.
- Avoid documenting unimplemented peer bus as available feature.

## Interfaces (CLI/API)
- Document core API and CLI demo responsibilities.

## Data model / schema changes
- Document request/response/event payload fields and validation expectations.

## Acceptance criteria
- Fresh agent can implement/operate feature from docs + task specs only.
- Docs clearly separate v1 (blocking parent-mediated) vs future peer bus.

## Test plan
- Manual doc consistency check against code and tests.
