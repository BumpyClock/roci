# roci-agent: Agent Runtime Aligned to pi-mono

## Objective
Ship a `roci-agent` runtime that preserves roci behavior and approvals while aligning eventing, control flow, and tool execution semantics with pi-mono.

## Scope
- Runtime refactor in `src/agent/` + `src/agent_loop/`.
- Add extensible message/event abstractions.
- Extend tool execution API for cancellation + streaming updates.
- Add steering/follow-up/transform hooks.
- Add observable state and public `Agent` orchestration API.
- Add tests + example demonstrating steering and follow-up flows.

## Locked Decisions
1. Refactor in-place; no parallel legacy runtime.
2. Keep approval system; integrate with new control-flow hooks.
3. Keep parallel tool execution with steering checks between batches.
4. Keep backward compatibility for existing `Tool` implementations.
5. Gate new runtime behind `roci-agent` feature flag.

## Non-goals
- Rewriting provider transport layer.
- Removing legacy `RunEvent` stream APIs in this epic.
- UI redesign for agent events.

## Deliverables
1. Type layer (`AgentMessageExt`, `AgentEvent`, tool update payload + callbacks).
2. Loop layer (outer follow-up loop + inner tool loop + steering interrupts).
3. Agent layer (`prompt/continue/steer/follow_up/abort/reset/wait_for_idle`).
4. State layer (subscription/event model).
5. Integration hooks (dynamic API key, session ID propagation).
6. Test matrix + example.

## Parallelization Plan
- Track A (types): `.1`, `.2`, `.3`.
- Track B (loop core): `.4` then `.5/.6/.7`.
- Track C (agent API): `.9.*` after `.5/.6/.7`.
- Track D (integration): `.10/.11/.12/.13` after `.9` (feature flag `.13` can start earlier).
- Track E (quality): `.14.*` then `.15`.

## Definition of Done
- `cargo test` passes with and without `--features roci-agent`.
- New tests cover steering, follow-ups, transform hook, event ordering, validation failures.
- Existing agent/tool flows remain backward compatible.
- Example compiles and demonstrates steering + follow-up behavior.
- All tasks in epic are `planned` with attached specs and explicit dependencies.
