use super::*;

#[tokio::test]
async fn run_request_threads_workspace_root_to_tools() {
    let (runner, _requests) = test_runner(ProviderScenario::ToolCallWithUsageThenTextWithUsage);
    let workspace = tempfile::tempdir().expect("workspace temp dir");
    let workspace_root = workspace
        .path()
        .canonicalize()
        .expect("canonical workspace");
    let configured_root = workspace.path().join(".");
    let observed_root = Arc::new(std::sync::Mutex::new(None));
    let tool_observed_root = Arc::clone(&observed_root);
    let tool: Arc<dyn Tool> = Arc::new(AgentTool::new(
        "noop_tool",
        "records workspace context",
        AgentToolParameters::empty(),
        move |_args, ctx: ToolExecutionContext| {
            let tool_observed_root = Arc::clone(&tool_observed_root);
            async move {
                *tool_observed_root.lock().expect("workspace lock") = ctx.workspace_root;
                Ok(serde_json::json!({ "ok": true }))
            }
        },
    ));
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("run tool")])
        .with_tools(vec![tool])
        .with_approval_policy(ApprovalPolicy::always())
        .with_workspace_root(configured_root);

    let handle = runner.start(request).await.expect("start run");
    let result = timeout(Duration::from_secs(2), handle.wait())
        .await
        .expect("run wait timeout");

    assert_eq!(result.status, RunStatus::Completed);
    assert_eq!(
        observed_root.lock().expect("workspace lock").as_ref(),
        Some(&workspace_root)
    );
}

#[tokio::test]
async fn direct_runner_rejects_missing_workspace_root() {
    let (runner, _requests) = test_runner(ProviderScenario::TextOnlyWithUsage);
    let workspace = tempfile::tempdir().expect("workspace temp dir");
    let missing_root = workspace.path().join("missing");
    let request = RunRequest::new(test_model(), vec![ModelMessage::user("hello")])
        .with_workspace_root(missing_root);

    let error = runner
        .start(request)
        .await
        .expect_err("missing workspace must fail before run start");

    assert!(matches!(error, RociError::Configuration(_)));
    assert!(error
        .to_string()
        .contains("failed to canonicalize workspace root"));
}
