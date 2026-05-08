use std::sync::Arc;

use roci::error::RociError;
use roci::prelude::{LogicalPath, SessionFileKind, SessionFs};
use roci::tools::tool::{AgentTool, Tool, ToolApproval, ToolExecutionContext};
use roci::tools::types::AgentToolParameters;

use super::common::{resolve_session_path, truncate_utf8, GREP_OUTPUT_MAX_BYTES};

/// Create the `grep` tool — searches for a pattern in files.
///
/// Runs `grep -rn` with the given pattern. Output is truncated to 32 KB.
/// When `path` is omitted the search defaults to the current directory.
pub fn grep_tool() -> Arc<dyn Tool> {
    let tool = AgentTool::new(
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
        |args_val, ctx: ToolExecutionContext| async move {
            let pattern = args_val.get_str("pattern")?;
            let path = args_val.get_str_opt("path").unwrap_or(".");

            if let (Some(session_fs), Some(logical_path)) =
                (ctx.session_fs.as_ref(), resolve_session_path(&ctx, path)?)
            {
                let mut result = String::new();
                session_grep(session_fs.as_ref(), &logical_path, pattern, &mut result)?;

                let truncated = result.len() > GREP_OUTPUT_MAX_BYTES;
                if truncated {
                    result = truncate_utf8(&result, GREP_OUTPUT_MAX_BYTES);
                    result.push_str("\n... (truncated)");
                }

                let exit_code = if result.is_empty() { 1 } else { 0 };
                return Ok(serde_json::json!({
                    "exit_code": exit_code,
                    "output": result,
                    "truncated": truncated,
                }));
            }

            let output = tokio::process::Command::new("grep")
                .args(["-rn", "--", pattern, path])
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
    );
    Arc::new(tool.with_approval(ToolApproval::safe_read_only()))
}

fn session_grep(
    session_fs: &(dyn SessionFs + Send + Sync),
    path: &LogicalPath,
    pattern: &str,
    output: &mut String,
) -> Result<(), RociError> {
    match session_fs
        .metadata(path)
        .map_err(|e| RociError::ToolExecution {
            tool_name: "grep".into(),
            message: format!("{path}: {e}"),
        })?
        .kind
    {
        SessionFileKind::File => session_grep_file(session_fs, path, pattern, output),
        SessionFileKind::Directory => {
            for entry in session_fs
                .list(path)
                .map_err(|e| RociError::ToolExecution {
                    tool_name: "grep".into(),
                    message: format!("{path}: {e}"),
                })?
            {
                match entry.metadata.kind {
                    SessionFileKind::File => {
                        session_grep_file(session_fs, &entry.path, pattern, output)?;
                    }
                    SessionFileKind::Directory => {
                        session_grep(session_fs, &entry.path, pattern, output)?;
                    }
                    SessionFileKind::Symlink => {}
                }
            }
            Ok(())
        }
        SessionFileKind::Symlink => Ok(()),
    }
}

fn session_grep_file(
    session_fs: &(dyn SessionFs + Send + Sync),
    path: &LogicalPath,
    pattern: &str,
    output: &mut String,
) -> Result<(), RociError> {
    use std::fmt::Write as _;

    let bytes = session_fs
        .read(path)
        .map_err(|e| RociError::ToolExecution {
            tool_name: "grep".into(),
            message: format!("{path}: {e}"),
        })?;
    let Ok(contents) = String::from_utf8(bytes) else {
        return Ok(());
    };

    for (line_index, line) in contents.lines().enumerate() {
        if line.contains(pattern) {
            writeln!(output, "{path}:{}:{line}", line_index + 1).map_err(|e| {
                RociError::ToolExecution {
                    tool_name: "grep".into(),
                    message: e.to_string(),
                }
            })?;
        }
    }

    Ok(())
}
