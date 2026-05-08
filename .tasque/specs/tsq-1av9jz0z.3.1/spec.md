## Overview
Define the reusable secret redaction API for text and JSON payloads used by approval previews, logs, events, and optional host-owned model-context redaction.

## Scope
- Add `SecretRedactor` utility in `roci-core` with `scan_text`, `redact_text`, and `redact_json` helpers.
- Define `SecretMatch` with kind, range or JSON path, and replacement token.
- Provide default replacement tokens that preserve structure, such as `[REDACTED_API_KEY]`, `[REDACTED_TOKEN]`, and `[REDACTED_SECRET]`.
- Preserve JSON shape when redacting JSON values.

## Decisions
- Redaction is default-on for previews/log/event helper surfaces once integrated.
- Model-visible tool results are not redacted by default; hosts may explicitly opt into model-context redaction later.
- Pi and Codex prior art have weaker generalized redaction coverage, so Roci owns this contract instead of copying a host-specific approach.

## Match semantics
Text ranges are UTF-8 byte offsets into original input. JSON paths use RFC 6901 JSON Pointer. V1 scans/redacts string values only; object keys are preserved. Overlaps resolve by start ascending, end descending, then kind priority: `PrivateKey`, `AuthHeader`, `BearerToken`, `ApiKey`, `EnvSecret`, `GenericSecret`. Keep first non-overlapping match.

## Non-goals
- No secret storage/key management.
- No telemetry pipeline implementation.
- No runner/tool integration beyond defining the API in this task.

## Acceptance criteria
1. Text scanning returns stable matches with secret kind and byte range.
2. Text redaction replaces secrets with stable kind-specific tokens.
3. JSON redaction preserves object/array shape and reports JSON paths for matches.
4. Default patterns cover common API keys, bearer tokens, auth headers, private key blocks, and env-style secret assignments.
5. Tests prove non-secret text remains unchanged and overlapping matches are handled deterministically.

## Validation
- Unit tests for scan/redact text.
- Unit tests for nested JSON redaction and path reporting.
- Fixture tests for common token/key/header/private-key shapes.
