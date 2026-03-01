# Goal
Broaden hooks from tool-only lifecycle to core run lifecycle points needed pre-extensions.

# Scope
- add `before_agent_start` hook
- add `context`/pre-conversion hook with mutation support
- unify hook dispatch conventions/errors with existing tool hooks

# Acceptance Criteria
- New hook points are available from roci-core AgentRuntime API (not CLI-only).
- Hooks can mutate/override system/context data through typed payloads.
- Hook errors/cancel semantics documented and tested.
- Demo CLI includes at least one visible example hook usage.

# Non-Goals
- Full extension loader/plugin ecosystem.
