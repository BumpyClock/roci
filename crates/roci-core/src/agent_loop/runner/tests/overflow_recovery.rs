use super::*;

use crate::agent_loop::events::RunEventPayload;
use crate::agent_loop::RunStatus;
use crate::agent_loop::TransformContextHookResult;
use crate::context::ContextBudget;
use std::sync::atomic::{AtomicUsize, Ordering};

fn system_messages(events: &[RunEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn output_overflow_reduces_max_tokens_before_retrying() {
    let (runner, requests) = test_runner(ProviderScenario::OutputOverflowThenComplete);
    let (sink, events) = capture_events();

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("overflow me")])
        .with_event_sink(sink)
        .with_context_budget(ContextBudget {
            reserve_output_tokens: 128,
            ..Default::default()
        })
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });
    request.settings.max_tokens = Some(1024);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(
        result.status,
        RunStatus::Completed,
        "overflow recovery should ignore generic retry max_attempts; error: {:?}",
        result.error
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2, "initial overflow + reduced retry");
    assert_eq!(requests[0].settings.max_tokens, Some(1024));
    assert_eq!(
        requests[1].settings.max_tokens,
        Some(128),
        "retry should use the reserved output budget as max_tokens"
    );

    let events = events.lock().expect("event lock");
    let messages = system_messages(&events);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("overflow recovery attempt=0")
                && message.contains("ReduceOutputBudget")),
        "expected explicit overflow recovery lifecycle event, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .all(|message| !message.starts_with("provider retry attempt=")),
        "overflow recovery must not emit generic retry lifecycle text: {messages:?}"
    );
}

#[tokio::test]
async fn classified_untyped_overflow_uses_compaction_lane() {
    let (runner, requests) = test_runner(ProviderScenario::ClassifiedOverflowThenComplete);
    let (sink, events) = capture_events();
    let compaction_calls = std::sync::Arc::new(AtomicUsize::new(0));
    let compaction_calls_for_hook = compaction_calls.clone();

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("overflow me")])
        .with_event_sink(sink)
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });
    request.hooks = RunHooks {
        compaction: Some(std::sync::Arc::new(move |messages, _cancel| {
            let compaction_calls_for_hook = compaction_calls_for_hook.clone();
            Box::pin(async move {
                compaction_calls_for_hook.fetch_add(1, Ordering::SeqCst);
                let latest = messages
                    .last()
                    .cloned()
                    .unwrap_or_else(|| ModelMessage::user("overflow me"));
                Ok(Some(vec![
                    ModelMessage::user("<compaction_summary>trimmed</compaction_summary>"),
                    latest,
                ]))
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(
        result.status,
        RunStatus::Completed,
        "provider-classified untyped overflow should recover through compaction; error: {:?}",
        result.error
    );
    assert_eq!(compaction_calls.load(Ordering::SeqCst), 1);

    let requests = requests.lock().expect("request lock");
    assert_eq!(requests.len(), 2);
    assert!(
        requests[1]
            .messages
            .iter()
            .any(|message| message.text().contains("<compaction_summary>")),
        "recovery request should include compacted context"
    );

    let events = events.lock().expect("event lock");
    let messages = system_messages(&events);
    assert!(
        messages
            .iter()
            .any(|message| message.contains("CompactContext")),
        "expected compaction recovery lifecycle event, got: {messages:?}"
    );
    assert!(
        messages
            .iter()
            .all(|message| !message.starts_with("provider retry attempt=")),
        "overflow recovery must not emit generic retry lifecycle text: {messages:?}"
    );
}

#[tokio::test]
async fn output_overflow_uses_one_reduction_then_fixed_compaction_ladder() {
    let (runner, requests) = test_runner(ProviderScenario::OutputOverflowAlways);
    let (sink, events) = capture_events();
    let compaction_calls = std::sync::Arc::new(AtomicUsize::new(0));
    let compaction_calls_for_hook = compaction_calls.clone();
    let long_input = "x".repeat(4_000);

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user(long_input)])
        .with_event_sink(sink)
        .with_context_budget(ContextBudget {
            reserve_output_tokens: 128,
            ..Default::default()
        })
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            ..Default::default()
        });
    request.settings.max_tokens = Some(1024);
    request.hooks = RunHooks {
        compaction: Some(std::sync::Arc::new(move |_messages, _cancel| {
            let compaction_calls_for_hook = compaction_calls_for_hook.clone();
            Box::pin(async move {
                let call_index = compaction_calls_for_hook.fetch_add(1, Ordering::SeqCst);
                if call_index == 0 {
                    Ok(Some(vec![ModelMessage::user("y".repeat(32))]))
                } else {
                    Ok(Some(vec![ModelMessage::user(
                        "<compaction_summary>trimmed</compaction_summary>",
                    )]))
                }
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("persisted after 3 attempts despite recovery"),
        "expected one reduction + two compactions before exhaustion, got: {:?}",
        result.error
    );
    assert_eq!(compaction_calls.load(Ordering::SeqCst), 2);

    let requests = requests.lock().expect("request lock");
    assert_eq!(
        requests.len(),
        4,
        "initial overflow + reduction + two compaction retries"
    );
    assert_eq!(requests[1].settings.max_tokens, Some(128));
    assert_eq!(requests[2].settings.max_tokens, Some(128));
    assert_eq!(requests[3].settings.max_tokens, Some(128));

    let events = events.lock().expect("event lock");
    let messages = system_messages(&events);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.contains("ReduceOutputBudget"))
            .count(),
        1,
        "overflow lane should reduce output budget once"
    );
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.contains("CompactContext"))
            .count(),
        2,
        "overflow lane should compact twice when the first compaction made enough progress"
    );
    assert!(
        messages
            .iter()
            .all(|message| !message.starts_with("provider retry attempt=")),
        "overflow recovery must not emit generic retry lifecycle text: {messages:?}"
    );
}

#[tokio::test]
async fn compaction_progress_uses_provider_facing_messages_after_transform() {
    let (runner, requests) = test_runner(ProviderScenario::ContextOverflowAlways);
    let compaction_calls = std::sync::Arc::new(AtomicUsize::new(0));
    let compaction_calls_for_hook = compaction_calls.clone();

    let mut request = RunRequest::new(
        test_model(),
        vec![ModelMessage::user("a"), ModelMessage::user("b")],
    )
    .with_retry_backoff(RetryBackoffPolicy {
        max_attempts: 1,
        ..Default::default()
    })
    .with_transform_context(std::sync::Arc::new(|payload| {
        Box::pin(async move {
            let messages = payload
                .messages
                .into_iter()
                .map(|message| ModelMessage::user(message.text().repeat(2_000)))
                .collect();
            Ok(TransformContextHookResult::ReplaceMessages { messages })
        })
    }));

    request.hooks = RunHooks {
        compaction: Some(std::sync::Arc::new(move |messages, _cancel| {
            let compaction_calls_for_hook = compaction_calls_for_hook.clone();
            Box::pin(async move {
                compaction_calls_for_hook.fetch_add(1, Ordering::SeqCst);
                Ok(Some(messages.into_iter().skip(1).collect()))
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Failed);
    assert!(
        result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("persisted after 2 attempts despite recovery"),
        "expected two compactions before exhaustion when transform-expanded provider messages free enough tokens, got: {:?}",
        result.error
    );
    assert_eq!(
        compaction_calls.load(Ordering::SeqCst),
        2,
        "provider-facing progress should allow a second compaction"
    );

    let requests = requests.lock().expect("request lock");
    assert_eq!(
        requests.len(),
        3,
        "initial overflow + two compaction retries"
    );
}
