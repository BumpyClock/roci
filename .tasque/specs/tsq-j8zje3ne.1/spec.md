# AgentMessage Trait System

## Goal
Define a trait-based extensible message system. Standard LLM messages (ModelMessage) are the default, but users can implement AgentMessageExt for custom message types.

## Design

### Trait: AgentMessageExt
```rust
pub trait AgentMessageExt: Send + Sync + std::fmt::Debug + Clone {
    /// Returns Some(ModelMessage) if this message should be sent to the LLM.
    /// Returns None if this is a UI-only or metadata message.
    fn as_llm_message(&self) -> Option<&ModelMessage>;

    /// Timestamp of the message
    fn timestamp(&self) -> Option<chrono::DateTime<chrono::Utc>>;

    /// Kind identifier for serialization/filtering
    fn kind(&self) -> &str;
}
```

### Default implementation for ModelMessage
```rust
impl AgentMessageExt for ModelMessage {
    fn as_llm_message(&self) -> Option<&ModelMessage> { Some(self) }
    fn timestamp(&self) -> Option<DateTime<Utc>> { self.timestamp }
    fn kind(&self) -> &str { "llm" }
}
```

### AgentMessage enum (concrete default type)
For ergonomics, provide a concrete enum that most users will use:
```rust
#[derive(Debug, Clone)]
pub enum AgentMessage {
    Llm(ModelMessage),
    Custom { kind: String, data: serde_json::Value, timestamp: DateTime<Utc> },
}

impl AgentMessageExt for AgentMessage { ... }
```

### convert_to_llm helper
```rust
pub fn convert_to_llm<M: AgentMessageExt>(messages: &[M]) -> Vec<ModelMessage> {
    messages.iter().filter_map(|m| m.as_llm_message().cloned()).collect()
}
```

## Files to modify
- Create: `src/agent/message.rs` (new file for AgentMessage types)
- Modify: `src/agent/mod.rs` (add module export)
- Modify: `src/lib.rs` (export from prelude if needed)

## Acceptance
- AgentMessageExt trait compiles with both ModelMessage and custom types
- convert_to_llm filters correctly
- AgentMessage enum provides ergonomic default
- Existing ModelMessage usage not broken
