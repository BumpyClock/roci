# Agent Loop Core Refactor

## Goal
Refactor `LoopRunner::start()` in `src/agent_loop/runner.rs` to support:
1. **Outer loop** (follow-up messages) — wraps existing loop
2. **Inner loop** (tool calls + steering) — existing loop with steering interrupts
3. **AgentEvent emission** alongside existing RunEvent emission
4. **Turn boundaries** (TurnStart/TurnEnd events)

## Key Constraint
This is an ADDITIVE refactor. All existing tests (~10 tests) must continue to pass. New capabilities are opt-in via new fields on RunRequest.

## Changes to RunRequest
```rust
pub struct RunRequest {
    // ... existing fields unchanged ...

    // NEW: Callback to get steering messages (checked between tool batches)
    pub get_steering_messages: Option<SteeringMessagesFn>,
    // NEW: Callback to get follow-up messages (checked after loop ends)
    pub get_follow_up_messages: Option<FollowUpMessagesFn>,
    // NEW: Pre-LLM context transformation hook
    pub transform_context: Option<TransformContextFn>,
    // NEW: AgentEvent sink (separate from RunEvent sink)
    pub agent_event_sink: Option<AgentEventSink>,
}

pub type SteeringMessagesFn = Arc<dyn Fn() -> Vec<ModelMessage> + Send + Sync>;
pub type FollowUpMessagesFn = Arc<dyn Fn() -> Vec<ModelMessage> + Send + Sync>;
pub type TransformContextFn = Arc<dyn Fn(Vec<ModelMessage>) -> Pin<Box<dyn Future<Output = Vec<ModelMessage>> + Send>> + Send + Sync>;
pub type AgentEventSink = Arc<dyn Fn(AgentEvent) + Send + Sync>;
```

## Loop Structure Refactor

### Current (single loop):
```
loop {
    iteration++
    check iteration limits
    drain input_rx
    compaction
    stream to LLM
    if no tool_calls -> complete
    execute tools (parallel batching)
    check failure limits
}
```

### New (outer + inner):
```
emit AgentStart
outer_loop {  // follow-up handling
    inner_loop {  // existing loop
        turn_index++
        emit TurnStart
        iteration++
        check iteration limits
        drain input_rx + steering messages
        compaction
        transform_context (NEW)
        stream to LLM
        emit MessageStart/Update/End
        if no tool_calls:
            emit TurnEnd
            break inner_loop
        execute tools (parallel batching, with steering check between batches)
        emit TurnEnd
        check failure limits
    }
    check follow_up_messages -> if any, push to messages, continue outer_loop
    else break
}
emit AgentEnd
```

### Steering Check Integration
In the tool execution section (lines 678-759), add steering check:
- After flushing a parallel batch: check `get_steering_messages()`
- After executing a sequential tool: check `get_steering_messages()`
- If steering messages arrive:
  - Skip remaining tool calls with result "Skipped due to queued user message"
  - Push steering messages to conversation
  - Continue to next inner loop iteration

## Files to modify
- `src/agent_loop/runner.rs` — Main refactor
- `src/agent_loop/mod.rs` — Export new type aliases

## Acceptance
- All 10+ existing tests pass unchanged
- New loop structure handles follow-ups (outer loop continues when follow-ups queued)
- Steering interrupts tool execution between batches
- TurnStart/TurnEnd events emitted around each inner loop iteration
- AgentEvent sink receives events if configured
- transform_context hook called before each LLM call
- When new fields are None, behavior is identical to current implementation
