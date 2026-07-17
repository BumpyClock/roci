# Subagent Runtime Wiring Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire subagent routing into `AgentRuntime` so the main runtime can expose management tools and emit semantic subagent events.

**Architecture:** Add an optional subagent runtime config to `AgentConfig`. `AgentRuntime::new` creates one routing controller when enabled, `resolve_tools_for_run` injects `SubagentRoutingTools`, and a small event bridge maps supervisor events into public semantic runtime payloads. Child runtime config clears subagent settings to keep recursive delegation disabled in v1.

**Tech Stack:** Rust, tokio, existing `AgentRuntime`, `ChatProjector`, `SubagentRoutingController`, `SubagentRoutingTools`.

---

## File Structure

- Modify: `crates/roci-core/src/agent/runtime/config.rs`
  Add `AgentSubagentConfig` and `AgentConfig.subagents`.
- Modify: `crates/roci-core/src/agent/runtime.rs`
  Store optional routing controller and initialize it.
- Modify: `crates/roci-core/src/agent/runtime/run_loop.rs`
  Apply main/default profile tool projection and inject routing tools during tool resolution when enabled.
- Modify: `crates/roci-core/src/agent/runtime/chat/event.rs`
  Add semantic subagent runtime payload DTOs.
- Modify: `crates/roci-core/src/agent/runtime/chat/projector.rs`
  Add projection helper for subagent runtime payloads.
- Create: `crates/roci-core/src/agent/runtime/subagent_events.rs`
  Translate `SubagentEvent` to semantic runtime events and publish them.
- Modify: `crates/roci-core/src/agent/runtime.rs`
  Add `mod subagent_events;` to the runtime root module.
- Modify: `crates/roci-core/src/agent/subagents/routing.rs`
  Add controller event subscription and parent tool call metadata capture.
- Modify: `crates/roci-core/src/agent/subagents/routing_tools.rs`
  Capture `ToolExecutionContext.tool_call_id` for parent tool call correlation.
- Modify: `crates/roci-core/src/agent/subagents/handle.rs`
  Expose child thread id from child runtime handle.
- Modify: `crates/roci-core/src/agent/subagents/launcher.rs`
  Clear subagent config in child configs.
- Test: existing `runtime_tests` plus focused new tests under appropriate runtime test module.

### Task 1: Runtime Config And Child Recursion Guard

**Files:**
- Modify: `crates/roci-core/src/agent/runtime/config.rs`
- Modify: `crates/roci-core/src/agent/subagents/launcher.rs`

- [ ] **Step 1: Write failing config tests**

Add tests named with `subagent_runtime_wiring` that assert:

- `AgentConfig::default().subagents.is_none()`
- `build_child_config` clears `subagents`

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture`

Expected: fail because config field does not exist.

- [ ] **Step 3: Implement config types**

Add:

```rust
#[derive(Clone)]
pub struct AgentSubagentConfig {
    pub profiles: crate::agent::subagents::SubagentProfileRegistry,
    pub supervisor: crate::agent::subagents::SubagentSupervisorConfig,
    pub enabled: bool,
    pub main_profile: Option<crate::agent::subagents::SubagentProfileRef>,
}
```

Add `pub subagents: Option<AgentSubagentConfig>` to `AgentConfig`, default `None`.

- [ ] **Step 4: Clear child subagent config**

In `build_child_config`, set `subagents: None`.

- [ ] **Step 5: Verify**

Run: `cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture`

Expected: config tests pass.

### Task 2: Runtime Controller Construction, Tool Projection, And Tool Injection

**Files:**
- Modify: `crates/roci-core/src/agent/runtime.rs`
- Modify: `crates/roci-core/src/agent/runtime/run_loop.rs`

- [ ] **Step 1: Write failing runtime tool tests**

Add tests named:

- `subagent_runtime_wiring_enabled_runtime_exposes_management_tools`
- `subagent_runtime_wiring_disabled_runtime_hides_management_tools`
- `subagent_runtime_wiring_default_agent_excluded_tools_hide_schema_and_dispatch`
- `subagent_runtime_wiring_injected_delegate_tool_executes`

Use fake provider/profile registry and `AgentRuntime::resolve_tools_for_run()` from runtime tests.

- [ ] **Step 2: Run failing tests**

Run: `cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture`

Expected: fail because no runtime controller/tool injection exists.

- [ ] **Step 3: Add runtime controller field**

Add to `AgentRuntime`:

```rust
subagent_controller: Option<Arc<crate::agent::subagents::SubagentRoutingController>>,
```

In `new_inner`, if `config.subagents` is `Some` and enabled, build child base config from `config.clone()` with `subagents = None`, then create `SubagentRoutingController::new(...)`.

- [ ] **Step 4: Inject tools**

In `resolve_tools_for_run`, after merging static/dynamic tools, if controller exists, extend the catalog with `SubagentRoutingTools::new(controller.clone()).tools()` using `ToolOrigin::Custom` and first-wins insertion.

- [ ] **Step 5: Apply default-agent tool projection**

Before resolving the final catalog, apply main/default profile tool projection to the same `ToolVisibilityPolicy` used for schema and dispatch:

- Choose `AgentSubagentConfig.main_profile` when set.
- Else use registry default profile when set.
- Else leave parent/default tool policy unchanged.
- Use existing projection helpers over the currently available native tool names.
- Combine projection output with existing `ToolVisibilityPolicy` by allowing only projected names when a parent/default profile applies.
- Assert denied tools cannot be returned by `resolve_tools_for_run`.

- [ ] **Step 6: Verify**

Run:

```bash
cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture
cargo test -p roci-core --features agent subagent_routing -- --nocapture
```

Expected: pass.

### Task 3: Semantic Runtime Event DTOs

**Files:**
- Modify: `crates/roci-core/src/agent/runtime/chat/event.rs`
- Modify: `crates/roci-core/src/agent/runtime/chat/projector.rs`

- [ ] **Step 1: Write failing payload serde/projector tests**

Cover all payload variants at least through serde roundtrip, and one projector record helper.

- [ ] **Step 2: Add DTOs**

Add `SubagentRuntimeSnapshot`, `SubagentMessageSnapshot`, and `SubagentToolCallSnapshot` plus subagent payload variants listed in spec. Do not reuse parent `MessageSnapshot` or `ToolExecutionSnapshot` for child events in this slice.

- [ ] **Step 3: Add projector record helper**

Add `ChatProjector::record_subagent_event(turn_id, payload)` that accepts only subagent payload variants and emits an `AgentRuntimeEvent`.

- [ ] **Step 4: Verify**

Run: `cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture`

Expected: pass.

### Task 4: Subagent Event Bridge

**Files:**
- Create: `crates/roci-core/src/agent/runtime/subagent_events.rs`
- Modify: `crates/roci-core/src/agent/runtime.rs`
- Modify: `crates/roci-core/src/agent/subagents/routing.rs`
- Modify: `crates/roci-core/src/agent/subagents/routing_tools.rs`
- Modify: `crates/roci-core/src/agent/subagents/handle.rs`

- [ ] **Step 1: Write failing bridge tests**

Cover:

- `SubagentEvent::Spawned` -> `SubagentStarted`
- `SubagentEvent::Completed` -> `SubagentCompleted`
- `SubagentEvent::Failed` -> `SubagentFailed`
- `SubagentEvent::Aborted` -> `SubagentCancelled`
- child `AgentEvent::ToolExecutionStart/End` -> semantic subagent tool events with subagent-specific snapshots
- child `AgentEvent::MessageStart/End` -> semantic subagent message events with subagent-specific snapshots
- child `AgentEvent::HumanInteractionRequested` -> `SubagentNeedsInput`
- no raw child `AgentEvent` appears in `AgentRuntimeEventPayload`

- [ ] **Step 2: Add controller subscription/context seams**

- Add `SubagentRoutingController::subscribe() -> broadcast::Receiver<SubagentEvent>` delegating to supervisor.
- Capture `ToolExecutionContext.tool_call_id` in routing tool handlers and pass it to controller as parent metadata.
- Store parent tool call id in child routing records.
- Expose `SubagentHandle::child_thread_id()` from child runtime `default_thread_id()`.
- Add `SubagentRoutingController::metadata(subagent_id) -> Option<SubagentRoutingMetadata>` with profile id, label, model, parent tool call id, child thread id, and optional source/target ids.

- [ ] **Step 3: Implement mapper**

Implement pure mapping helpers first. Keep child message/tool mapping semantic;
preserve the full `HumanInteractionRequest` for input events. Do not serialize
the surrounding raw `AgentEvent`.
The mapper must read `SubagentRoutingMetadata` from the controller, maintain a
bridge-local `HashMap<SubagentId, u64>` sequence counter, increment sequence per
mapped event, and populate `SubagentRuntimeSnapshot.sequence`.

Add bridge tests asserting:

- `parent_tool_call_id` is present for events created from management tool execution.
- `child_thread_id` is present after spawn.
- sequence increments monotonically for multiple events from the same subagent.
- child tool/message payloads use `SubagentToolCallSnapshot` / `SubagentMessageSnapshot`.

- [ ] **Step 4: Subscribe/publish**

When runtime controller exists, subscribe to controller/supervisor events and publish mapped runtime events through `publish_runtime_events`/queue helper with the active/default thread context.

- [ ] **Step 5: Verify**

Run: `cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture`

Expected: pass.

### Task 5: Final Gates

**Files:**
- All touched core files.

- [ ] **Step 1: Focused tests**

Run:

```bash
cargo test -p roci-core --features agent subagent_runtime_wiring -- --nocapture
cargo test -p roci-core --features agent subagent_routing -- --nocapture
cargo test -p roci-core --features agent subagent_routing_tools -- --nocapture
```

- [ ] **Step 2: Core gates**

Run:

```bash
cargo check -p roci-core --features agent,mcp
cargo clippy -p roci-core --features agent,mcp --all-targets -- -D warnings
cargo fmt --all --check
git diff --check
tsq spec tsq-r0c1agt6.4 --check
```

- [ ] **Step 3: Close task**

Close `.4` only if tests pass. Note live CLI verification remains deferred to `.6`.

## Self-Review

- Spec coverage: config, tool injection, recursion guard, semantic event DTOs, event bridge, and tests covered.
- Scope guard: no CLI flags/profile file loading/rendering/live CLI smoke here.
- Risk: event bridge must not block runtime or leak raw child `AgentEvent`.
