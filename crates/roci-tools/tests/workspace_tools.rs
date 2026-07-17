use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use roci::error::RociError;
use roci::prelude::{LocalSessionFs, LogicalPath, SessionFs};
use roci::tools::{SandboxProvider, ToolArguments, ToolExecutionContext};
use roci_tools::builtin::{
    grep_tool, list_directory_tool, read_file_tool, shell_tool, write_file_tool,
};

fn args(value: serde_json::Value) -> ToolArguments {
    ToolArguments::new(value)
}

fn workspace_ctx(root: &Path) -> ToolExecutionContext {
    ToolExecutionContext {
        workspace_root: Some(root.canonicalize().expect("canonical workspace")),
        ..ToolExecutionContext::default()
    }
}

struct DenySandbox;

#[async_trait]
impl SandboxProvider for DenySandbox {
    async fn validate_shell_command(
        &self,
        _command: &str,
        _cwd: &LogicalPath,
    ) -> Result<(), RociError> {
        Err(RociError::ToolExecution {
            tool_name: "shell".to_string(),
            message: "sandbox denied command".to_string(),
        })
    }
}

#[tokio::test]
async fn workspace_file_tools_resolve_relative_paths_inside_root() {
    let workspace = tempfile::tempdir().expect("workspace temp dir");
    std::fs::create_dir(workspace.path().join("src")).expect("create src");
    std::fs::write(workspace.path().join("src/input.txt"), "needle\n").expect("write input");
    let ctx = workspace_ctx(workspace.path());

    let read = read_file_tool()
        .execute(&args(serde_json::json!({ "path": "src/input.txt" })), &ctx)
        .await
        .expect("read relative file");
    let write = write_file_tool()
        .execute(
            &args(serde_json::json!({ "path": "out/result.txt", "content": "done" })),
            &ctx,
        )
        .await
        .expect("write relative file");
    let list = list_directory_tool()
        .execute(&args(serde_json::json!({ "path": "src" })), &ctx)
        .await
        .expect("list relative directory");
    let grep = grep_tool()
        .execute(
            &args(serde_json::json!({ "pattern": "needle", "path": "src" })),
            &ctx,
        )
        .await
        .expect("grep relative directory");

    assert_eq!(read["content"], "needle\n");
    assert_eq!(write["path"], "out/result.txt");
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("out/result.txt")).expect("written file"),
        "done"
    );
    assert_eq!(list["count"], 1);
    assert_eq!(grep["exit_code"], 0);
    assert!(grep["output"]
        .as_str()
        .unwrap_or_default()
        .contains("needle"));
}

#[tokio::test]
async fn workspace_file_tools_reject_absolute_and_parent_paths() {
    let workspace = tempfile::tempdir().expect("workspace temp dir");
    let ctx = workspace_ctx(workspace.path());

    let absolute = workspace.path().join("inside.txt");
    std::fs::write(&absolute, "inside").expect("write inside file");
    let read = read_file_tool()
        .execute(
            &args(serde_json::json!({ "path": absolute.to_string_lossy() })),
            &ctx,
        )
        .await;
    let write = write_file_tool()
        .execute(
            &args(serde_json::json!({ "path": "../outside.txt", "content": "no" })),
            &ctx,
        )
        .await;
    let list = list_directory_tool()
        .execute(&args(serde_json::json!({ "path": ".." })), &ctx)
        .await;
    let grep = grep_tool()
        .execute(
            &args(serde_json::json!({ "pattern": "inside", "path": "../" })),
            &ctx,
        )
        .await;

    for result in [read, write, list, grep] {
        let error = result.expect_err("workspace escape should fail");
        assert!(error.to_string().contains("workspace path denied"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn workspace_file_tools_reject_symlink_escape() {
    let parent = tempfile::tempdir().expect("parent temp dir");
    let workspace = parent.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    let outside = parent.path().join("outside.txt");
    std::fs::write(&outside, "secret").expect("write outside file");
    std::os::unix::fs::symlink(&outside, workspace.join("escape.txt")).expect("create symlink");
    let ctx = workspace_ctx(&workspace);

    let error = read_file_tool()
        .execute(&args(serde_json::json!({ "path": "escape.txt" })), &ctx)
        .await
        .expect_err("symlink escape should fail");

    assert!(error.to_string().contains("workspace path denied"));

    let listing = list_directory_tool()
        .execute(&args(serde_json::json!({ "path": "." })), &ctx)
        .await
        .expect("listing must inspect link itself without following target");
    let link = listing["entries"]
        .as_array()
        .and_then(|entries| entries.iter().find(|entry| entry["name"] == "escape.txt"))
        .expect("symlink entry");
    assert_eq!(link["type"], "symlink");
}

#[tokio::test]
async fn workspace_context_takes_precedence_over_session_files() {
    let root = tempfile::tempdir().expect("root temp dir");
    let workspace = root.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    std::fs::write(workspace.join("same.txt"), "workspace").expect("write workspace file");
    let session_fs =
        Arc::new(LocalSessionFs::new(root.path().join("session")).expect("session fs"));
    session_fs
        .write(
            &LogicalPath::parse("work/same.txt").expect("logical path"),
            b"session",
        )
        .expect("write session file");
    let ctx = ToolExecutionContext {
        workspace_root: Some(workspace.canonicalize().expect("canonical workspace")),
        session_fs: Some(session_fs),
        session_cwd: Some(LogicalPath::parse("work").expect("logical cwd")),
        ..ToolExecutionContext::default()
    };

    let result = read_file_tool()
        .execute(&args(serde_json::json!({ "path": "same.txt" })), &ctx)
        .await
        .expect("read workspace file");

    assert_eq!(result["content"], "workspace");
}

#[tokio::test]
async fn workspace_shell_sets_cwd_but_is_not_a_filesystem_sandbox() {
    let parent = tempfile::tempdir().expect("parent temp dir");
    let workspace = parent.path().join("workspace");
    std::fs::create_dir(&workspace).expect("create workspace");
    std::fs::write(parent.path().join("outside.txt"), "outside-visible")
        .expect("write outside file");
    let ctx = workspace_ctx(&workspace);

    let pwd = shell_tool()
        .execute(&args(serde_json::json!({ "command": "pwd" })), &ctx)
        .await
        .expect("run pwd");
    let escaped = shell_tool()
        .execute(
            &args(serde_json::json!({
                "command": "cat \"$(dirname \"$PWD\")/outside.txt\""
            })),
            &ctx,
        )
        .await
        .expect("trusted shell may expand a path outside cwd");
    let absolute = shell_tool()
        .execute(
            &args(serde_json::json!({
                "command": format!("cat {:?}", parent.path().join("outside.txt"))
            })),
            &ctx,
        )
        .await
        .expect("trusted shell may use an absolute host path");

    assert_eq!(
        pwd["output"].as_str().unwrap_or_default().trim(),
        workspace
            .canonicalize()
            .expect("canonical workspace")
            .to_string_lossy()
    );
    assert!(escaped["output"]
        .as_str()
        .unwrap_or_default()
        .contains("outside-visible"));
    assert!(absolute["output"]
        .as_str()
        .unwrap_or_default()
        .contains("outside-visible"));
}

#[tokio::test]
async fn workspace_shell_honors_configured_sandbox_provider() {
    let workspace = tempfile::tempdir().expect("workspace temp dir");
    let ctx = ToolExecutionContext {
        sandbox_provider: Some(Arc::new(DenySandbox)),
        ..workspace_ctx(workspace.path())
    };

    let error = shell_tool()
        .execute(&args(serde_json::json!({ "command": "echo denied" })), &ctx)
        .await
        .expect_err("configured sandbox must run before workspace command");

    assert!(error.to_string().contains("sandbox denied command"));
}
