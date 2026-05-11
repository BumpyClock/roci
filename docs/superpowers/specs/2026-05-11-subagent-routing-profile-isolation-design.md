# Subagent Routing Profile Isolation Design

## Overview

Add a public `agent profile` contract for subagent routing and isolation. For `tsq-r0c1agt6.2`, implement only the profile data model, validation, projection boundaries, and pure tool/MCP isolation contract that downstream controller, runtime, CLI, and verification tasks consume.

## Context

Roci already has subagent execution pieces, MCP identity/namespacing, model candidates, retry/health, and security primitives. The next P0 feature is `tsq-r0c1agt6`, which adds a user-facing subagent routing and custom-agent selection surface.

This design records the shared product and architecture decisions for the full feature, while implementation is split across existing tasks:

- `tsq-r0c1agt6.2`: agent profile model, validation, projection boundaries, and per-subagent tool/MCP isolation contract.
- `tsq-r0c1agt6.3`: routing controller and subagent management/delegate tools.
- `tsq-r0c1agt6.4`: runtime tool visibility, child runtime wiring, and semantic subagent events.
- `tsq-r0c1agt6.5`: roci-cli profile loading, selection, and rendering.
- `tsq-r0c1agt6.6`: docs and automated plus live verification.

The design draws from Pi, Codex, and Claude Code:

- Pi shows the ergonomics of profile-driven subprocess delegation, but its event stream is mostly tool-detail based.
- Codex shows strong controller, registry, depth, role/config overlay, and parent-visible collaboration item patterns.
- Claude Code shows the best public "agent" ergonomics, semantic task/subagent events, and strict runtime isolation.

## Goals

- Use one public-facing concept: `agent profile`.
- Keep internal boundaries clean so Roci can later evolve toward a Codex-style profile/role split without breaking user-facing UX.
- Make per-subagent tool/MCP isolation a real runtime contract, not prompt-only guidance.
- Provide a v1 background subagent UX with handles, wait/list/cancel/send operations, and semantic events.
- Keep parent model context compact. Subagent event visibility is an observability lane, not automatic context injection.

## Non-Goals

- No peer-to-peer subagent bus in v1.
- No recursive child delegation by default in v1.
- No MCP per-tool/resource filters in v1.
- No rich file/link artifact schema in v1.
- No direct child-to-user prompt modal in v1.
- No raw child `AgentEvent` exposure as public runtime contract.
- No persisted global profile selection in v1.

## Public UX

Users see "agent profiles" in CLI/docs/config.

Canonical v1 TOML shape:

```toml
[subagents.scout]
display_name = "Scout"
infer = "Use for repo search and orientation"
model = "openai:gpt-4o"
tools = ["grep", "read_file"]
excluded_tools = ["shell"]
default_agent_excluded_tools = ["apply_patch"]
skills = ["rust-skills"]
mcp_servers = ["github"]
default = true

prompt = """
You are Scout. Read broadly. Do not edit files.
"""
```

`subagents.<id>` table name is the profile id. Profile ids must be non-empty,
unique after all config sources are loaded, and stable enough to store in
subagent events/results.

V1 public fields are:

- `display_name: Option<String>`
- `infer: Option<String>`
- `model: Option<String>` using `provider:model` syntax
- `prompt: Option<String>`
- `tools: Option<Vec<String>>`
- `excluded_tools: Vec<String>`
- `skills: Vec<String>`
- `mcp_servers: Vec<String>`
- `default_agent_excluded_tools: Vec<String>`
- `default: bool`

Existing internal `SubagentProfile` / TOML profile support may remain as a
compat layer, but new public docs/tests for `tsq-r0c1agt6.2` must target the
`[subagents.<id>]` shape above. Conversion into existing runtime types must be
explicit: `prompt` maps to the runtime system prompt field, and `model` maps to
a single model candidate. Multi-model fallback remains existing internal API
unless a later task exposes it publicly.

V1 uses TOML only with an optional inline multiline `prompt` field. There is no `prompt_file` or markdown/frontmatter parser in v1. This keeps loading, validation, and path security small. A future `prompt_file` can be added if prompts become too large for TOML.

`roci chat --profile scout` applies a profile for the current invocation/session only. Profile selection does not persist across future sessions in v1.

## Interfaces (CLI/API)

The public configuration interface for this design is the `[subagents.<id>]` TOML table. Runtime/API consumers should not apply this public artifact directly. They should consume validated `AgentProfile` values and explicit `MainAgentProjection` / `SubagentProjection` outputs.

Downstream CLI and tool APIs are expected to expose profile selection and subagent management, but `.2` only defines the profile/projection contract those APIs depend on.

## Internal Boundaries

The public profile artifact should not be applied directly everywhere. Runtime code should use projections:

- `AgentProfile`: loaded and validated public config artifact.
- `MainAgentProjection`: how a profile changes the main/default agent.
- `SubagentProjection`: how a profile changes child runtime construction.
- `SubagentRoutingController`: session-scoped registry and lifecycle owner for profiles and running subagents.

This keeps a future Codex-style split possible. If Roci later introduces public "roles", existing `agent profile` files can remain valid while roles become overlays or specialized projections.

## Data model / schema changes

The schema change for `.2` is the canonical `[subagents.<id>]` TOML shape and related Rust data structures. Existing internal profile support may remain as compatibility code, but new public tests and docs target the canonical shape.

Projection types should make scope visible in the type system:

- `AgentProfile`: validated public config artifact.
- `MainAgentProjection`: main/default runtime prompt, model, tool exclusions, skills, and explicit MCP server ids.
- `SubagentProjection`: child runtime prompt, model, native tool projection, skills, and explicit MCP server ids.

## Field Scope

Profile fields must have explicit projection scope:

- `display_name`: main and subagent display.
- `infer`: routing/tool-description hint only; no automatic routing in v1.
- `model`: main and subagent projection.
- `prompt`: main and subagent projection.
- `tools`: main and subagent projection.
- `excluded_tools`: main and subagent projection.
- `skills`: main and subagent projection.
- `mcp_servers`: main and subagent projection; MCP is explicit opt-in wherever a profile is projected. Child runtimes never inherit parent/default MCP servers implicitly.
- `default_agent_excluded_tools`: parent/default agent only.
- `default`: default delegate target when `delegate_subagent` omits `profile`.

There may be zero or one default profile. If `delegate_subagent` omits `profile`, the controller uses the configured default profile when available, otherwise returns a clear error.

## Tool And MCP Isolation

Native tools and MCP have different defaults:

- Native/built-in tools inherit from the parent/default visible set unless the profile restricts them.
- MCP servers are explicit opt-in through `mcp_servers`.

Native tool projection is deterministic:

- Let `base_native_tools` be the tool set already visible to the parent/default
  runtime before applying an agent profile.
- If `tools` is absent, start from `base_native_tools`.
- If `tools` is present, start from `base_native_tools ∩ tools`; profile
  projection must not grant tools hidden from the parent/default runtime.
- Remove every entry in `excluded_tools`.
- If a tool appears in both `tools` and `excluded_tools`, fail profile
  validation/resolution with a configuration error.
- Unknown native tool names fail at projection time, when the tool catalog is
  available.

`default_agent_excluded_tools` affects only the parent/default runtime
projection. It is never copied into `SubagentProjection`. The projection API
must make this separation visible so downstream runtime and CLI tasks cannot
accidentally apply the field to child runtimes.

Hidden or denied tools must be removed from both:

- model-visible schema/specification
- dispatch/execution path

Profile conflicts are invalid config. If a tool appears in both `tools` and `excluded_tools`, or in conflicting projection-specific allow/deny fields, profile loading or resolution must fail early with a clear error.

MCP references are server-level in v1:

```toml
mcp_servers = ["github"]
```

The child receives all exposed tools/resources from those servers. Per-tool/resource filters are tracked as a follow-up (`tsq-r0c1agt6.7`). V1 must use stable MCP server identity from the existing MCP namespacing contract and must not parse exposed tool names ad hoc.

MCP validation/projection phases:

- Profile parse validates `mcp_servers` entries are non-empty, unique server ids.
- Main and subagent projections resolve each entry against configured MCP aggregate
  servers and fails on unknown ids.
- Projection/routing uses structured identity from the MCP contract:
  `McpToolIdentity::Mcp { server_id, tool_name }` for tools and structured
  `{ server_id, uri }` identity for resources.
- Model-visible exposed names such as `mcp__<server_id>__<tool_name>` may be
  displayed, but runtime filtering/routing must not parse them.

## Subagent Management Surface

The main/default agent receives model-visible management tools in v1:

- `delegate_subagent`
- `list_subagents`
- `wait_subagent`
- `cancel_subagent`
- `send_subagent_message`

Child subagents do not receive these tools by default. This prevents recursive agent trees before depth, permission, and peer-routing guardrails are mature. The controller should still include depth and permission checks so recursive/depth-limited delegation can be enabled later per profile.

`send_subagent_message` is parent-to-child only in v1. It is handle-based and addressed to a `subagent_id`. It supports follow-up, steering, and answering child questions. It is not an open peer-to-peer broadcast mechanism.

## Runtime Flow

Foreground delegation:

1. Parent model calls `delegate_subagent { profile?, task, run_in_background=false }`.
2. Controller resolves the profile, using the default when omitted.
3. Runtime builds child config from `SubagentProjection`.
4. Child runs to completion.
5. Parent model receives a compact `DelegateSubagentResult`.

Background delegation:

1. Parent model calls `delegate_subagent { profile?, task, run_in_background=true }`.
2. Controller starts child and returns a handle: `subagent_id` plus running status.
3. Parent/user sees semantic events through the observability lane.
4. Parent can use `list_subagents`, `wait_subagent`, `cancel_subagent`, and `send_subagent_message`.

Background subagents must never become orphaned invisible work. Every background run has a handle and emits parent-visible semantic events.

## Child Questions

Subagents can surface questions, but not directly prompt the user in v1.

Flow:

1. Child emits `SubagentNeedsInput { subagent_id, question, context }`.
2. Parent-visible event stream shows the need for input.
3. Main agent or user answers through `send_subagent_message`.

This keeps the main session as the control point and leaves room for richer prompt mediation later.

## Events And Context

There are two channels.

### Observability Lane

Semantic runtime events are parent-visible for UI, CLI, SDK consumers, and debugging:

- `SubagentStarted`
- `SubagentProgress`
- `SubagentToolCallStarted`
- `SubagentToolCallCompleted`
- `SubagentMessage`
- `SubagentNeedsInput`
- `SubagentCompleted`
- `SubagentFailed`
- `SubagentCancelled`

Each event should carry enough identity to correlate parent and child work:

```rust
subagent_id
profile_id
parent_turn_id
parent_tool_call_id
child_thread_id
source_subagent_id
target_subagent_id
sequence
timestamp
```

`source_subagent_id` and `target_subagent_id` are optional in v1. They create a seam for later inter-agent communication without enabling direct peer routing now.

### Model Context Lane

The parent model does not receive every child event. By default it receives only the final compact result from `delegate_subagent` or `wait_subagent`.

```json
{
  "subagent_id": "sub_123",
  "profile_id": "scout",
  "status": "completed",
  "summary": "Found the relevant runtime and MCP wiring.",
  "artifacts": [
    {
      "kind": "text",
      "title": "Key evidence",
      "content": "runtime.rs owns child startup; mcp/aggregate.rs owns server identity."
    }
  ],
  "child_thread_id": "thread_456",
  "usage": null,
  "error": null
}
```

This prevents context spam and avoids accidental steering from child internals.

## Results And Artifacts

`DelegateSubagentResult` v1 includes:

- `subagent_id`
- `profile_id`
- `status`
- `summary`
- `artifacts`
- `child_thread_id`
- `usage`
- `error`

Artifacts are minimal text artifacts in v1:

```rust
pub struct SubagentArtifact {
    pub kind: String,
    pub title: String,
    pub content: String,
}
```

The `artifacts: Vec<_>` shape and stable `kind` field make it easy to evolve toward richer file/link/structured artifacts later (`tsq-r0c1agt6.9`) without designing storage or attachment systems now.

## Persistence

Child thread/session persistence is conditional:

- If the app/runtime has configured session storage, persist the child thread linked by `child_thread_id` and `parent_tool_call_id`.
- If no session store is configured, keep child state in memory only.

Subagent session data must never be written to project cwd by default. App-owned storage decides where durable session data lives.

## CLI Shape

CLI implementation belongs to `tsq-r0c1agt6.5`, but the v1 design expects:

- `roci chat --profile <id>` applies a profile for this invocation.
- `roci chat --no-subagents` disables management tool injection.
- `roci chat --list-agents` or equivalent lists loaded profiles.
- CLI renders semantic subagent lifecycle/progress events compactly.

The exact flag names can be aligned with existing CLI conventions during `.5`, but the public concept should remain `agent profile`.

## Follow-Ups

Created follow-up tasks:

- `tsq-r0c1agt6.7`: per-tool and resource MCP filters.
- `tsq-r0c1agt6.8`: opt-in full child event debug projection.
- `tsq-r0c1agt6.9`: richer subagent artifacts for files, links, and structured outputs.
- `tsq-r0c1agt6.10`: inter-agent communication and peer routing seam.

## Acceptance Criteria

For `tsq-r0c1agt6.2`:

- TOML parse tests cover all profile fields, multiline `prompt`, default profile, and conflict rejection.
- Projection tests prove field scope for main vs child runtimes.
- Isolation contract tests prove native tool inheritance, explicit MCP opt-in,
  and hidden-tool removal from a pure model-visible/tool-dispatch projection
  over a fake tool catalog. Full runtime dispatch enforcement belongs to
  `tsq-r0c1agt6.4`.
- MCP isolation tests use stable server identity, not exposed-name parsing.
- Public structs and errors make invalid profile conflicts clear.

For downstream tasks:

- Controller tests cover list/default resolve, foreground, background, wait, cancel, send, unknown profile, depth, and main-agent-only enforcement.
- Runtime tests prove semantic events are emitted and raw child `AgentEvent` is not public.
- Runtime tests prove `SubagentNeedsInput` routes through the parent/send path.
- CLI tests cover profile load, profile selection, disabled subagents, listing, and event rendering.
- Live tmux verification proves `roci-cli` can load a profile, delegate to a child, show semantic events, and return a compact result to the parent.

## Test plan

For `.2`, run focused core tests first:

- `cargo test -p roci-core --features agent subagent_profile`
- `cargo test -p roci-core --features agent subagent_projection`
- `cargo test -p roci-core --features agent subagent_isolation`

Then run package gates:

- `cargo fmt --all --check`
- `cargo check -p roci-core --features agent,mcp`
- `cargo clippy -p roci-core --features agent,mcp --all-targets -- -D warnings`

Downstream `.3-.6` tasks own live tmux CLI verification for controller/runtime/CLI behavior.

## Self-Review Notes

This design intentionally keeps the first implementation slice narrow. `tsq-r0c1agt6.2` should not implement the full management tool surface or CLI. It should produce the profile/projection/isolation contract that downstream tasks can consume.
