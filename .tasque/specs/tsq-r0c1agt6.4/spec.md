# Subagent Runtime Wiring Design

## Overview

Implement `tsq-r0c1agt6.4`: wire the `.3` subagent routing controller into `AgentRuntime` so the main/default agent can see management tools and runtime subscribers can observe semantic subagent lifecycle events.

This slice does not add CLI flags or profile loading. `roci-cli` wiring belongs to `.5`; full live tmux verification belongs to `.6`.

## Constraints / Non-goals

- Do not add `roci-cli` flags, rendering, or profile discovery in this slice.
- Do not persist child session files in project cwd.
- Do not expose raw child `AgentEvent` as public `AgentRuntimeEventPayload`.
- Do not enable recursive subagent management tools in child runtimes.
- Do not add peer-to-peer subagent messaging.
- Do not add MCP per-tool/resource filters.

## Interfaces

Add runtime config for subagents to `AgentConfig`:

- `subagents: Option<AgentSubagentConfig>`

`AgentSubagentConfig` includes:

- `profiles: SubagentProfileRegistry`
- `supervisor: SubagentSupervisorConfig`
- `enabled: bool`

`enabled = false` disables management tool injection even if profiles exist. `None` means subagents are not configured.

`AgentRuntime` builds a `SubagentRoutingController` from the same `ProviderRegistry`, `RociConfig`, parent config, supervisor config, and profile registry. The child base config must clear subagent config before controller construction so child runtimes do not receive management tools by default.

Runtime tool resolution must:

- Resolve existing static and dynamic tools as today.
- Add `SubagentRoutingTools` only for the main/default runtime when subagents are configured and enabled.
- Keep tool schema and dispatch aligned by adding actual `Tool` implementations to the same catalog that feeds model-visible schemas and execution.
- Keep child runtimes recursion-disabled by clearing subagent config in `build_child_config`.

## Data model / schema changes

Add semantic runtime payload variants to `AgentRuntimeEventPayload`:

- `SubagentStarted { subagent: SubagentRuntimeSnapshot }`
- `SubagentProgress { subagent: SubagentRuntimeSnapshot, message: Option<String> }`
- `SubagentToolCallStarted { subagent: SubagentRuntimeSnapshot, tool: ToolExecutionSnapshot }`
- `SubagentToolCallCompleted { subagent: SubagentRuntimeSnapshot, tool: ToolExecutionSnapshot }`
- `SubagentMessage { subagent: SubagentRuntimeSnapshot, message: MessageSnapshot }`
- `SubagentNeedsInput { subagent: SubagentRuntimeSnapshot, question: String, context: Option<String> }`
- `SubagentCompleted { subagent: SubagentRuntimeSnapshot, result: DelegateSubagentResult }`
- `SubagentFailed { subagent: SubagentRuntimeSnapshot, error: String }`
- `SubagentCancelled { subagent: SubagentRuntimeSnapshot }`

`SubagentRuntimeSnapshot` includes:

- `subagent_id`
- `profile_id`
- `label`
- `status`
- `model`
- `parent_turn_id`
- `parent_tool_call_id`
- `child_thread_id`
- `source_subagent_id`
- `target_subagent_id`
- `sequence`

`source_subagent_id` and `target_subagent_id` stay optional seams for future peer routing.

## Behavior

Runtime startup:

- If `AgentConfig.subagents` is `Some(config)` and `config.enabled`, create one routing controller for the main runtime.
- Store controller on `AgentRuntime`.
- Subscribe to controller/supervisor events and project them into public semantic runtime events.

Tool visibility:

- Main/default runtime receives `delegate_subagent`, `list_subagents`, `wait_subagent`, `cancel_subagent`, and `send_subagent_message`.
- Child runtimes do not receive these tools because `build_child_config` clears subagent runtime config.
- If subagents are disabled, no management tools are injected.

Semantic events:

- Translate child lifecycle events to `SubagentStarted`, `SubagentCompleted`, `SubagentFailed`, `SubagentCancelled`.
- Translate child message/tool events to semantic subagent message/tool variants.
- Translate child human input request events to `SubagentNeedsInput`.
- Do not project `SubagentEvent::AgentEvent` or raw child `AgentEvent` directly into `AgentRuntimeEventPayload`.
- Publish semantic events through the same runtime event store/broadcast path as other chat runtime events.

## Test plan

Focused tests:

- Runtime with subagents enabled exposes management tools.
- Runtime with subagents disabled does not expose management tools.
- Child config built by `build_child_config` clears subagent config and does not recurse.
- Tool schemas and dispatch include injected management tools.
- Runtime projects `SubagentStarted` and terminal events for a fake subagent run.
- Runtime projects child tool/message events as semantic subagent events without exposing raw `AgentEvent`.
- Runtime projects child human input request as `SubagentNeedsInput`.

Commands:

- `cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture`
- `cargo test -p roci-core --features agent subagent_routing -- --nocapture`
- `cargo check -p roci-core --features agent,mcp`
- `cargo clippy -p roci-core --features agent,mcp --all-targets -- -D warnings`
- `cargo fmt --all --check`
- `git diff --check`

Live `roci-cli` verification is deferred to `.6` because `.4` does not load CLI profile files or render events.

## Acceptance criteria

- `AgentRuntime` can inject management tools when configured.
- Child runtimes are recursion-disabled by default.
- Semantic runtime payloads exist for subagent lifecycle/message/tool/input events.
- Raw child `AgentEvent` is not a public runtime event payload.
- Automated tests prove enabled/disabled tool visibility and event projection.
