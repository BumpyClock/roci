use super::*;

#[tokio::test]
async fn run_parallel_waits_for_spawned_children_and_honors_concurrency_limit() {
    let (supervisor, max_active) = make_delayed_supervisor(SubagentSupervisorConfig {
        max_concurrent: 2,
        ..Default::default()
    });

    let completions = supervisor
        .run_parallel(vec![
            delayed_spec("one", "medium one"),
            delayed_spec("two", "medium two"),
            delayed_spec("three", "medium three"),
        ])
        .await
        .expect("parallel run should complete");

    assert_eq!(completions.len(), 3);
    assert!(completions
        .iter()
        .all(|completion| completion.result.status == SubagentStatus::Completed));
    assert_eq!(
        max_active.load(Ordering::SeqCst),
        2,
        "semaphore should cap provider work at max_concurrent"
    );
    assert!(supervisor.list_active().await.is_empty());
}

#[tokio::test]
async fn spawn_rejects_zero_max_concurrent_guardrail() {
    let (supervisor, _) = make_delayed_supervisor(SubagentSupervisorConfig {
        max_concurrent: 0,
        ..Default::default()
    });

    let err = match supervisor
        .spawn(delayed_spec("blocked", "medium task"))
        .await
    {
        Ok(_) => panic!("zero max_concurrent should fail before a child can hang"),
        Err(err) => err,
    };

    assert!(err.to_string().contains("max_concurrent"));
}

#[tokio::test]
async fn race_returns_first_completion_and_aborts_remaining_children() {
    let (supervisor, _) = make_delayed_supervisor(SubagentSupervisorConfig {
        max_concurrent: 2,
        ..Default::default()
    });

    let completion = supervisor
        .race(vec![
            delayed_spec("slow-worker", "slow task"),
            delayed_spec("fast-worker", "fast task"),
        ])
        .await
        .expect("race should run")
        .expect("race should return first child");

    assert_eq!(completion.label.as_deref(), Some("fast-worker"));
    assert_eq!(completion.result.status, SubagentStatus::Completed);
    assert!(supervisor.list_active().await.is_empty());
}

#[tokio::test]
async fn watch_all_streams_snapshots_until_all_current_children_are_terminal() {
    let (supervisor, _) = make_delayed_supervisor(SubagentSupervisorConfig {
        max_concurrent: 2,
        ..Default::default()
    });
    let first = supervisor
        .spawn(delayed_spec("first", "fast task"))
        .await
        .expect("first child should spawn");
    let second = supervisor
        .spawn(delayed_spec("second", "slow task"))
        .await
        .expect("second child should spawn");

    let mut snapshots = supervisor.watch_all().await;
    let mut terminal_ids = std::collections::HashSet::new();

    while terminal_ids.len() < 2 {
        let snapshot = tokio::time::timeout(Duration::from_secs(2), snapshots.next())
            .await
            .expect("watch_all should produce terminal snapshots")
            .expect("watch_all stream should stay open until both children finish");
        if is_terminal(snapshot.status) {
            terminal_ids.insert(snapshot.subagent_id);
        }
    }

    assert!(terminal_ids.contains(&first.id()));
    assert!(terminal_ids.contains(&second.id()));
}

#[tokio::test]
async fn watch_any_streams_until_first_current_child_is_terminal() {
    let (supervisor, _) = make_delayed_supervisor(SubagentSupervisorConfig {
        max_concurrent: 2,
        ..Default::default()
    });
    let slow = supervisor
        .spawn(delayed_spec("slow-worker", "slow task"))
        .await
        .expect("slow child should spawn");
    let fast = supervisor
        .spawn(delayed_spec("fast-worker", "fast task"))
        .await
        .expect("fast child should spawn");

    let mut snapshots = supervisor.watch_any().await;
    let terminal = loop {
        let snapshot = tokio::time::timeout(Duration::from_secs(2), snapshots.next())
            .await
            .expect("watch_any should produce a terminal snapshot")
            .expect("watch_any stream should stay open until one child finishes");
        if is_terminal(snapshot.status) {
            break snapshot;
        }
    };

    assert_eq!(terminal.subagent_id, fast.id());
    assert_eq!(terminal.status, SubagentStatus::Completed);

    slow.abort().await;
    let _ = slow.wait().await;
}
