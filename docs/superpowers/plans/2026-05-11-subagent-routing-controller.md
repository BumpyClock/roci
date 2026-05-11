# Subagent Routing Controller Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build `tsq-r0c1agt6.3`: core routing controller plus subagent management tool surface.

**Architecture:** Add a focused routing module above `SubagentSupervisor`. The controller owns handle/result cache, profile default resolution, caller authorization, compact result mapping, and static tool wrappers. Runtime injection and semantic runtime events stay in `.4`.

**Tech Stack:** Rust, `tokio`, existing `SubagentSupervisor`, existing `Tool`/`AgentTool` APIs, fake provider tests.

---

## File Structure

- Modify: `crates/roci-core/src/agent/subagents/types.rs`
  Add routing DTOs and caller/context structs.
- Create: `crates/roci-core/src/agent/subagents/routing.rs`
  Implement `SubagentRoutingController`, state cache, tool builder, compact result mapper, and unit tests.
- Modify: `crates/roci-core/src/agent/subagents/handle.rs`
  Store child runtime clone and add `send_message` for active child steering.
- Modify: `crates/roci-core/src/agent/subagents/supervisor/mod.rs`
  Pass child runtime clone into `SubagentHandle`.
- Modify: `crates/roci-core/src/agent/subagents/mod.rs`
  Export routing module/types.
- Modify: `crates/roci-core/src/agent/subagents/profiles.rs`
  Add `default_profile_ref()` helper if controller needs direct default lookup.
- Test: `crates/roci-core/src/agent/subagents/routing.rs`
  Keep routing tests local to module with fake provider/tools.

### Task 1: Routing DTOs And Handle Send

**Files:**
- Modify: `crates/roci-core/src/agent/subagents/types.rs`
- Modify: `crates/roci-core/src/agent/subagents/handle.rs`
- Modify: `crates/roci-core/src/agent/subagents/supervisor/mod.rs`

- [ ] **Step 1: Add DTO tests**

Add tests in `types.rs` that serialize/deserialize `DelegateSubagentRequest`, `DelegateSubagentResult`, `SubagentCaller`, `SubagentCancelResult`, and `SendSubagentMessageResult`.

Expected shape:

```json
{
  "profile": "scout",
  "task": "Find runtime wiring",
  "label": "runtime scan",
  "run_in_background": true
}
```

- [ ] **Step 2: Run failing DTO tests**

Run: `cargo test -p roci-core --features agent subagent_routing_dto -- --nocapture`

Expected: fail because types do not exist.

- [ ] **Step 3: Implement DTOs**

Add structs with `Debug`, `Clone`, `Serialize`, `Deserialize`, `PartialEq` where possible:

```rust
pub struct DelegateSubagentRequest {
    pub profile: Option<SubagentProfileRef>,
    pub task: String,
    pub label: Option<String>,
    #[serde(default)]
    pub run_in_background: bool,
}

pub struct SubagentArtifact {
    pub kind: String,
    pub title: String,
    pub content: String,
}

pub struct DelegateSubagentResult {
    pub subagent_id: SubagentId,
    pub profile_id: SubagentProfileRef,
    pub status: SubagentStatus,
    pub summary: String,
    pub artifacts: Vec<SubagentArtifact>,
    pub child_thread_id: Option<String>,
    pub usage: Option<serde_json::Value>,
    pub error: Option<String>,
}
```

Also add `SubagentCaller`, `SubagentKnownChild`, `SubagentCancelResult`, and `SendSubagentMessageResult`.

- [ ] **Step 4: Add handle runtime clone**

Add `runtime: AgentRuntime` to `SubagentHandle`, pass it from `SubagentSupervisor::spawn_with_context`, and implement:

```rust
pub async fn send_message(&self, message: impl Into<crate::attachments::PromptInput>) -> Result<(), RociError> {
    match self.status().await {
        SubagentStatus::Pending | SubagentStatus::Running => self.runtime.steer(message).await,
        status => Err(RociError::InvalidState(format!(
            "subagent {} is {status:?}; cannot send message",
            self.id
        ))),
    }
}
```

- [ ] **Step 5: Verify DTO/handle tests**

Run: `cargo test -p roci-core --features agent subagent_routing_dto -- --nocapture`

Expected: pass.

### Task 2: Controller Core

**Files:**
- Create: `crates/roci-core/src/agent/subagents/routing.rs`
- Modify: `crates/roci-core/src/agent/subagents/profiles.rs`
- Modify: `crates/roci-core/src/agent/subagents/mod.rs`

- [ ] **Step 1: Write controller tests**

Cover:

- `delegate_without_profile_uses_default_profile`
- `delegate_without_default_profile_returns_clear_error`
- `delegate_unknown_profile_returns_clear_error`
- `list_profiles_returns_sorted_profile_summaries`
- `foreground_delegate_returns_compact_summary`
- `background_delegate_is_listed_until_waited`
- `wait_subagent_caches_completion_result`
- `cancel_subagent_reports_canceled`
- `management_methods_reject_child_callers`
- `send_subagent_message_rejects_unknown_and_terminal_children`

- [ ] **Step 2: Run failing controller tests**

Run: `cargo test -p roci-core --features agent subagent_routing -- --nocapture`

Expected: fail because routing module does not exist.

- [ ] **Step 3: Implement `default_profile_ref`**

Add deterministic helper:

```rust
pub fn default_profile_ref(&self) -> Option<String> {
    self.profiles
        .values()
        .find(|profile| profile.default)
        .map(|profile| profile.name.clone())
}
```

- [ ] **Step 4: Implement controller state**

Use one authoritative profile registry. Preferred public constructor shape:

```rust
pub struct SubagentRoutingController {
    supervisor: Arc<SubagentSupervisor>,
    profiles: SubagentProfileRegistry,
    state: Arc<tokio::sync::Mutex<RoutingState>>,
    max_depth: u32,
}
```

`SubagentRoutingController::new(...)` must create both `SubagentSupervisor` and controller state from the same `SubagentProfileRegistry` clone. Avoid a public constructor that accepts unrelated `supervisor` and `profiles` values unless it validates equality or is `#[cfg(test)]`.

`RoutingState` stores `HashMap<SubagentId, ChildRoutingRecord>`. Each record stores profile, label, model, status, handle, and cached result.

- [ ] **Step 5: Implement delegate/list/wait/cancel/send**

Delegate builds `SubagentSpec { profile, label, input: SubagentInput::Prompt { task }, overrides: Default::default() }`, calls supervisor spawn, stores handle, and either waits or returns running result.

Implement `list_profiles()` through `profiles.profile_summaries()` and expose sorted summaries.

Add shared authorization:

```rust
fn authorize_main_agent(caller: &SubagentCaller) -> Result<(), RociError> {
    if caller.is_main_agent {
        Ok(())
    } else {
        Err(RociError::Configuration(
            "subagent management tools are only available to the main agent".into(),
        ))
    }
}
```

Call it from `delegate`, `list_profiles`, `list_subagents`, `wait_subagent`, `cancel_subagent`, and `send_subagent_message`.

Compact result summary:

```rust
fn summarize_result(result: &SubagentRunResult) -> String {
    result.messages
        .iter()
        .rev()
        .find(|message| message.role == Role::Assistant)
        .map(ModelMessage::text)
        .unwrap_or_default()
}
```

- [ ] **Step 6: Export routing module**

Update `mod.rs`:

```rust
pub mod routing;
pub use routing::{SubagentRoutingController, SubagentRoutingTools};
```

- [ ] **Step 7: Verify controller tests**

Run: `cargo test -p roci-core --features agent subagent_routing -- --nocapture`

Expected: pass.

### Task 3: Model-Callable Tool Surface

**Files:**
- Modify: `crates/roci-core/src/agent/subagents/routing.rs`
- Modify: `crates/roci-core/src/agent/subagents/mod.rs`

- [ ] **Step 1: Write tool execution tests**

Cover:

- `delegate_subagent_tool_executes_foreground_and_returns_json`
- `delegate_subagent_tool_accepts_minimal_task_args`
- `list_subagents_tool_returns_known_children`
- `wait_subagent_tool_returns_compact_result`
- `cancel_subagent_tool_returns_cancel_result`
- `send_subagent_message_tool_returns_acceptance_or_error`

- [ ] **Step 2: Run failing tool tests**

Run: `cargo test -p roci-core --features agent subagent_routing_tools -- --nocapture`

Expected: fail because tools are not built.

- [ ] **Step 3: Implement `SubagentRoutingTools`**

Build `Vec<Arc<dyn Tool>>` using `AgentTool::new` and explicit JSON schemas from `AgentToolParameters::from_schema`.

Example `delegate_subagent` args:

```json
{
  "type": "object",
  "properties": {
    "profile": { "type": "string" },
    "task": { "type": "string" },
    "label": { "type": "string" },
    "run_in_background": { "type": "boolean" }
  },
  "required": ["task"]
}
```

Each handler deserializes args and calls the controller with fixed `SubagentCaller::main_agent()`. Minimal delegate args `{ "task": "Find runtime wiring" }` must deserialize because `run_in_background` defaults to `false`.

- [ ] **Step 4: Mark tools safe host-control**

Use `ToolApproval::safe_host_input()` for management tools. They mutate host runtime state, but only inside already-authorized agent session control plane and not external resources.

- [ ] **Step 5: Verify tool tests**

Run: `cargo test -p roci-core --features agent subagent_routing_tools -- --nocapture`

Expected: pass.

### Task 4: Integration Gate

**Files:**
- All modified core files.

- [ ] **Step 1: Run focused tests**

Run:

```bash
cargo test -p roci-core --features agent subagent_routing -- --nocapture
cargo test -p roci-core --features agent subagent_routing_tools -- --nocapture
cargo test -p roci-core --features agent subagent_routing_dto -- --nocapture
```

Expected: pass.

- [ ] **Step 2: Run core gates**

Run:

```bash
cargo check -p roci-core --features agent,mcp
cargo clippy -p roci-core --features agent,mcp --all-targets -- -D warnings
cargo fmt --all --check
git diff --check
```

Expected: pass.

- [ ] **Step 3: Update task state**

If tests pass, close `tsq-r0c1agt6.3` with note that live CLI verification is deferred until `.6` because `.3` does not inject tools into runtime/CLI.

## Self-Review

- Spec coverage: `.3` controller, default resolution, profile listing, foreground/background, wait/cancel/send, tool DTOs, tool wrappers, and main-only management enforcement covered.
- Scope guard: no runtime injection, semantic runtime event projection, CLI profile loading, or live CLI smoke in this plan.
- Placeholder scan: no TBD/TODO markers.
- Risk: `send_subagent_message` depends on storing child runtime clone in handle. Implementation must clone `launched.runtime` before moving the original into `run_child_task`. Tests must prove terminal child rejection and active child steering queue acceptance.
