# Tool Trait Extension

## Goal
Extend the existing Tool trait with optional methods for label, streaming updates, and cancellation support. Must not break existing Tool implementations.

## Design

### Extended Tool trait
```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> AgentToolParameters;

    // NEW: Human-readable label for UI (defaults to name)
    fn label(&self) -> &str {
        self.name()
    }

    // CHANGED: execute now takes CancellationToken + on_update
    async fn execute(
        &self,
        args: ToolArguments,
        ctx: ToolExecutionContext,
    ) -> std::result::Result<serde_json::Value, RociError>;

    // NEW: Extended execute with cancellation + streaming updates
    // Default implementation delegates to execute() for backward compat
    async fn execute_ext(
        &self,
        args: ToolArguments,
        ctx: ToolExecutionContext,
        cancel: tokio_util::sync::CancellationToken,
        on_update: Option<ToolUpdateCallback>,
    ) -> std::result::Result<serde_json::Value, RociError> {
        // Default: ignore cancel and on_update, delegate to simple execute
        let _ = cancel;
        let _ = on_update;
        self.execute(args, ctx).await
    }
}

pub type ToolUpdateCallback = Arc<dyn Fn(ToolUpdatePayload) + Send + Sync>;
```

### Add tokio-util dependency
```toml
# Cargo.toml
tokio-util = { version = "0.7", features = ["sync"], optional = true }

[features]
agent = ["dep:tokio-util"]
```

## Key Principle
- Existing Tool impls only need `execute()` — unchanged
- New tools can override `execute_ext()` for cancellation + streaming
- Agent loop calls `execute_ext()` which defaults to `execute()`

## Files to modify
- Modify: `src/tools/tool.rs` (add label, execute_ext, ToolUpdateCallback)
- Modify: `Cargo.toml` (add tokio-util dep)
- Modify: `src/agent_loop/runner.rs` (call execute_ext instead of execute)

## Acceptance
- Existing Tool impls compile without changes
- New tools can implement execute_ext with cancellation
- label() defaults to name()
- tokio-util only pulled in with agent feature
