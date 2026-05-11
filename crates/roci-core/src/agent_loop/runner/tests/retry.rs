use super::*;

fn model(model_id: &str) -> LanguageModel {
    LanguageModel::Custom {
        provider: "stub".to_string(),
        model_id: model_id.to_string(),
    }
}

fn retry_events(events: &[RunEvent]) -> Vec<RetryEvent> {
    events
        .iter()
        .filter_map(|event| match &event.payload {
            RunEventPayload::Retry { event } => Some(event.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn retry_events_schedule_resume_then_complete_before_advancing() {
    let (runner, _requests) = test_runner(ProviderScenario::RetryableTimeoutThenComplete);
    let (sink, events) = capture_events();
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")])
        .with_event_sink(sink)
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 2,
            initial_delay_ms: 1,
            multiplier: 1.0,
            jitter_ratio: 0.0,
            max_delay_ms: 1,
        });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let retry_events = retry_events(&events.lock().expect("events lock"));
    assert_eq!(retry_events.len(), 2);
    assert_eq!(retry_events[0].kind, RetryEventKind::RetryScheduled);
    assert_eq!(retry_events[0].next_action, RetryNextAction::Sleep);
    assert_eq!(retry_events[1].kind, RetryEventKind::RetryResuming);
    assert_eq!(
        retry_events[1].next_action,
        RetryNextAction::ResumeSameCandidate
    );
    assert!(events
        .lock()
        .expect("events lock")
        .iter()
        .all(|event| !matches!(event.payload, RunEventPayload::Error { .. })));
    assert!(retry_events[1].elapsed_retry_ms >= retry_events[0].elapsed_retry_ms);
    assert!(retry_events[1].elapsed_retry_ms > 0);
}

#[tokio::test]
async fn stream_timeout_retries_same_candidate_before_advancing() {
    let (runner, requests) = test_runner_by_model(vec![
        ("flaky", ProviderScenario::StreamTimeoutThenComplete),
        ("ok", ProviderScenario::MissingOptionalFields),
    ]);
    let (sink, events) = capture_events();
    let request = RunRequest::with_candidates(
        vec![model("flaky"), model("ok")],
        vec![ModelMessage::user("hello")],
    )
    .unwrap()
    .with_event_sink(sink)
    .with_retry_backoff(RetryBackoffPolicy {
        max_attempts: 2,
        initial_delay_ms: 1,
        multiplier: 1.0,
        jitter_ratio: 0.0,
        max_delay_ms: 1,
    });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(requests.lock().expect("requests lock").len(), 2);
    let retry_events = retry_events(&events.lock().expect("events lock"));
    assert!(retry_events
        .iter()
        .any(|event| event.kind == RetryEventKind::RetryResuming));
    assert!(!retry_events
        .iter()
        .any(|event| event.kind == RetryEventKind::CandidateAdvancing));
}

#[tokio::test]
async fn exhausted_transient_candidate_advances_to_next_candidate() {
    let (runner, _requests) = test_runner_by_model(vec![
        ("timeout", ProviderScenario::RetryableTimeoutExhausted),
        ("ok", ProviderScenario::MissingOptionalFields),
    ]);
    let shared_health = Arc::new(crate::models::SharedModelHealthRegistry::default());
    let health = crate::models::ModelHealthTracker::new_session(shared_health.clone());
    let (sink, events) = capture_events();
    let request = RunRequest::with_candidates(
        vec![model("timeout"), model("ok")],
        vec![ModelMessage::user("hello")],
    )
    .unwrap()
    .with_event_sink(sink)
    .with_model_health_tracker(health)
    .with_retry_backoff(RetryBackoffPolicy {
        max_attempts: 1,
        initial_delay_ms: 1,
        multiplier: 1.0,
        jitter_ratio: 0.0,
        max_delay_ms: 1,
    });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    let retry_events = retry_events(&events.lock().expect("events lock"));
    assert!(retry_events.iter().any(|event| {
        event.kind == RetryEventKind::CandidateAdvancing
            && event.candidate_index == 0
            && event.next_action == RetryNextAction::AdvanceCandidate
            && event.candidates_remaining == 0
    }));
    let source_key = crate::models::ModelHealthKey::from_model(&model("timeout"));
    assert_eq!(
        shared_health.snapshot(&source_key).status,
        crate::models::ModelHealthStatus::Unhealthy
    );
}

#[tokio::test]
async fn single_candidate_retry_exhausted_returns_failure() {
    let (runner, _requests) = test_runner(ProviderScenario::RetryableTimeoutExhausted);
    let (sink, events) = capture_events();
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")])
        .with_event_sink(sink)
        .with_retry_backoff(RetryBackoffPolicy {
            max_attempts: 1,
            initial_delay_ms: 1,
            multiplier: 1.0,
            jitter_ratio: 0.0,
            max_delay_ms: 1,
        });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Failed);
    let retry_events = retry_events(&events.lock().expect("events lock"));
    assert!(retry_events.iter().any(|event| {
        event.kind == RetryEventKind::RetryExhausted
            && event.failure_category == FailureCategory::Timeout
            && event.next_action == RetryNextAction::ReturnFailure
    }));
}

#[tokio::test]
async fn persistent_retry_cancel_does_not_advance_candidate() {
    let (runner, _requests) = test_runner_by_model(vec![
        ("rate", ProviderScenario::RateLimitedExceedsCap),
        ("ok", ProviderScenario::MissingOptionalFields),
    ]);
    let (sink, events) = capture_events();
    let request = RunRequest::with_candidates(
        vec![model("rate"), model("ok")],
        vec![ModelMessage::user("hello")],
    )
    .unwrap()
    .with_event_sink(sink)
    .with_retry_mode(RetryMode::Persistent);

    let mut handle = runner.start(request).await.expect("start run");
    wait_for_retry_event(&events, RetryEventKind::RetryScheduled).await;
    assert!(handle.abort());
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Canceled);
    let retry_events = retry_events(&events.lock().expect("events lock"));
    assert!(retry_events
        .iter()
        .any(|event| event.kind == RetryEventKind::RetryCanceled));
    assert!(!retry_events
        .iter()
        .any(|event| event.kind == RetryEventKind::CandidateAdvancing));
}

#[tokio::test]
async fn bounded_zero_retry_mode_rejects_configuration() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")])
        .with_retry_mode(RetryMode::Bounded { max_attempts: 0 });

    let result = runner.start(request).await;

    assert!(
        matches!(result, Err(RociError::Configuration(message)) if message.contains("max_attempts"))
    );
}

#[tokio::test]
async fn partial_output_failure_does_not_advance_candidate() {
    let (runner, _requests) = test_runner_by_model(vec![
        ("partial", ProviderScenario::TextThenStreamError),
        ("ok", ProviderScenario::MissingOptionalFields),
    ]);
    let (sink, events) = capture_events();
    let request = RunRequest::with_candidates(
        vec![model("partial"), model("ok")],
        vec![ModelMessage::user("hello")],
    )
    .unwrap()
    .with_event_sink(sink)
    .with_retry_backoff(RetryBackoffPolicy {
        max_attempts: 1,
        initial_delay_ms: 1,
        multiplier: 1.0,
        jitter_ratio: 0.0,
        max_delay_ms: 1,
    });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Failed);
    let retry_events = retry_events(&events.lock().expect("events lock"));
    assert!(!retry_events
        .iter()
        .any(|event| event.kind == RetryEventKind::CandidateAdvancing));
    assert!(retry_events.iter().any(|event| {
        event.kind == RetryEventKind::RetryExhausted && event.partial_output_seen
    }));
}

async fn wait_for_retry_event(events: &Arc<std::sync::Mutex<Vec<RunEvent>>>, kind: RetryEventKind) {
    timeout(Duration::from_secs(2), async {
        loop {
            if retry_events(&events.lock().expect("events lock"))
                .iter()
                .any(|event| event.kind == kind)
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("retry event timeout");
}
