use super::support::*;
use super::*;
use crate::agent_loop::RunStatus;
use crate::context::ContextBudget;
use crate::types::Usage;

fn runtime_with_budget(context_budget: Option<ContextBudget>) -> AgentRuntime {
    let registry = registry_with_streaming_provider("stub", 40, 5);
    let mut config = test_agent_config();
    config.model = "stub:budget-runtime"
        .parse()
        .expect("stub model should parse");
    config.context_budget = context_budget;
    AgentRuntime::new(registry, test_config(), config)
}

#[tokio::test]
async fn reset_clears_session_usage_ledger() {
    let agent = runtime_with_budget(None);

    let result = agent.prompt("first run").await.expect("run should succeed");
    assert_eq!(result.status, RunStatus::Completed);

    let usage = agent.session_usage().await;
    assert_eq!(
        usage.input_tokens, 40,
        "precondition: ledger should be nonzero"
    );
    assert_eq!(
        usage.output_tokens, 5,
        "precondition: ledger should be nonzero"
    );
    assert_eq!(
        usage.total_tokens, 45,
        "precondition: ledger should be nonzero"
    );

    agent.reset().await;
    let usage = agent.session_usage().await;
    assert_eq!(usage.input_tokens, 0, "reset should clear input_tokens");
    assert_eq!(usage.output_tokens, 0, "reset should clear output_tokens");
    assert_eq!(usage.total_tokens, 0, "reset should clear total_tokens");
}

#[tokio::test]
async fn session_budget_uses_prior_run_ledger_across_runs() {
    let agent = runtime_with_budget(Some(ContextBudget {
        max_session_input_tokens: Some(120),
        reserve_output_tokens: 0,
        ..Default::default()
    }));

    let first = agent
        .prompt("first run")
        .await
        .expect("first run should return a result");
    assert_eq!(
        first.status,
        RunStatus::Completed,
        "error: {:?}",
        first.error
    );

    let usage_after_first = agent.session_usage().await;
    assert_eq!(usage_after_first.input_tokens, 40);
    assert_eq!(usage_after_first.output_tokens, 5);
    assert_eq!(usage_after_first.total_tokens, 45);

    let second = agent
        .continue_run("x".repeat(600))
        .await
        .expect("second run should return a result");
    assert_eq!(second.status, RunStatus::Failed);
    assert!(
        second
            .error
            .as_deref()
            .expect("budget rejection should carry an error")
            .contains("session input"),
        "second run should fail session-input preflight"
    );
    assert!(
        second.usage_delta.is_none(),
        "pre-provider budget failures must not report usage"
    );

    let usage_after_second = agent.session_usage().await;
    assert_eq!(
        usage_after_second, usage_after_first,
        "pre-provider budget failures must not mutate the persisted session ledger"
    );
}

#[tokio::test]
async fn session_usage_starts_at_zero() {
    let registry = test_registry();
    let config = test_agent_config();
    let agent = AgentRuntime::new(registry, test_config(), config);

    let usage = agent.session_usage().await;
    assert_eq!(usage, Usage::default());
}

#[tokio::test]
async fn reset_allows_new_run_after_budget_exhaustion() {
    // After budget exhaustion, reset() clears the session ledger so a new run
    // can succeed.
    let agent = runtime_with_budget(Some(ContextBudget {
        max_session_input_tokens: Some(120),
        reserve_output_tokens: 0,
        ..Default::default()
    }));

    // Run 1 completes.
    let result1 = agent
        .prompt("hi")
        .await
        .expect("first run should return a result");
    assert_eq!(
        result1.status,
        RunStatus::Completed,
        "error: {:?}",
        result1.error
    );

    // Run 2 rejected by budget.
    let result2 = agent
        .continue_run("x".repeat(600))
        .await
        .expect("second run should return a result");
    assert_eq!(result2.status, RunStatus::Failed);

    // Reset clears the ledger.
    agent.reset().await;
    let usage = agent.session_usage().await;
    assert_eq!(usage.input_tokens, 0, "reset should clear session usage");

    // Run 3 should succeed after reset.
    let result3 = agent
        .prompt("fresh start")
        .await
        .expect("third run should return a result");
    assert_eq!(
        result3.status,
        RunStatus::Completed,
        "run after reset should succeed; error: {:?}",
        result3.error
    );
    let usage3 = agent.session_usage().await;
    assert_eq!(
        usage3.input_tokens, 40,
        "session ledger should have third run's input"
    );
}
