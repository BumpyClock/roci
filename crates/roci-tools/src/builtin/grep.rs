use std::sync::Arc;

use roci::error::RociError;
use roci::tools::tool::{AgentTool, Tool, ToolApproval, ToolExecutionContext};
use roci::tools::types::AgentToolParameters;

use super::common::{truncate_utf8, GREP_OUTPUT_MAX_BYTES};

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
        |args_val, _ctx: ToolExecutionContext| async move {
            let pattern = args_val.get_str("pattern")?;
            let path = args_val.get_str_opt("path").unwrap_or(".");

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
