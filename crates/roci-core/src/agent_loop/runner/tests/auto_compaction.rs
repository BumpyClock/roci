use super::*;

#[tokio::test]
async fn auto_compaction_triggers_when_context_exceeds_reserved_window() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = calls.clone();
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.hooks = RunHooks {
        compaction: Some(Arc::new(move |_messages, _cancel| {
            let calls_clone = calls_clone.clone();
            Box::pin(async move {
                calls_clone.fetch_add(1, Ordering::SeqCst);
                Ok(None)
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };
    request.auto_compaction = Some(AutoCompactionConfig {
        reserve_tokens: 4096,
    });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn auto_compaction_replaces_messages_before_provider_call() {
    let (runner, requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(
        test_model(),
        vec![
            ModelMessage::system("system must stay"),
            ModelMessage::user("old context"),
            ModelMessage::user("new context"),
        ],
    );
    request.hooks = RunHooks {
        compaction: Some(Arc::new(move |messages, _cancel| {
            Box::pin(async move {
                Ok(Some(vec![
                    messages[0].clone(),
                    ModelMessage::user("<compaction_summary>\nsummary\n</compaction_summary>"),
                    messages
                        .last()
                        .cloned()
                        .expect("compaction input should have latest message"),
                ]))
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };
    request.auto_compaction = Some(AutoCompactionConfig {
        reserve_tokens: 4096,
    });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run should complete without timeout");
    assert_eq!(result.status, RunStatus::Completed);

    let recorded = requests.lock().expect("request lock");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].messages.len(), 3);
    assert_eq!(recorded[0].messages[0].role, crate::types::Role::System);
    assert_eq!(recorded[0].messages[0].text(), "system must stay");
    assert!(recorded[0].messages[1]
        .text()
        .contains("<compaction_summary>"));
    assert_eq!(recorded[0].messages[2].text(), "new context");
}

#[tokio::test]
async fn compaction_failure_fails_run_and_surfaces_error() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.hooks = RunHooks {
        compaction: Some(Arc::new(move |_messages, _cancel| {
            Box::pin(async {
                Err(RociError::InvalidState(
                    "forced compaction failure".to_string(),
                ))
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };
    request.auto_compaction = Some(AutoCompactionConfig {
        reserve_tokens: 4096,
    });

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Failed);
    let error = result.error.unwrap_or_default();
    assert!(error.contains("compaction failed"));
    assert!(error.contains("forced compaction failure"));
}

#[tokio::test]
async fn abort_during_auto_compaction_cancels_compaction_token_and_run() {
    let (runner, _requests) = test_runner(ProviderScenario::MissingOptionalFields);
    let (started_tx, started_rx) = oneshot::channel::<()>();
    let started_tx = Arc::new(std::sync::Mutex::new(Some(started_tx)));
    let compaction_cancel_observed = Arc::new(AtomicBool::new(false));
    let compaction_cancel_observed_for_hook = compaction_cancel_observed.clone();

    let mut request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")]);
    request.hooks = RunHooks {
        compaction: Some(Arc::new(move |_messages, cancel| {
            let started_tx = started_tx.clone();
            let compaction_cancel_observed = compaction_cancel_observed_for_hook.clone();
            Box::pin(async move {
                if let Some(tx) = started_tx.lock().expect("start signal lock").take() {
                    let _ = tx.send(());
                }
                tokio::spawn(async move {
                    cancel.cancelled().await;
                    compaction_cancel_observed.store(true, Ordering::SeqCst);
                });
                std::future::pending::<Result<Option<Vec<ModelMessage>>, RociError>>().await
            })
        })),
        pre_tool_use: None,
        post_tool_use: None,
    };
    request.auto_compaction = Some(AutoCompactionConfig {
        reserve_tokens: 4096,
    });

    let mut handle = runner.start(request).await.expect("start run");
    timeout(Duration::from_secs(1), started_rx)
        .await
        .expect("compaction hook should start")
        .expect("compaction hook start signal should send");
    assert!(handle.abort(), "abort should be accepted");

    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run should complete without timeout");

    assert_eq!(result.status, RunStatus::Canceled);
    timeout(Duration::from_secs(1), async {
        while !compaction_cancel_observed.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("compaction cancel token should be canceled");
}
