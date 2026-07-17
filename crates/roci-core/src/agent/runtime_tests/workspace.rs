use super::support::*;
use super::*;

#[test]
fn try_new_canonicalizes_workspace_root() {
    let parent = tempfile::tempdir().expect("parent temp dir");
    let workspace = parent.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    let configured = workspace.join(".");
    let mut config = test_agent_config();
    config.workspace_root = Some(configured);

    let agent = AgentRuntime::try_new(test_registry(), test_config(), config)
        .expect("valid workspace should construct runtime");

    assert_eq!(
        agent.config.workspace_root.as_ref(),
        Some(&workspace.canonicalize().expect("canonical workspace"))
    );
}

#[test]
fn try_new_rejects_missing_workspace_root() {
    let parent = tempfile::tempdir().expect("parent temp dir");
    let missing = parent.path().join("missing");
    let mut config = test_agent_config();
    config.workspace_root = Some(missing);

    let error = match AgentRuntime::try_new(test_registry(), test_config(), config) {
        Ok(_) => panic!("missing workspace should fail"),
        Err(error) => error,
    };

    assert!(matches!(error, RociError::Configuration(_)));
    assert!(error.to_string().contains("workspace root"));
}

#[test]
fn try_new_rejects_workspace_root_that_is_not_a_directory() {
    let parent = tempfile::tempdir().expect("parent temp dir");
    let file = parent.path().join("file.txt");
    std::fs::write(&file, "not a workspace").expect("write file");
    let mut config = test_agent_config();
    config.workspace_root = Some(file);

    let error = match AgentRuntime::try_new(test_registry(), test_config(), config) {
        Ok(_) => panic!("file workspace should fail"),
        Err(error) => error,
    };

    assert!(matches!(error, RociError::Configuration(_)));
    assert!(error.to_string().contains("not a directory"));
}
