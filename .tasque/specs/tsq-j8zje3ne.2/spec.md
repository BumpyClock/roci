# AgentEvent Enum

## Goal
Define comprehensive event types for the agent loop, matching pi-mono event granularity while keeping roci-unique events (approval, reasoning).

## Design

### New events to add alongside existing RunEvent system
```rust
#[derive(Debug, Clone)]
pub enum AgentEvent {
    // Lifecycle
    AgentStart { run_id: RunId },
    AgentEnd { run_id: RunId, messages: Vec<AgentMessage> },

    // Turn boundaries (NEW - pi-mono alignment)
    TurnStart { run_id: RunId, turn_index: usize },
    TurnEnd {
        run_id: RunId,
        turn_index: usize,
        assistant_message: AgentMessage,
        tool_results: Vec<AgentToolResult>,
    },

    // Message streaming
    MessageStart { message: AgentMessage },
    MessageUpdate { message: AgentMessage, delta: TextStreamDelta },
    MessageEnd { message: AgentMessage },

    // Tool execution (enhanced with streaming updates)
    ToolExecutionStart { tool_call_id: String, tool_name: String, args: serde_json::Value },
    ToolExecutionUpdate { tool_call_id: String, tool_name: String, partial_result: ToolUpdatePayload },
    ToolExecutionEnd { tool_call_id: String, tool_name: String, result: AgentToolResult, is_error: bool },

    // Existing roci events (keep)
    Approval(ApprovalRequest),
    Reasoning { text: String },
    Error { error: String },
    System { message: String },
}
```

### ToolUpdatePayload
```rust
pub struct ToolUpdatePayload {
    pub content: Vec<ContentPart>,
    pub details: serde_json::Value,
}
```

## Approach
- Add AgentEvent as a NEW type in `src/agent_loop/events.rs`
- Keep existing RunEvent/RunEventPayload for backward compat
- The refactored loop will emit AgentEvent; existing RunEvent can be derived from AgentEvent for compat

## Files to modify
- Modify: `src/agent_loop/events.rs` (add AgentEvent enum + ToolUpdatePayload)
- Modify: `src/agent_loop/mod.rs` (export new types)

## Acceptance
- AgentEvent covers all pi-mono event types + roci-unique events
- ToolUpdatePayload enables streaming partial results
- Existing RunEvent types not removed (backward compat)
