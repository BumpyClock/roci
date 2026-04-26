## Context
Source note: /Users/adityasharma/Projects/roci/.ai_agents/codex_agent_loop.md

## Findings
- chat path registers shell directly without a Codex-style permissions prompt item.
- ApprovalPolicy::Ask currently classifies exec/process as command execution, but builtin shell falls through as Other and is auto-accepted.

## Scope guardrail
- Priority is correctness/safety, not parity.
- Prompt-level sandbox messaging is optional; approval semantics fix is the core requirement.

## Acceptance
- Decide desired approval model for shell/file-changing tools.
- Fix approval classification and add regression tests.
- If useful, document or expose sandbox/permission state more explicitly.
