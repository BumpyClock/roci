# Task Spec: Auth provider test expansion

## Deliverables
- Add deep tests for ClaudeCodeAuth, GitHubCopilotAuth, OpenAiCodexAuth.
- Cover success, refresh, pending/slowdown/denied, and error mapping paths.

## Acceptance Criteria
- Critical auth flows for all 3 providers are covered.
- Existing auth logic verified without external network dependency.

## Tests
- Provider-specific unit tests with mocked HTTP/filesystem inputs.
- Regression tests for known edge cases.
