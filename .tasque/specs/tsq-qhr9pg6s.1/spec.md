# Goal
Ensure assistant output is always materialized into run history/messages on turns without tool calls and on cancellation/abort paths.

# Scope
- roci-core runner engine turn finalization paths
- tool-less turn completion
- cancellation/abort completion paths
- event emission consistency with stored messages

# Acceptance Criteria
- On a turn where model returns assistant content and no tool calls, final assistant message is appended to conversation state.
- On cancel/abort after partial output, finalized assistant message semantics are deterministic and persisted (matching chosen policy in docs/tests).
- No regression in tool-call turns.
- Regression tests cover tool-less and canceled runs.

# Non-Goals
- Session tree/fork persistence work.
