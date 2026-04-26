## Overview
Add MCP auth discovery and token lifecycle support at the SDK boundary while keeping interactive UX host-owned.

## Scope
- Add RFC 9728 protected-resource discovery.
- Add RFC 8414 authorization server metadata lookup.
- Add token exchange / refresh primitives and MCP-scoped token persistence.
- Reuse existing `TokenStore` through an MCP-specific adapter keyed by server identity.

## Non-goals
- No baked-in browser/device-code UX.
- No host config file management in this task.
- No reconnect policy beyond surfacing `needs_auth` and refresh outcomes.

## API / contract
- SDK exposes auth discovery + refresh services behind traits/callbacks.
- Hosts provide interactive steps (open browser, receive callback, display device code, approval UI).
- Token storage keys must be stable across reconnects and scoped by server identity, not raw URL alone.

## Acceptance criteria
1. SDK can discover auth metadata for compliant protected MCP resources and honor explicit metadata overrides.
2. Access/refresh tokens can be persisted and refreshed through the shared token-store abstraction.
3. Auth failures are classified into retryable vs terminal (`needs_auth`) outcomes.
4. Tests cover discovery success/failure, refresh success/failure, and token-store round trips.

## Validation
- Unit tests for discovery parsing, token expiry checks, and token-store keying.
- Integration tests with mocked metadata/token endpoints.
