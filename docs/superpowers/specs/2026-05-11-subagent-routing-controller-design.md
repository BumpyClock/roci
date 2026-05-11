# Subagent Routing Controller Design

## Overview

Implement `tsq-r0c1agt6.3`: a core `SubagentRoutingController` plus model-callable subagent management tools. This slice builds the session-scoped control plane over existing `SubagentSupervisor`/profile primitives, without wiring it into `AgentRuntime` or `roci-cli` yet.

This spec narrows the approved full feature design in `docs/superpowers/specs/2026-05-11-subagent-routing-profile-isolation-design.md` to `.3`.

## Constraints / Non-goals

- Do not inject tools into `AgentRuntime` in this slice; `.4` owns runtime visibility and event projection.
- Do not add CLI flags or rendering in this slice; `.5` owns `roci-cli`.
- Do not run live CLI verification in this slice; `.6` owns full tmux/live verification after runtime and CLI wiring exist.
- Do not expose raw child `AgentEvent` as the public runtime event contract.
- Do not enable recursive subagent delegation by default.
- Do not write child session data to project cwd.

## Interfaces

Create a new `agent::subagents::routing` module exported through `agent::subagents`.

Core API:

- `SubagentRoutingController`: session-scoped lifecycle owner for delegation state.
- `SubagentRoutingController::delegate(request, caller)`: resolve profile, spawn child, return either a running handle result or compact completion result.
- `SubagentRoutingController::list_profiles()`: deterministic profile summaries from registry.
- `SubagentRoutingController::list_subagents()`: deterministic known child summaries.
- `SubagentRoutingController::wait_subagent(id)`: return compact cached/completed result.
- `SubagentRoutingController::cancel_subagent(id)`: cancel active child and return status.
- `SubagentRoutingController::send_subagent_message(id, message)`: route parent-to-child steering message for active child.
- `SubagentRoutingTools`: builds model-visible static tools for later runtime injection.

Controller methods accept a `SubagentCaller`:

- Main/default agent callers are allowed to use management tools.
- Child callers are rejected in v1 with clear config/permission errors.
- `depth` is tracked now so later recursive/depth-limited delegation can evolve without changing public method shape.
- Every management method uses the same `authorize_main_agent` check in v1.

Tool names:

- `delegate_subagent`
- `list_subagents`
- `wait_subagent`
- `cancel_subagent`
- `send_subagent_message`

`.3` creates these tools and tests their execution through the `Tool` trait. `.4` decides when they are injected into `AgentRuntime` and emits semantic runtime events.

## Data model / schema changes

Add DTOs to `agent::subagents::types`:

- `DelegateSubagentRequest { profile, task, label, run_in_background }`
- `DelegateSubagentResult { subagent_id, profile_id, status, summary, artifacts, child_thread_id, usage, error }`
- `SubagentArtifact { kind, title, content }`
- `SubagentCaller { is_main_agent, depth, source_subagent_id }`
- `SubagentKnownChild { subagent_id, profile_id, label, status, model }`
- `SubagentCancelResult { subagent_id, status, canceled }`
- `SendSubagentMessageResult { subagent_id, accepted }`

`DelegateSubagentResult.summary` is derived from the last assistant text in child messages. If the child fails, summary is empty and `error` carries the failure. `artifacts` remains empty in v1 unless a later task adds richer extraction.

`child_thread_id`, `usage`, and richer artifacts are nullable/seam fields in `.3`; runtime/session persistence work remains downstream.

`DelegateSubagentRequest.run_in_background` defaults to `false` during JSON deserialization so model calls may send only `{ "task": "..." }`.

The routing controller and supervisor must share one authoritative profile registry. The controller may retain its own registry clone for default lookup/listing, but the public constructor must build the supervisor from the same clone or expose a supervisor registry snapshot so default/list/spawn cannot diverge.

## Behavior

Profile resolution:

- If request includes `profile`, resolve that profile.
- If request omits `profile`, use the single registry default profile.
- If no default profile exists, return a clear error.
- Unknown profile returns a clear error.

Foreground delegation:

- Spawn child through `SubagentSupervisor`.
- Wait for completion.
- Store compact result in controller cache.
- Return compact `DelegateSubagentResult`.

Background delegation:

- Spawn child through `SubagentSupervisor`.
- Store handle in controller state.
- Return compact running `DelegateSubagentResult` with `status = running`.
- Later `wait_subagent` returns completion and caches it.

Cancel:

- Active child cancel calls the stored `SubagentHandle::abort`.
- Unknown child returns error.
- Terminal child returns `canceled = false`.

Send:

- Only main/default caller can send.
- Only active children accept messages in v1.
- Message routes through child runtime steering queue.
- Unknown or terminal child returns clear error.

Main-agent-only enforcement applies to all management operations in this slice: delegate, list profiles, list subagents, wait, cancel, and send.

Tool execution:

- Tool handlers deserialize JSON args via `ToolArguments::deserialize`.
- Tool outputs are JSON serialization of the DTOs.
- Management tools are safe host-control tools, not arbitrary external mutation.

## Test plan

Focused tests:

- Profile listing and default resolution are deterministic.
- Minimal delegate tool args `{ "task": "..." }` deserialize and run foreground by default.
- Delegate without profile uses default profile.
- Delegate without default fails clearly.
- Unknown profile fails clearly.
- Foreground delegate uses fake provider and returns last assistant text as compact summary.
- Background delegate returns running handle and appears in `list_subagents`.
- `wait_subagent` returns and caches compact completion.
- `cancel_subagent` cancels active child and reports status.
- All management methods reject child callers.
- `send_subagent_message` rejects unknown/terminal children.
- Tool wrappers execute through `Tool::execute` and return JSON DTOs.

Commands:

- `cargo test -p roci-core --features agent subagent_routing -- --nocapture`
- `cargo check -p roci-core --features agent,mcp`
- `cargo clippy -p roci-core --features agent,mcp --all-targets -- -D warnings`
- `cargo fmt --all --check`

Live `roci-cli` verification is not required for `.3` because management tools are not injected into runtime until `.4/.5`. `.6` owns full live tmux CLI verification.

## Acceptance criteria

- Controller and DTOs are public under `agent::subagents`.
- Tool surface exists but is not injected globally.
- Tests prove foreground/background/wait/cancel/send/default/error paths.
- Child subagents cannot use management tools by default through caller checks.
- No raw child `AgentEvent` becomes public runtime event contract in this slice.
