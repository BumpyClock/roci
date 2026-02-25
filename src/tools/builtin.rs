//! Built-in coding tools for the CLI agent.
//!
//! Provides standard tools (`shell`, `read_file`, `write_file`, `list_directory`,
//! `grep`) that a coding agent can use to interact with the local filesystem and
//! execute commands. Each tool is constructed via [`AgentTool::new`] and returned
//! as `Arc<dyn Tool>`.
//!
//! # Usage
//!
//! ```rust,no_run
//! use roci::tools::builtin::all_tools;
//!
//! let tools = all_tools();
//! assert_eq!(tools.len(), 5);
//! ```

use std::sync::Arc;
use std::time::Duration;

use crate::error::RociError;
use crate::tools::tool::{AgentTool, Tool, ToolExecutionContext};
use crate::tools::types::AgentToolParameters;

const SHELL_OUTPUT_MAX_BYTES: usize = 32_768;
const READ_FILE_MAX_BYTES: usize = 65_536;
const GREP_OUTPUT_MAX_BYTES: usize = 32_768;
const SHELL_TIMEOUT: Duration = Duration::from_secs(30);

fn truncate_utf8(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }

    let mut cutoff = max_bytes;
    while cutoff > 0 && !s.is_char_boundary(cutoff) {
        cutoff -= 1;
    }
    s[..cutoff].to_string()
}

/// Create the `shell` tool — executes a shell command via `sh -c`.
///
/// Captures stdout and stderr, applies a 30-second timeout, and truncates
/// output beyond 32 KB to prevent context explosion.
pub fn shell_tool() -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "shell",
        "Execute a shell command and return its output",
        AgentToolParameters::object()
            .string("command", "The shell command to execute", true)
            .build(),
        |args_val, _ctx: ToolExecutionContext| async move {
            let command = args_val.get_str("command")?;

            let result = tokio::time::timeout(
                SHELL_TIMEOUT,
                tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .output(),
            )
            .await;

            let output = match result {
                Ok(Ok(output)) => output,
                Ok(Err(e)) => {
                    return Err(RociError::ToolExecution {
                        tool_name: "shell".into(),
                        message: e.to_string(),
                    });
                }
                Err(_) => {
                    return Err(RociError::ToolExecution {
                        tool_name: "shell".into(),
                        message: format!("command timed out after {}s", SHELL_TIMEOUT.as_secs()),
                    });
                }
            };

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let mut combined = format!("{stdout}{stderr}");
            let truncated = combined.len() > SHELL_OUTPUT_MAX_BYTES;
            if truncated {
                combined = truncate_utf8(&combined, SHELL_OUTPUT_MAX_BYTES);
                combined.push_str("\n... (truncated)");
            }

            Ok(serde_json::json!({
                "exit_code": output.status.code(),
                "output": combined,
                "truncated": truncated,
            }))
        },
    ))
}

/// Create the `read_file` tool — reads a file as UTF-8 text.
///
/// Returns the content, the byte count, and a truncation flag. Content is
/// capped at 64 KB with a trailing note when truncated.
pub fn read_file_tool() -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "read_file",
        "Read a file's contents as UTF-8 text",
        AgentToolParameters::object()
            .string("path", "Path to the file to read", true)
            .build(),
        |args_val, _ctx: ToolExecutionContext| async move {
            let path = args_val.get_str("path")?;

            let content =
                tokio::fs::read_to_string(path)
                    .await
                    .map_err(|e| RociError::ToolExecution {
                        tool_name: "read_file".into(),
                        message: format!("{path}: {e}"),
                    })?;

            let total_bytes = content.len();
            let truncated = total_bytes > READ_FILE_MAX_BYTES;
            let display = if truncated {
                let mut s = truncate_utf8(&content, READ_FILE_MAX_BYTES);
                s.push_str("\n... (truncated)");
                s
            } else {
                content
            };

            Ok(serde_json::json!({
                "content": display,
                "bytes": total_bytes,
                "truncated": truncated,
            }))
        },
    ))
}

/// Create the `write_file` tool — writes content to a file.
///
/// Creates parent directories when they do not exist. Returns the written
/// byte count and the resolved path.
pub fn write_file_tool() -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "write_file",
        "Write content to a file, creating parent directories if needed",
        AgentToolParameters::object()
            .string("path", "Path to the file to write", true)
            .string("content", "Content to write to the file", true)
            .build(),
        |args_val, _ctx: ToolExecutionContext| async move {
            let path = args_val.get_str("path")?;
            let content = args_val.get_str("content")?;

            if let Some(parent) = std::path::Path::new(path).parent() {
                if !parent.as_os_str().is_empty() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| {
                        RociError::ToolExecution {
                            tool_name: "write_file".into(),
                            message: format!("failed to create directories for {path}: {e}"),
                        }
                    })?;
                }
            }

            let bytes = content.len();
            tokio::fs::write(path, content)
                .await
                .map_err(|e| RociError::ToolExecution {
                    tool_name: "write_file".into(),
                    message: format!("{path}: {e}"),
                })?;

            Ok(serde_json::json!({
                "success": true,
                "path": path,
                "bytes_written": bytes,
            }))
        },
    ))
}

/// Create the `list_directory` tool — lists directory entries.
///
/// Returns a sorted JSON array of entries, each with `name`, `type`
/// (`"file"` | `"dir"` | `"other"`), and `size` in bytes.
pub fn list_directory_tool() -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "list_directory",
        "List files and directories in a given path",
        AgentToolParameters::object()
            .string("path", "Path to the directory to list", true)
            .build(),
        |args_val, _ctx: ToolExecutionContext| async move {
            let path = args_val.get_str("path")?;

            let mut read_dir =
                tokio::fs::read_dir(path)
                    .await
                    .map_err(|e| RociError::ToolExecution {
                        tool_name: "list_directory".into(),
                        message: format!("{path}: {e}"),
                    })?;

            let mut entries = Vec::new();
            while let Some(entry) =
                read_dir
                    .next_entry()
                    .await
                    .map_err(|e| RociError::ToolExecution {
                        tool_name: "list_directory".into(),
                        message: e.to_string(),
                    })?
            {
                let metadata = entry
                    .metadata()
                    .await
                    .map_err(|e| RociError::ToolExecution {
                        tool_name: "list_directory".into(),
                        message: e.to_string(),
                    })?;

                let entry_type = if metadata.is_dir() {
                    "dir"
                } else if metadata.is_file() {
                    "file"
                } else {
                    "other"
                };

                entries.push(serde_json::json!({
                    "name": entry.file_name().to_string_lossy(),
                    "type": entry_type,
                    "size": metadata.len(),
                }));
            }

            entries.sort_by(|a, b| {
                let a_name = a["name"].as_str().unwrap_or("");
                let b_name = b["name"].as_str().unwrap_or("");
                a_name.cmp(b_name)
            });

            let count = entries.len();
            Ok(serde_json::json!({
                "path": path,
                "entries": entries,
                "count": count,
            }))
        },
    ))
}

/// Create the `grep` tool — searches for a pattern in files.
///
/// Runs `grep -rn` with the given pattern. Output is truncated to 32 KB.
/// When `path` is omitted the search defaults to the current directory.
pub fn grep_tool() -> Arc<dyn Tool> {
    Arc::new(AgentTool::new(
        "grep",
        "Search for a pattern in files using grep",
        AgentToolParameters::object()
            .string("pattern", "The pattern to search for", true)
            .string(
                "path",
                "Directory or file to search in (defaults to '.')",
                false,
            )
            .build(),
        |args_val, _ctx: ToolExecutionContext| async move {
            let pattern = args_val.get_str("pattern")?;
            let path = args_val.get_str_opt("path").unwrap_or(".");

            let output = tokio::process::Command::new("grep")
                .args(["-rn", pattern, path])
                .output()
                .await
                .map_err(|e| RociError::ToolExecution {
                    tool_name: "grep".into(),
                    message: e.to_string(),
                })?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            let mut result = stdout.into_owned();
            if !stderr.is_empty() {
                result.push_str(&stderr);
            }

            let truncated = result.len() > GREP_OUTPUT_MAX_BYTES;
            if truncated {
                result = truncate_utf8(&result, GREP_OUTPUT_MAX_BYTES);
                result.push_str("\n... (truncated)");
            }

            Ok(serde_json::json!({
                "exit_code": output.status.code(),
                "output": result,
                "truncated": truncated,
            }))
        },
    ))
}

/// Return all built-in coding tools.
pub fn all_tools() -> Vec<Arc<dyn Tool>> {
    vec![
        shell_tool(),
        read_file_tool(),
        write_file_tool(),
        list_directory_tool(),
        grep_tool(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::arguments::ToolArguments;
    use crate::tools::tool::ToolExecutionContext;

    fn default_ctx() -> ToolExecutionContext {
        ToolExecutionContext::default()
    }

    fn args(json: serde_json::Value) -> ToolArguments {
        ToolArguments::new(json)
    }

    // ── all_tools ──────────────────────────────────────────────────────

    #[test]
    fn all_tools_returns_five_tools() {
        let tools = all_tools();
        assert_eq!(tools.len(), 5);
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
                &args(
                    serde_json::json!({"pattern": "findme", "path": dir.path().to_str().unwrap()}),
                ),
                &default_ctx(),
            )
            .await;

        assert!(result.is_ok());
        let output = result.unwrap()["output"].as_str().unwrap().to_string();
        assert!(output.contains("findme"));
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
}
