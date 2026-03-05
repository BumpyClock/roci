# Sub-agents Plan (Supervisor + Profiles)

## Status
- v1 implemented.
- Implementation epic: `tsq-tb6qdmqm`.

## Core Decision
Implement sub-agents in `roci-core` as a supervisor layer built on top of existing `AgentRuntime`.

Do now:
- core-owned sub-agent supervisor
- named profiles with TOML-backed loading
- ordered model fallback candidates
- prompt / snapshot / prompt+snapshot child input modes
- read-only context propagation
- parent-facing lifecycle + wrapped child events
- shared `UserInputCoordinator` for child `ask_user`
- bounded concurrency and explicit wait/watch APIs

Do not do now:
- generic peer bus
- child-to-child messaging
- persistent child state DB
- session tree/fork integration
- out-of-process child launching as the primary path

## Why This Shape
Current roci already has a working blocking `ask_user` core path:
- `ask_user` tool in `roci-tools`
- `AgentEvent::UserInputRequested`
- `UserInputCoordinator`
- `AgentRuntime::submit_user_input(...)`
- CLI demo host wiring

What roci does not have yet:
- child identity
- child spawn/lifecycle APIs
- named sub-agent profiles
- model fallback selection for children
- read-only context handoff to children
- parent-facing child event stream
- parallel child orchestration helpers

A bus-first rewrite would disturb a working `ask_user` path before we have a second real inter-agent message type. That is not the right cost profile.

## Reference Takeaways
### From `pi-mono`
Relevant files:
- `/Users/adityasharma/Projects/references/pi-mono/packages/coding-agent/README.md`
- `/Users/adityasharma/Projects/references/pi-mono/packages/coding-agent/examples/extensions/subagent/README.md`
- `/Users/adityasharma/Projects/references/pi-mono/packages/coding-agent/examples/extensions/subagent/index.ts`

Useful ideas:
- isolated context per child
- bounded fan-out / bounded concurrency
- role/profile-driven delegation
- parent-visible child progress
- abort propagation

Not worth copying:
- subprocess-per-child as the primary architecture
- sub-agent orchestration only in an extension/UI layer

### From `codex-rs`
Relevant files:
- `/Users/adityasharma/Projects/references/codex/codex-rs/docs/protocol_v1.md`
- `/Users/adityasharma/Projects/references/codex/codex-rs/app-server/src/thread_state.rs`
- `/Users/adityasharma/Projects/references/codex/codex-rs/app-server/src/codex_message_processor.rs`
- `/Users/adityasharma/.codex/config.toml`
- `/Users/adityasharma/.codex/*_config.toml`

Useful ideas:
- one active task per child runtime/thread
- parallelism by many isolated runtimes
- explicit lifecycle APIs
- `RequestUserInput` as a first-class core event
- role-based config files

Not worth copying yet:
- full server/protocol/thread database complexity
- protocol-first architecture for local v1

## Design Rules
- Each sub-agent is its own `AgentRuntime` with exactly one active run at a time.
- Parallelism is achieved by supervising multiple child runtimes concurrently.
- Parent-facing orchestration events are distinct from raw child `AgentEvent`s.
- Sub-agent behavior is driven by named profiles, not only ad-hoc prompt strings.
- Profiles may be built-in or loaded from TOML.
- Each profile may define an ordered list of model candidates across providers.
- Child launch stays behind an internal launcher/factory seam.
- Public APIs should remain launcher-agnostic even if v1 ships only an in-process launcher.
- Context propagation is explicit and read-only.
- The CLI may later demo this, but the design remains harness-first and core-owned.

## Profiles
### Goal
Support Codex-like named agent profiles, but generalized for roci.

### Sources
- built-in profiles shipped by `roci-core`
- project/global TOML-defined profiles loaded by core
- explicit per-spawn overrides

### Discovery roots and precedence
- Use the same config-store/root precedence model as other roci config files.
- Profiles should live in the same config locations/patterns as the rest of the agent config store, not ad-hoc custom paths.
- Project overrides global.
- User-defined profiles can override built-ins with the same name.

### Profile contents
A `SubagentProfile` should be able to define:
- `name`
- `description`
- `kind`
- `system_prompt`
- `tools` policy
- ordered `models` candidate list
- optional defaults for timeouts / metadata
- `version = 1`

### Built-in profiles
Ship a small stable built-in set:
- `developer`
- `planner`
- `explorer`

### Inheritance
- Support single-parent inheritance only.
- Child scalar fields replace parent scalar fields.
- `models` replaces wholesale unless we explicitly add merge semantics later.
- `tools` merge only through `ToolPolicy`, not implicit array merging.

### Model candidates
A profile should support an ordered list of candidates, not one model string.

Example shape:
```toml
name = "developer"
description = "General coding agent"
inherits = "builtin:developer"

[[models]]
provider = "codex"
model = "gpt-5.4"
reasoning_effort = "high"

[[models]]
provider = "openai"
model = "gpt-5.4"
reasoning_effort = "high"

[[models]]
provider = "anthropic"
model = "claude-sonnet-4.5"
reasoning_effort = "medium"

[tools]
mode = "inherit"
add = []
remove = []

system_prompt = """
You are a coding sub-agent...
"""
```

Rules:
- candidate list is ordered by preference
- selection happens at child startup / provider acquisition time
- no mid-run model switching
- actual selected candidate is surfaced in spawned/snapshot events
- fallback only applies to launch-time/provider-acquisition failures such as:
  - missing credentials
  - unsupported model/provider
  - provider creation/startup failure
- no fallback after the child has already started a run and later fails mid-task

### Tool policy
Use explicit policy instead of ad-hoc booleans.

`ToolPolicy` should support:
- `inherit`
- `replace`
- `inherit + add/remove`

Default:
- inherit parent tools
- allow explicit override from profile or spawn request

## Child Input Modes
The parent should be able to spawn a child with:
- prompt only
- snapshot only
- prompt + snapshot

Recommended API surface:
```rust
pub enum SubagentInput {
    Prompt { task: String },
    Snapshot { mode: SnapshotMode },
    PromptWithSnapshot { task: String, mode: SnapshotMode },
}
```

Default behavior:
- parent helpers should prefer `PromptWithSnapshot`
- default helper should prefer `PromptWithSnapshot + SummaryOnly`

### Snapshot modes
```rust
pub enum SnapshotMode {
    SummaryOnly,
    SelectedMessages,
    FullReadonlySnapshot,
}
```

Rules:
- do not automatically pass full parent conversation by default
- parent must choose the snapshot mode explicitly
- snapshot is read-only; child cannot mutate parent conversation state directly
- `SelectedMessages` selection should be explicit in the API; core should not guess “recent N messages”

## Shared Context
Use explicit structured context, not implicit shared mutable state.

Suggested shape:
```rust
pub struct SubagentContext {
    pub summary: Option<String>,
    pub selected_messages: Vec<ModelMessage>,
    pub file_hints: Vec<PathBuf>,
    pub resources: serde_json::Value,
    pub metadata: serde_json::Value,
}
```

Rules:
- snapshot in, results/events out
- no live shared mutable message store between parent and child
- parent may decide how much context to materialize
- `FullReadonlySnapshot` should include materialized conversation/context, not transient runtime internals, live handles, or mutable queues

## Prompt Policy
Profiles are first-class, but prompt injection still matters.

`SubagentPromptPolicy` should add dynamic system instructions such as:
- you are a sub-agent
- do not address the user directly
- use `ask_user` when user input is required
- return concise progress and results to the parent

Users/developers may override or extend the prompt through profile TOML and per-spawn overrides.

## Wait / Watch Model
Support both observational and terminal coordination.

### Observational
- `watch_snapshot()` on the handle
- parent can peek at progress without waiting for completion

### Terminal
- `wait(id)` for one specific child
- `wait_any()` for the next child completion
- `wait_all()` for fan-out/fan-in joins

Why both matter:
- `watch_snapshot()` is for progress-aware orchestration
- `wait*()` is for terminal coordination

## Parent-Facing Events
Use a wrapper event type around child events.

Suggested shape:
```rust
pub enum SubagentEvent {
    Spawned { subagent_id: SubagentId, label: Option<String>, profile: String, model: Option<LanguageModel> },
    StatusChanged { subagent_id: SubagentId, status: SubagentStatus },
    AgentEvent { subagent_id: SubagentId, label: Option<String>, event: AgentEvent },
    Completed { subagent_id: SubagentId, result: SubagentRunResult },
    Failed { subagent_id: SubagentId, error: String },
    Aborted { subagent_id: SubagentId },
}
```

Rule:
- child `AgentEvent::UserInputRequested` is forwarded with `subagent_id` out-of-band through `SubagentEvent`
- do not mutate the `ask_user` tool contract to include parent-only metadata
- `watch_snapshot()` payload should include at least:
  - `subagent_id`
  - `profile`
  - `label`
  - selected model candidate
  - status
  - turn/message progress
  - last error

## ask_user Flow
1. Parent creates `SubagentSupervisor` with a shared `UserInputCoordinator`.
2. Supervisor resolves profile + overrides + model candidate.
3. Supervisor launches child `AgentRuntime` using the shared coordinator.
4. Child executes tools normally.
5. If child calls `ask_user`, runtime emits `AgentEvent::UserInputRequested { request }`.
6. Supervisor forwards this as `SubagentEvent::AgentEvent { subagent_id, ... }`.
7. Parent host renders the question.
8. Parent host calls `SubagentSupervisor::submit_user_input(response)`.
9. Shared coordinator resolves the waiting child tool call.

Why this is enough:
- request/response correlation already exists by `request_id`
- parent only needs child identity for orchestration/UX
- no generic bus required for this path

## Launcher Seam
Use an internal launcher/factory seam.

Purpose:
- keep supervisor orchestration separate from child construction
- make testing easier
- allow future out-of-process or remote child launchers

V1 implementation:
- default in-process launcher backed by `AgentRuntime`

Public API:
- should not expose launcher abstraction unless later proven necessary

## Core Types
Suggested new module area: `crates/roci-core/src/agent/subagents/`

### Identity / profile
- `SubagentId`
- `SubagentKind`
- `SubagentProfile`
- `SubagentProfileRegistry`
- `SubagentProfileRef`
- `ModelCandidate`
- `ToolPolicy`

### Spawn/config
- `SubagentSupervisorConfig`
- `SubagentSpec`
- `SubagentInput`
- `SnapshotMode`
- `SubagentContext`
- `SubagentOverrides`

### Runtime/lifecycle
- `SubagentStatus`
- `SubagentEvent`
- `SubagentSummary`
- `SubagentRunResult`
- `SubagentCompletion`
- `SubagentHandle`
- `SubagentSupervisor`

## API Sketch
```rust
pub struct SubagentSupervisor { /* ... */ }

impl SubagentSupervisor {
    pub fn new(
        registry: Arc<ProviderRegistry>,
        roci_config: RociConfig,
        base_config: AgentConfig,
        supervisor_config: SubagentSupervisorConfig,
        profile_registry: SubagentProfileRegistry,
    ) -> Self;

    pub async fn spawn(&self, spec: SubagentSpec) -> Result<SubagentHandle, RociError>;
    pub async fn abort(&self, subagent_id: SubagentId) -> Result<bool, UnknownSubagent>;
    pub async fn wait(&self, subagent_id: SubagentId) -> Result<SubagentRunResult, UnknownSubagent>;
    pub async fn wait_any(&self) -> Option<SubagentCompletion>;
    pub async fn wait_all(&self) -> Vec<SubagentCompletion>;
    pub async fn submit_user_input(&self, response: UserInputResponse) -> Result<(), UnknownUserInputRequest>;
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<SubagentEvent>;
    pub async fn list_active(&self) -> Vec<SubagentSummary>;
}
```

## Guardrails
- configurable max concurrent children
- configurable default user input timeout
- configurable auto-abort-on-drop / shutdown behavior
- configurable max active children
- child shutdown removes it from active registry
- duplicate terminal completions ignored
- label collisions allowed but IDs are canonical

## Future Peer-Bus Seam
We still want a clean upgrade path, but it belongs above `ask_user`, not inside it.

Preserve these seams now:
- stable child identity
- parent-facing event wrapper
- supervisor-owned orchestration and routing point
- launcher seam
- explicit profile/context model

Future peer bus may add:
- child-to-child delivery
- generic inter-agent message types
- ACLs/quotas
- persistence

That future layer should coexist with the current `ask_user` contract unchanged.

## Resolved Defaults
- Profile discovery/precedence follows the existing roci config-store model.
- Built-in profile set is `developer`, `planner`, `explorer`.
- Profiles are versioned (`version = 1`).
- Inheritance is single-parent only.
- Launch-time fallback only; no mid-run failover.
- Default child helper mode is `PromptWithSnapshot + SummaryOnly`.
- `SelectedMessages` is explicit, not heuristic.
- Spawn tool policy defaults to inherit parent tools unless explicitly overridden.

## Rejected Option: Bus-First Rewrite Now
Pros:
- generic from day one
- explicit message lifecycle/status model
- easier persistence story later

Cons:
- rewrites a working `ask_user` path now
- larger migration surface
- delays delivery of the actual harness API
- adds abstractions before the second message type exists

Recommendation:
- defer

## Implementation Phases
1. Define types: profiles, candidates, input modes, context, supervisor config.
2. Load/resolve profiles from TOML + built-ins.
3. Implement supervisor + handle + launcher seam.
4. Implement snapshot propagation + prompt policy.
5. Implement child event forwarding + `ask_user` reuse.
6. Implement wait/watch/list/guardrails.
7. Add tests/examples/docs.

## Test Matrix
- profile resolution uses built-ins + TOML overrides correctly
- model candidate fallback chooses first viable candidate
- child spawn returns handle immediately while work continues
- prompt-only, snapshot-only, and prompt+snapshot modes all work
- `watch_snapshot()` exposes child progress
- multiple children run concurrently with bounded concurrency
- forwarded events include `subagent_id`
- child `ask_user` reaches parent host and unblocks on response
- timeout/cancel propagate correctly through supervisor path
- `wait_any()` returns the first completed child
- `abort()` stops a running child
- supervisor shutdown aborts active children
