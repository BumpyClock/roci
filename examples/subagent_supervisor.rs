//! Sub-agent supervisor example.
//!
//! Demonstrates supervisor creation, profile registration, spawning children,
//! subscribing to events, and waiting for completion.
//!
//! Run with:
//! ```sh
//! ANTHROPIC_API_KEY=sk-... cargo run --example subagent_supervisor --features agent
//! ```
//!
//! Without a valid API key, the spawn will fail with a model-resolution
//! error -- that's expected and is handled gracefully below.

use std::sync::Arc;

use roci::agent::runtime::QueueDrainMode;
use roci::agent::subagents::{
    SubagentInput, SubagentProfileRegistry, SubagentSpec, SubagentSupervisor,
    SubagentSupervisorConfig,
};
use roci::agent::AgentConfig;
use roci::agent_loop::runner::RetryBackoffPolicy;
use roci::config::RociConfig;
use roci::resource::CompactionSettings;
use roci::types::GenerationSettings;

#[tokio::main]
async fn main() {
    println!("=== Sub-agent Supervisor Example ===\n");

    // -- 1. Provider registry (has providers only when feature flags are enabled)
    let registry = Arc::new(roci::default_registry());

    // -- 2. Configuration
    let roci_config = RociConfig::new();

    // -- 3. Profile registry with built-in profiles
    let profile_registry = SubagentProfileRegistry::with_builtins();
    println!("Registered built-in profiles: builtin:developer, builtin:planner, builtin:explorer");

    // -- 4. Base agent config (inherited by children unless overridden)
    let model = "anthropic:claude-sonnet-4-20250514"
        .parse()
        .expect("valid model identifier");

    let base_config = AgentConfig {
        model,
        system_prompt: Some("You are a helpful assistant.".into()),
        tools: Vec::new(),
        dynamic_tool_providers: Vec::new(),
        settings: GenerationSettings::default(),
        transform_context: None,
        convert_to_llm: None,
        before_agent_start: None,
        event_sink: None,
        session_id: None,
        steering_mode: QueueDrainMode::All,
        follow_up_mode: QueueDrainMode::All,
        transport: None,
        max_retry_delay_ms: None,
        retry_backoff: RetryBackoffPolicy::default(),
        api_key_override: None,
        provider_headers: Default::default(),
        provider_metadata: std::collections::HashMap::new(),
        provider_payload_callback: None,
        get_api_key: None,
        compaction: CompactionSettings::default(),
        session_before_compact: None,
        session_before_tree: None,
        pre_tool_use: None,
        post_tool_use: None,
        user_input_timeout_ms: None,
        user_input_coordinator: None,
    };

    // -- 5. Supervisor config
    let supervisor_config = SubagentSupervisorConfig {
        max_concurrent: 4,
        max_active_children: Some(10),
        default_input_timeout_ms: None,
        abort_on_drop: true,
    };
    println!("Supervisor config: max_concurrent=4, max_active=10, abort_on_drop=true");

    // -- 6. Create the supervisor
    let supervisor = SubagentSupervisor::new(
        registry,
        roci_config,
        base_config,
        supervisor_config,
        profile_registry,
    );
    println!("Supervisor created.\n");

    // -- 7. Subscribe to events
    let mut event_rx = supervisor.subscribe();
    tokio::spawn(async move {
        while let Ok(event) = event_rx.recv().await {
            println!("[event] {event:?}");
        }
    });
    println!("Event subscriber started.");

    // -- 8. List active children (should be empty)
    let active = supervisor.list_active().await;
    println!("Active children: {} (expected 0)", active.len());

    // -- 9. Attempt to spawn a child
    //
    //    Without valid API credentials this will fail at model resolution.
    //    That's fine -- we demonstrate the error path.
    let spec = SubagentSpec {
        profile: "builtin:developer".into(),
        label: Some("example-worker".into()),
        input: SubagentInput::Prompt {
            task: "Say hello in Rust.".into(),
        },
        overrides: Default::default(),
    };

    println!("\nSpawning child with profile 'builtin:developer'...");
    match supervisor.spawn(spec).await {
        Ok(handle) => {
            println!(
                "Child spawned: id={}, label={:?}",
                handle.id(),
                handle.label()
            );
            println!("Waiting for child to complete...");
            let result = handle.wait().await;
            println!(
                "Child result: status={:?}, error={:?}",
                result.status, result.error
            );
        }
        Err(e) => {
            println!("Spawn failed (expected without API key): {e}");
        }
    }

    // -- 10. wait_all (returns immediately since nothing is running)
    let completions = supervisor.wait_all().await;
    println!("\nwait_all returned {} completions.", completions.len());

    // -- 11. Shutdown
    println!("Shutting down supervisor...");
    supervisor.shutdown().await;
    println!("Supervisor shut down cleanly.");

    println!("\n=== Example Complete ===");
}
