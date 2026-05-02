use std::sync::Arc;
use std::time::Duration;

use roci::error::RociError;
use roci::tools::arguments::ToolArguments;
use roci::tools::tool::{AgentTool, Tool, ToolApproval, ToolApprovalKind, ToolExecutionContext};
use roci::tools::types::AgentToolParameters;

use super::common::{truncate_utf8, READ_FILE_MAX_BYTES, SHELL_OUTPUT_MAX_BYTES};
use super::*;

fn default_ctx() -> ToolExecutionContext {
    ToolExecutionContext::default()
}

fn args(json: serde_json::Value) -> ToolArguments {
    ToolArguments::new(json)
}

// ── all_tools ──────────────────────────────────────────────────────

#[test]
fn all_tools_returns_six_tools() {
    let tools = all_tools();
    assert_eq!(tools.len(), 6);
}

#[test]
fn all_tools_contains_expected_names() {
    let tools = all_tools();
    let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
    assert!(names.contains(&"shell"));
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"list_directory"));
    assert!(names.contains(&"grep"));
}

#[test]
fn tool_catalog_marks_all_tools_as_builtin() {
    let catalog = tool_catalog();
    let descriptors = catalog.descriptors();

    assert_eq!(descriptors.len(), 6);
    assert!(descriptors
        .iter()
        .all(|descriptor| descriptor.origin == roci::tools::ToolOrigin::Builtin));
}

#[test]
fn builtins_declare_expected_approval_metadata() {
    assert_eq!(read_file_tool().approval(), ToolApproval::safe_read_only());
    assert_eq!(
        list_directory_tool().approval(),
        ToolApproval::safe_read_only()
    );
    assert_eq!(grep_tool().approval(), ToolApproval::safe_read_only());
    assert_eq!(ask_user_tool().approval(), ToolApproval::safe_host_input());
    assert_eq!(
        shell_tool().approval(),
        ToolApproval::requires_approval(ToolApprovalKind::CommandExecution)
    );
    assert_eq!(
        write_file_tool().approval(),
        ToolApproval::requires_approval(ToolApprovalKind::FileChange)
    );
}

// ── shell ──────────────────────────────────────────────────────────

#[tokio::test]
async fn shell_executes_echo_and_returns_output() {
    let tool = shell_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"command": "echo hello"})),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert_eq!(result["exit_code"], 0);
    assert!(result["output"].as_str().unwrap().contains("hello"));
}

#[tokio::test]
async fn shell_captures_stderr() {
    let tool = shell_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"command": "echo err >&2"})),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert!(result["output"].as_str().unwrap().contains("err"));
}

#[tokio::test]
async fn shell_returns_nonzero_exit_code() {
    let tool = shell_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"command": "exit 42"})),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert_eq!(result["exit_code"], 42);
}

#[tokio::test]
async fn shell_truncates_large_output() {
    let tool = shell_tool();
    let cmd = format!(
        "python3 -c \"print('x' * {})\"",
        SHELL_OUTPUT_MAX_BYTES + 1000
    );
    let result = tool
        .execute(&args(serde_json::json!({"command": cmd})), &default_ctx())
        .await
        .unwrap();

    assert!(result["truncated"].as_bool().unwrap());
    assert!(result["output"]
        .as_str()
        .unwrap()
        .ends_with("... (truncated)"));
}

#[test]
fn truncate_utf8_never_splits_codepoints() {
    let s = "ab😀cd";
    assert_eq!(truncate_utf8(s, 0), "");
    assert_eq!(truncate_utf8(s, 1), "a");
    assert_eq!(truncate_utf8(s, 2), "ab");
    // 3/4/5 would cut into 😀 (4-byte codepoint), so must back off to "ab".
    assert_eq!(truncate_utf8(s, 3), "ab");
    assert_eq!(truncate_utf8(s, 4), "ab");
    assert_eq!(truncate_utf8(s, 5), "ab");
    assert_eq!(truncate_utf8(s, 6), "ab😀");
}

#[tokio::test]
async fn shell_fails_on_missing_command_argument() {
    let tool = shell_tool();
    let result = tool
        .execute(&args(serde_json::json!({})), &default_ctx())
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn shell_times_out_on_long_running_command() {
    let tool = Arc::new(AgentTool::new(
        "shell_short_timeout",
        "shell with short timeout for testing",
        AgentToolParameters::object()
            .string("command", "The shell command to execute", true)
            .build(),
        |args_val, _ctx: ToolExecutionContext| async move {
            let command = args_val.get_str("command")?;
            let timeout = Duration::from_millis(100);
            let result = tokio::time::timeout(
                timeout,
                tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .output(),
            )
            .await;

            match result {
                Ok(Ok(output)) => Ok(serde_json::json!({"exit_code": output.status.code()})),
                Ok(Err(e)) => Err(RociError::ToolExecution {
                    tool_name: "shell".into(),
                    message: e.to_string(),
                }),
                Err(_) => Err(RociError::ToolExecution {
                    tool_name: "shell".into(),
                    message: "command timed out".into(),
                }),
            }
        },
    ));

    let result = tool
        .execute(
            &args(serde_json::json!({"command": "sleep 10"})),
            &default_ctx(),
        )
        .await;

    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("timed out"),
        "expected timeout error, got: {err_msg}"
    );
}

// ── read_file ──────────────────────────────────────────────────────

#[tokio::test]
async fn read_file_returns_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("hello.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let tool = read_file_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"path": file_path.to_str().unwrap()})),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert_eq!(result["content"].as_str().unwrap(), "hello world");
    assert_eq!(result["bytes"], 11);
    assert_eq!(result["truncated"], false);
}

#[tokio::test]
async fn read_file_returns_error_for_nonexistent_file() {
    let tool = read_file_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"path": "/tmp/roci_nonexistent_file_abc123"})),
            &default_ctx(),
        )
        .await;

    assert!(result.is_err());
}

#[tokio::test]
async fn read_file_truncates_large_content() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("large.txt");
    let content = "x".repeat(READ_FILE_MAX_BYTES + 1000);
    std::fs::write(&file_path, &content).unwrap();

    let tool = read_file_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"path": file_path.to_str().unwrap()})),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert!(result["truncated"].as_bool().unwrap());
    assert!(result["content"]
        .as_str()
        .unwrap()
        .ends_with("... (truncated)"));
    assert_eq!(result["bytes"].as_u64().unwrap(), content.len() as u64);
}

// ── write_file ─────────────────────────────────────────────────────

#[tokio::test]
async fn write_file_creates_file_with_content() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("out.txt");

    let tool = write_file_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "hello roci",
            })),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert_eq!(result["success"], true);
    assert_eq!(result["bytes_written"], 10);
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "hello roci");
}

#[tokio::test]
async fn write_file_creates_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("a").join("b").join("c.txt");

    let tool = write_file_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({
                "path": file_path.to_str().unwrap(),
                "content": "nested",
            })),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert_eq!(result["success"], true);
    assert_eq!(std::fs::read_to_string(&file_path).unwrap(), "nested");
}

#[tokio::test]
async fn write_file_returns_path_in_response() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("file.txt");
    let path_str = file_path.to_str().unwrap();

    let tool = write_file_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"path": path_str, "content": "data"})),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert_eq!(result["path"].as_str().unwrap(), path_str);
}

// ── list_directory ─────────────────────────────────────────────────

#[tokio::test]
async fn list_directory_returns_files_and_dirs() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file.txt"), "content").unwrap();
    std::fs::create_dir(dir.path().join("subdir")).unwrap();

    let tool = list_directory_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"path": dir.path().to_str().unwrap()})),
            &default_ctx(),
        )
        .await
        .unwrap();

    let entries = result["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2);

    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"file.txt"));
    assert!(names.contains(&"subdir"));

    let file_entry = entries.iter().find(|e| e["name"] == "file.txt").unwrap();
    assert_eq!(file_entry["type"], "file");
    assert_eq!(file_entry["size"], 7);

    let dir_entry = entries.iter().find(|e| e["name"] == "subdir").unwrap();
    assert_eq!(dir_entry["type"], "dir");
}

#[tokio::test]
async fn list_directory_returns_sorted_entries() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("zebra.txt"), "").unwrap();
    std::fs::write(dir.path().join("alpha.txt"), "").unwrap();
    std::fs::write(dir.path().join("middle.txt"), "").unwrap();

    let tool = list_directory_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"path": dir.path().to_str().unwrap()})),
            &default_ctx(),
        )
        .await
        .unwrap();

    let names: Vec<&str> = result["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["alpha.txt", "middle.txt", "zebra.txt"]);
}

#[tokio::test]
async fn list_directory_returns_count() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "").unwrap();
    std::fs::write(dir.path().join("b.txt"), "").unwrap();

    let tool = list_directory_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"path": dir.path().to_str().unwrap()})),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert_eq!(result["count"], 2);
}

#[tokio::test]
async fn list_directory_returns_error_for_nonexistent_path() {
    let tool = list_directory_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"path": "/tmp/roci_nonexistent_dir_xyz999"})),
            &default_ctx(),
        )
        .await;

    assert!(result.is_err());
}

// ── grep ───────────────────────────────────────────────────────────

#[tokio::test]
async fn grep_finds_matching_lines_in_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("test.txt"),
        "hello world\nfoo bar\nhello again\n",
    )
    .unwrap();

    let tool = grep_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({
                "pattern": "hello",
                "path": dir.path().to_str().unwrap(),
            })),
            &default_ctx(),
        )
        .await
        .unwrap();

    let output = result["output"].as_str().unwrap();
    assert!(output.contains("hello world"));
    assert!(output.contains("hello again"));
    assert_eq!(result["exit_code"], 0);
}

#[tokio::test]
async fn grep_returns_exit_code_one_for_no_matches() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("empty_match.txt"), "nothing here\n").unwrap();

    let tool = grep_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({
                "pattern": "ZZZZNOTFOUND",
                "path": dir.path().to_str().unwrap(),
            })),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert_eq!(result["exit_code"], 1);
}

#[tokio::test]
async fn grep_uses_current_directory_when_path_omitted() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("needle.txt"), "findme\n").unwrap();

    let tool = grep_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"pattern": "findme", "path": dir.path().to_str().unwrap()})),
            &default_ctx(),
        )
        .await;

    assert!(result.is_ok());
    let output = result.unwrap()["output"].as_str().unwrap().to_string();
    assert!(output.contains("findme"));
}

#[tokio::test]
async fn grep_treats_dash_prefixed_pattern_as_pattern_not_flag() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("dash.txt"), "-foo\nbar\n").unwrap();

    let tool = grep_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({
                "pattern": "-foo",
                "path": dir.path().to_str().unwrap(),
            })),
            &default_ctx(),
        )
        .await
        .unwrap();

    assert_eq!(result["exit_code"], 0);
    assert!(result["output"].as_str().unwrap().contains("-foo"));
}

// ── tool metadata ──────────────────────────────────────────────────

#[test]
fn each_tool_has_nonempty_description() {
    for tool in all_tools() {
        assert!(
            !tool.description().is_empty(),
            "tool '{}' has empty description",
            tool.name()
        );
    }
}

#[test]
fn each_tool_has_object_parameter_schema() {
    for tool in all_tools() {
        assert_eq!(
            tool.parameters().schema["type"],
            "object",
            "tool '{}' schema type is not 'object'",
            tool.name()
        );
    }
}

// ── ask_user ────────────────────────────────────────────────────────

#[tokio::test]
async fn ask_user_rejects_missing_kind() {
    let tool = ask_user_tool();
    let result = tool
        .execute(&args(serde_json::json!({})), &default_ctx())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ask_user_rejects_unsupported_kind() {
    let tool = ask_user_tool();
    let result = tool
        .execute(&args(serde_json::json!({"kind": "mode"})), &default_ctx())
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ask_user_rejects_missing_question() {
    let tool = ask_user_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"kind": "question", "id": "q1"})),
            &default_ctx(),
        )
        .await;
    assert!(result.is_err());
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn ask_user_returns_error_without_callback() {
    let tool = ask_user_tool();
    let result = tool
        .execute(
            &args(serde_json::json!({"kind": "question", "id": "q1", "question": "Name?"})),
            &default_ctx(),
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("callback"));
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn ask_user_returns_response_with_callback() {
    let tool = ask_user_tool();
    let ctx = ToolExecutionContext {
        request_user_input: Some(Arc::new(|req| {
            Box::pin(async move {
                Ok(roci::tools::UserInputResponse {
                    request_id: req.request_id,
                    result: roci::tools::UserInputResult::Question {
                        answer: "Alice".into(),
                    },
                })
            })
        })),
        ..default_ctx()
    };
    let result = tool
        .execute(
            &args(serde_json::json!({"kind": "question", "id": "q1", "question": "Name?"})),
            &ctx,
        )
        .await
        .unwrap();
    assert_eq!(result["result"]["kind"], "question");
    assert_eq!(result["result"]["answer"], "Alice");
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn ask_user_parses_all_semantic_prompt_kinds() {
    use std::sync::Mutex;

    let seen = Arc::new(Mutex::new(Vec::new()));
    let tool = ask_user_tool();
    let ctx = ToolExecutionContext {
        request_user_input: Some({
            let seen = Arc::clone(&seen);
            Arc::new(move |req| {
                let seen = Arc::clone(&seen);
                Box::pin(async move {
                    seen.lock().expect("seen lock").push(req.prompt.clone());
                    Ok(roci::tools::UserInputResponse {
                        request_id: req.request_id,
                        result: roci::tools::UserInputResult::Canceled,
                    })
                })
            })
        }),
        ..default_ctx()
    };

    let payloads = [
        serde_json::json!({"kind": "question", "question": "Name?"}),
        serde_json::json!({"kind": "confirm", "question": "Continue?", "default": true}),
        serde_json::json!({
            "kind": "choice",
            "question": "Unit?",
            "choices": [{"id": "c", "label": "Celsius"}]
        }),
        serde_json::json!({
            "kind": "multi_choice",
            "question": "Tools?",
            "choices": [{"id": "fmt", "label": "Format"}],
            "default": ["fmt"]
        }),
        serde_json::json!({
            "kind": "form",
            "title": "Profile",
            "fields": [
                {"id": "name", "label": "Name", "input_kind": "text", "required": true},
                {
                    "id": "unit",
                    "label": "Unit",
                    "input_kind": "choice",
                    "choices": [{"id": "c", "label": "Celsius"}]
                }
            ]
        }),
    ];

    for payload in payloads {
        tool.execute(&args(payload), &ctx).await.unwrap();
    }

    let seen = seen.lock().expect("seen lock");
    assert!(matches!(
        seen[0],
        roci::tools::AskUserPrompt::Question { .. }
    ));
    assert!(matches!(
        seen[1],
        roci::tools::AskUserPrompt::Confirm { .. }
    ));
    assert!(matches!(seen[2], roci::tools::AskUserPrompt::Choice { .. }));
    assert!(matches!(
        seen[3],
        roci::tools::AskUserPrompt::MultiChoice { .. }
    ));
    assert!(matches!(seen[4], roci::tools::AskUserPrompt::Form { .. }));
}

#[cfg(feature = "agent")]
#[tokio::test]
async fn ask_user_surfaces_interactive_prompt_unavailable_error() {
    let tool = ask_user_tool();
    let ctx = ToolExecutionContext {
        request_user_input: Some(Arc::new(|req| {
            Box::pin(async move {
                Err(roci::tools::UserInputError::InteractivePromptUnavailable {
                    request_id: req.request_id,
                    reason: "stdin is not an interactive terminal".into(),
                })
            })
        })),
        ..default_ctx()
    };
    let result = tool
        .execute(
            &args(serde_json::json!({"kind": "question", "id": "q1", "question": "Name?"})),
            &ctx,
        )
        .await;

    let err = result.expect_err("interactive prompt failure should surface");
    assert!(err.to_string().contains("interactive prompt unavailable"));
}
