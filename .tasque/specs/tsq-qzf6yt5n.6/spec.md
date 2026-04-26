## Goal
Provide a first-class patch-editing tool with predictable, grammar-validated behavior.

## Scope
- Add `apply_patch` to `roci-tools`.
- Adopt the Codex/Copilot patch grammar for Add/Delete/Update/Move hunks.
- Parse -> verify -> classify safety -> execute.
- Integrate with SDK approval hooks using the new safety contract.

## Decisions
- Keep generic SDK tool input shape JSON-based for now; the tool accepts a required `patch: string` argument and parses internally.
- v1 limits: no binary patches, chmod, directory ops, or writes outside workspace/root policy.
- Return deterministic structured errors for parse failures, path violations, and mismatched context.

## Acceptance
- Grammar coverage matches begin/end + add/delete/update/move/end-of-file forms.
- Approval path distinguishes read-only impossible / mutating / destructive operations from parsed patch intent.
- Tests cover valid patches, invalid grammar, path escape attempts, and repeated-apply failures.