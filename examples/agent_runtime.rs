//! Example: AgentRuntime with steering and follow-up patterns.
//!
//! This demonstrates the pi-mono aligned agent API:
//! - `prompt()` -- start conversations
//! - `steer()` -- interrupt tool execution
//! - `follow_up()` -- continue after completion
//! - `watch_state()` / `snapshot()` / `watch_snapshot()` -- observe state
//! - `abort()` / `reset()` -- lifecycle control
//!
//! The example compiles without API keys. Actual LLM calls are commented out
//! because they require a valid provider key at runtime.

use std::sync::Arc;

use roci::agent::runtime::AgentSnapshot;
use roci::agent::{AgentConfig, AgentRuntime, AgentState, QueueDrainMode};
use roci::agent_loop::AgentEvent;
use roci::config::RociConfig;
use roci::resource::CompactionSettings;
use roci::tools::{AgentTool, AgentToolParameters};
use roci::types::GenerationSettings;

#[tokio::main]
async fn main() {
    // -- 1. Create a simple echo tool
    let echo_tool: Arc<dyn roci::tools::Tool> = Arc::new(AgentTool::new(
        "echo",
        "Echoes back the input message",
        AgentToolParameters::object()
            .string("message", "The message to echo back", true)
            .build(),
        |args, _ctx| async move {
            let msg = args.get_str("message").unwrap_or("(empty)");
            Ok(serde_json::json!({ "echoed": msg }))
        },
    ));

    // -- 2. Set up an event sink to observe agent lifecycle
    let event_sink: Arc<dyn Fn(AgentEvent) + Send + Sync> =
        Arc::new(|event: AgentEvent| match &event {
            AgentEvent::AgentStart { run_id } => {
                println!("[start] run_id: {run_id}");
            }
            AgentEvent::TurnStart { turn_index, .. } => {
                println!("  -> Turn {turn_index} started");
            }
            AgentEvent::TurnEnd {
                turn_index,
                tool_results,
                ..
            } => {
                println!(
                    "  <- Turn {turn_index} ended ({} tool results)",
                    tool_results.len()
                );
            }
            AgentEvent::ToolExecutionStart { tool_name, .. } => {
                println!("  [tool] executing: {tool_name}");
            }
            AgentEvent::AgentEnd { .. } => {
                println!("[end] Agent ended");
            }
            _ => {}
        });

    // -- 3. Build agent configuration
    let model = "openai:gpt-4o".parse().expect("valid model identifier");

    let config = AgentConfig {
        model,
        system_prompt: Some("You are a helpful assistant with access to an echo tool.".into()),
        tools: vec![echo_tool],
        dynamic_tool_providers: Vec::new(),
        settings: GenerationSettings::default(),
        transform_context: None,
        convert_to_llm: None,
        event_sink: Some(event_sink),
        session_id: None,
        steering_mode: QueueDrainMode::All,
        follow_up_mode: QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        get_api_key: None,
        compaction: CompactionSettings::default(),
        session_before_compact: None,
        session_before_tree: None,
    };

    let roci_config = RociConfig::new();
    let registry = Arc::new(roci::default_registry());
    let agent = AgentRuntime::new(registry, roci_config, config);

    // -- 4. Verify initial state
    assert_eq!(agent.state().await, AgentState::Idle);
    println!("Agent state: {:?}", agent.state().await);
    assert!(agent.messages().await.is_empty());
    println!("Message history is empty");

    // -- 5. Snapshot -- point-in-time view of all observable dimensions
    let snap: AgentSnapshot = agent.snapshot().await;
    println!(
        "Snapshot: state={:?}, turn={}, msgs={}, streaming={}, error={:?}",
        snap.state, snap.turn_index, snap.message_count, snap.is_streaming, snap.last_error,
    );

    // -- 6. Watch for state changes (background task)
    let mut state_rx = agent.watch_state();
    tokio::spawn(async move {
        while state_rx.changed().await.is_ok() {
            let current = *state_rx.borrow();
            println!("[watch] State changed -> {current:?}");
        }
    });

    // watch_snapshot() gives richer updates than watch_state() -- it
    // includes turn_index, message_count, is_streaming, and last_error.
    let mut snap_rx = agent.watch_snapshot();
    tokio::spawn(async move {
        while snap_rx.changed().await.is_ok() {
            let snap = snap_rx.borrow().clone();
            println!(
                "[snapshot] update: {:?} turn={} msgs={}",
                snap.state, snap.turn_index, snap.message_count,
            );
        }
    });

    // -- 7. Queue steering messages
    //
    // Steering messages are injected between tool batches during a run,
    // allowing you to redirect the agent mid-execution. They accumulate
    // and are drained on the next inner-loop iteration.
    agent.steer("Focus on Rust code only").await;
    agent.steer("Keep responses concise").await;
    println!("Queued 2 steering messages");

    // -- 8. Queue follow-up messages
    //
    // Follow-up messages are checked when the inner loop ends naturally
    // (no more tool calls). If present, they extend the conversation
    // without the caller needing to issue another prompt().
    agent
        .follow_up("Also explain the performance implications")
        .await;
    println!("Queued 1 follow-up message");

    // -- 9. Start a prompt (requires valid API key)
    //
    // Uncomment the following to actually run against a provider:
    //
    //   let result = agent.prompt("Write a hello world in Rust").await;
    //   match result {
    //       Ok(run_result) => println!("Run completed: {:?}", run_result),
    //       Err(e) => eprintln!("Run failed: {e}"),
    //   }
    //
    // You can also continue a conversation:
    //
    //   let result = agent.continue_run("Now explain it line by line").await;
    //
    println!("Skipping actual prompt -- requires API key");

    // -- 10. Abort a run
    //
    // abort() returns `true` if a cancellation signal was sent, `false`
    // if the agent was idle or already aborting.
    let aborted = agent.abort().await;
    println!("Abort while idle returned: {aborted}"); // false -- nothing to abort

    // -- 11. Reset clears all state
    //
    // reset() aborts any in-flight run, waits for idle, then clears
    // message history and all queued steering/follow-up messages.
    agent.reset().await;
    assert_eq!(agent.state().await, AgentState::Idle);
    assert!(agent.messages().await.is_empty());
    println!("Agent reset -- ready for next conversation");

    println!("\nAgentRuntime API surface demonstrated successfully!");
}
