## Goal
Extract compaction + branch-summary logic from `runtime.rs` into focused summary module while preserving behavior.

## Scope
- Create `crates/roci-core/src/agent/runtime/summary.rs`.
- Move methods/helpers:
  - `compact`
  - `summarize_branch_entries`
  - `split_messages_for_compaction`
  - `count_tokens_before_entry`
  - `legacy_summary_override`
  - `validate_compaction_override`
  - `compact_messages_with_model`
- Keep call sites in runtime facade delegating with no external API changes.

## Risk-sensitive Behavior To Preserve
- Compaction cancellation and override contract semantics.
- `first_kept_entry_id`/`tokens_before` validation errors.
- Tool-result boundary handling (`Role::Tool` constraints).
- Branch summary cancellation/override behavior.

## Acceptance
1. All compaction/summary tests pass after extraction.
2. No changes to generated compaction summary message type/shape.
3. No changes to error strings unless strictly required; if changed, document in task notes.

## Verification Commands
- `cargo test -p roci-core --features agent "agent::runtime::tests::compaction_and_branch_summary::"`
- `cargo test -p roci-core --features agent "agent::runtime::tests::session_before_compact::"`
- `cargo test -p roci-core --features agent "agent::runtime::tests::session_before_tree::"`
