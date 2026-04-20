use super::*;
use crate::agent_loop::RunStatus;
use crate::context::{estimate_message_tokens, ContextBudget};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use support::{test_model, test_runner, ProviderScenario};

#[tokio::test]
async fn no_budget_configured_preserves_existing_behavior() {
    let (runner, _requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hi")]).with_retry_backoff(
        RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        },
    );
    // No context_budget set — should complete normally.
    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(result.status, RunStatus::Completed);
    // Usage delta should still be present (captured from stream).
    let usage = result.usage_delta.expect("usage_delta should be present");
    assert_eq!(usage.input_tokens, 50);
    assert_eq!(usage.output_tokens, 10);
}

#[tokio::test]
async fn per_turn_budget_rejection() {
    let (runner, _requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    // Set a very tight per-turn limit that will be exceeded by the request.
    let budget = ContextBudget {
        max_turn_input_tokens: Some(1), // 1 token — any real request will exceed this
        reserve_output_tokens: 0,
        ..Default::default()
    };
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello world")])
        .with_context_budget(budget)
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });
    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(result.status, RunStatus::Failed);
    let error = result.error.as_deref().unwrap();
    assert!(
        error.contains("context budget exceeded"),
        "expected budget rejection, got: {error}"
    );
    assert!(
        error.contains("turn input"),
        "expected turn input violation detail, got: {error}"
    );
}

#[tokio::test]
async fn session_input_budget_rejection() {
    let (runner, _requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    // Session input limit already nearly exhausted via prior usage.
    let budget = ContextBudget {
        max_session_input_tokens: Some(100),
        reserve_output_tokens: 0,
        ..Default::default()
    };
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello world")])
        .with_context_budget(budget)
        .with_prior_session_usage(99, 0) // 99 prior input, so this turn tips it over
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });
    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(result.status, RunStatus::Failed);
    let error = result.error.as_deref().unwrap();
    assert!(
        error.contains("context budget exceeded"),
        "expected budget rejection, got: {error}"
    );
    assert!(
        error.contains("session input"),
        "expected session input violation detail, got: {error}"
    );
}

#[tokio::test]
async fn session_output_budget_rejection() {
    let (runner, _requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    // Set max session output very low; prior output already at limit.
    let budget = ContextBudget {
        max_session_output_tokens: Some(5),
        reserve_output_tokens: 100, // projected output = prior + reserve
        ..Default::default()
    };
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")])
        .with_context_budget(budget)
        .with_prior_session_usage(0, 1) // prior output = 1, projected = 1 + 100 = 101 > 5
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });
    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(
        result.status,
        RunStatus::Failed,
        "error: {:?}",
        result.error
    );
    let error = result.error.as_deref().unwrap();
    assert!(
        error.contains("context budget exceeded"),
        "expected budget rejection, got: {error}"
    );
    assert!(
        error.contains("session output"),
        "expected session output violation detail, got: {error}"
    );
}

#[tokio::test]
async fn generous_budget_allows_request() {
    let (runner, _requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    // StubProvider context_length = 4096, so reserve must be < 4096.
    let budget = ContextBudget {
        max_turn_input_tokens: Some(100_000),
        max_session_input_tokens: Some(1_000_000),
        max_session_output_tokens: Some(1_000_000),
        reserve_output_tokens: 100,
        ..Default::default()
    };
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hi")])
        .with_context_budget(budget)
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });
    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "error: {:?}",
        result.error
    );
}

#[tokio::test]
async fn usage_delta_accumulates_across_iterations() {
    // When no budget is set, usage delta should still be tracked from stream.
    let (runner, _requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hi")]).with_retry_backoff(
        RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        },
    );
    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(result.status, RunStatus::Completed);
    let usage = result.usage_delta.expect("usage_delta should be present");
    // TextOnlyWithUsage reports input=50, output=10
    assert_eq!(usage.input_tokens, 50);
    assert_eq!(usage.output_tokens, 10);
    assert_eq!(usage.total_tokens, 60);
}

// ---- New tests for follow-up fixes ----

#[tokio::test]
async fn mid_stream_failure_still_captures_usage() {
    // TextWithUsageThenStreamError emits partial text with usage (input=30,
    // output=5) then a stream error. The run should fail but usage_delta
    // must still reflect the provider-reported usage from the partial stream.
    let (runner, _requests) = test_runner(ProviderScenario::TextWithUsageThenStreamError);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hi")]).with_retry_backoff(
        RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        },
    );
    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(result.status, RunStatus::Failed);
    let usage = result
        .usage_delta
        .expect("usage_delta should be present even on mid-stream failure");
    assert_eq!(usage.input_tokens, 30, "should capture partial input usage");
    assert_eq!(
        usage.output_tokens, 5,
        "should capture partial output usage"
    );
}

#[tokio::test]
async fn pre_provider_budget_rejection_has_no_usage_delta() {
    // When the budget rejects before the provider is called, usage_delta
    // should be None (not Some(Usage::default())).
    let (runner, _requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    let budget = ContextBudget {
        max_turn_input_tokens: Some(1),
        reserve_output_tokens: 0,
        ..Default::default()
    };
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello world")])
        .with_context_budget(budget)
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });
    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result.usage_delta.is_none(),
        "pre-provider failure should not report usage_delta, got: {:?}",
        result.usage_delta
    );
}

#[tokio::test]
async fn stream_error_without_usage_produces_heuristic_estimate() {
    // TextThenStreamError emits partial text without usage then errors.
    // The run should still have usage_delta with heuristic estimate > 0.
    let (runner, _requests) = test_runner(ProviderScenario::TextThenStreamError);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hi")]).with_retry_backoff(
        RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        },
    );
    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(result.status, RunStatus::Failed);
    let usage = result
        .usage_delta
        .expect("usage_delta should be present with heuristic estimate");
    assert!(
        usage.input_tokens > 0,
        "heuristic should estimate nonzero input"
    );
    assert!(
        usage.output_tokens > 0,
        "heuristic should estimate nonzero output for partial text"
    );
}

#[tokio::test]
async fn stream_error_before_any_delta_does_not_charge_output_tokens() {
    let (runner, _requests) = test_runner(ProviderScenario::ImmediateStreamError);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hi")]).with_retry_backoff(
        RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        },
    );

    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(result.status, RunStatus::Failed);

    let usage = result
        .usage_delta
        .expect("post-provider failures should still carry input usage");
    assert!(
        usage.input_tokens > 0,
        "input usage should still be charged for the provider-facing request"
    );
    assert_eq!(
        usage.output_tokens, 0,
        "no output tokens should be charged before any text/tool delta arrives"
    );
}

#[tokio::test]
async fn heuristic_usage_uses_provider_facing_messages_after_transform() {
    let (runner, requests) = test_runner(ProviderScenario::TextThenStreamError);
    let raw_message = ModelMessage::user("raw");
    let transformed_message = ModelMessage::user("provider-facing transformed payload ".repeat(64));
    let raw_estimate = estimate_message_tokens(&raw_message);

    let request = RunRequest::new(test_model(), vec![raw_message]).with_transform_context(
        Arc::new(move |_payload| {
            let transformed_message = transformed_message.clone();
            Box::pin(async move {
                Ok(TransformContextHookResult::ReplaceMessages {
                    messages: vec![transformed_message],
                })
            })
        }),
    );

    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(result.status, RunStatus::Failed);

    let usage = result
        .usage_delta
        .expect("heuristic usage should be recorded after provider stream starts");
    let requests = requests.lock().expect("request lock");
    let provider_messages = &requests[0].messages;
    let expected_input: u32 = provider_messages
        .iter()
        .map(estimate_message_tokens)
        .sum::<usize>() as u32;

    assert!(
        expected_input > raw_estimate as u32,
        "transformed provider payload should differ from the raw agent input"
    );
    assert_eq!(
        usage.input_tokens, expected_input,
        "heuristic usage must be based on the provider-facing request payload"
    );
}

#[tokio::test]
async fn exact_anchor_allows_second_call_that_full_recount_would_reject() {
    let (runner, requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    let long_input = "x".repeat(1600);
    let initial_message = ModelMessage::user(long_input);
    let follow_up_message = ModelMessage::user("follow up");
    let assistant_message = ModelMessage::assistant("hello");
    let initial_turn_input = estimate_message_tokens(&initial_message);
    let exact_second_turn_input = 50
        + estimate_message_tokens(&assistant_message)
        + estimate_message_tokens(&follow_up_message);
    let second_turn_full_recount = initial_turn_input
        + estimate_message_tokens(&assistant_message)
        + estimate_message_tokens(&follow_up_message);
    let budget_limit = initial_turn_input + 1;

    assert!(
        exact_second_turn_input < budget_limit,
        "budget must still allow the exact-anchor estimate"
    );
    assert!(
        second_turn_full_recount > budget_limit,
        "budget must reject a full heuristic recount on the second turn"
    );

    let follow_up_calls = Arc::new(AtomicUsize::new(0));
    let request = RunRequest::new(test_model(), vec![initial_message])
        .with_follow_up_messages(Arc::new({
            let follow_up_calls = follow_up_calls.clone();
            let follow_up_message = follow_up_message.clone();
            move || {
                let follow_up_calls = follow_up_calls.clone();
                let follow_up_message = follow_up_message.clone();
                Box::pin(async move {
                    if follow_up_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        vec![follow_up_message]
                    } else {
                        Vec::new()
                    }
                })
            }
        }))
        .with_context_budget(ContextBudget {
            max_turn_input_tokens: Some(budget_limit),
            reserve_output_tokens: 0,
            ..Default::default()
        })
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });

    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "exact-anchor reuse should let the second provider call proceed"
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2, "both provider calls should be attempted");
}

#[tokio::test]
async fn zero_usage_completion_does_not_install_exact_anchor() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let long_input = "x".repeat(1600);
    let initial_message = ModelMessage::user(long_input);
    let follow_up_message = ModelMessage::user("follow up");
    let assistant_message = ModelMessage::assistant("done");
    let initial_turn_input = estimate_message_tokens(&initial_message);
    let second_turn_full_recount = initial_turn_input
        + estimate_message_tokens(&assistant_message)
        + estimate_message_tokens(&follow_up_message);
    let budget_limit = initial_turn_input + 1;

    assert!(
        second_turn_full_recount > budget_limit,
        "without an anchor, the second turn should fail budget preflight"
    );

    let follow_up_calls = Arc::new(AtomicUsize::new(0));
    let request = RunRequest::new(test_model(), vec![initial_message])
        .with_follow_up_messages(Arc::new({
            let follow_up_calls = follow_up_calls.clone();
            let follow_up_message = follow_up_message.clone();
            move || {
                let follow_up_calls = follow_up_calls.clone();
                let follow_up_message = follow_up_message.clone();
                Box::pin(async move {
                    if follow_up_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                        vec![follow_up_message]
                    } else {
                        Vec::new()
                    }
                })
            }
        }))
        .with_context_budget(ContextBudget {
            max_turn_input_tokens: Some(budget_limit),
            reserve_output_tokens: 0,
            ..Default::default()
        })
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });

    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;
    assert_eq!(
        result.status,
        RunStatus::Failed,
        "zero-usage completions must not create a fail-open anchor"
    );
    assert!(
        result
            .error
            .as_deref()
            .expect("budget failure should carry an error")
            .contains("turn input"),
        "second turn should fail the turn-input budget preflight"
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(
        requests.len(),
        1,
        "second provider call should be blocked before dispatch when zero usage cannot install an anchor"
    );
}

#[tokio::test]
async fn exact_anchor_allows_request_that_full_heuristic_would_reject() {
    // This test proves the exact-anchor estimation path is actually exercised.
    //
    // Setup:
    //   - A very long user message (~500 heuristic tokens).
    //   - ToolCallWithUsageThenTextWithUsage scenario: call 0 returns a tool call
    //     with provider-reported input=50, call 1 returns text with input=60.
    //   - The noop_tool is registered so the tool phase succeeds.
    //
    // Key insight:
    //   After call 0 completes with usage, the exact-anchor is set with
    //   prompt_tokens=50 and the provider-facing messages from call 0.
    //   On call 1, the same prefix is present plus a small tail (assistant reply
    //   + tool result), so the anchor path estimates ~50 + ~small tail.
    //
    //   Without anchor, the full heuristic would re-estimate the long user
    //   message at ~500 tokens, causing a budget rejection.
    //
    //   By setting max_turn_input_tokens between these two estimates, the test
    //   passes only if the anchor path is used.
    let (runner, _requests) = test_runner(ProviderScenario::ToolCallWithUsageThenTextWithUsage);

    // ~2000 chars → ~500 heuristic tokens (estimate_message_tokens uses ~4 chars/token + overhead).
    let long_text = "a]".repeat(1000);

    let noop_tool: std::sync::Arc<dyn crate::tools::tool::Tool> =
        std::sync::Arc::new(AgentTool::new(
            "noop_tool",
            "does nothing",
            AgentToolParameters::empty(),
            |_args: ToolArguments, _ctx: ToolExecutionContext| async move {
                Ok(serde_json::json!({"ok": true}))
            },
        ));

    // max_turn_input_tokens=520: above call 0 heuristic (~504) so it passes,
    // above anchor estimate for call 1 (~91) so it passes with anchor,
    // but below call 1 full heuristic (~545) so it would fail without anchor.
    // StubProvider context_length=4096, so reserve_output_tokens < 4096.
    let budget = ContextBudget {
        max_turn_input_tokens: Some(520),
        reserve_output_tokens: 100,
        ..Default::default()
    };

    let request = RunRequest::new(test_model(), vec![ModelMessage::user(long_text)])
        .with_tools(vec![noop_tool])
        .with_context_budget(budget)
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });

    let handle = runner.start(request).await.unwrap();
    let result = handle.wait().await;

    // If the anchor path works, call 1's estimate stays under the 520-token
    // limit and the run completes. A full heuristic recount would exceed that
    // limit and fail budget preflight.
    assert_eq!(
        result.status,
        RunStatus::Completed,
        "exact-anchor should keep estimate under budget; error: {:?}",
        result.error
    );

    // Verify usage_delta accumulated from both calls.
    let usage = result
        .usage_delta
        .expect("two provider calls should produce usage");
    // Call 0: input=50 output=10; Call 1: input=60 output=5 → total input=110 output=15.
    assert_eq!(usage.input_tokens, 110, "accumulated input from both calls");
    assert_eq!(
        usage.output_tokens, 15,
        "accumulated output from both calls"
    );
}
