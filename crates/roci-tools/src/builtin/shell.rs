use std::sync::Arc;

use roci::error::RociError;
use roci::tools::tool::{AgentTool, Tool, ToolApproval, ToolApprovalKind, ToolExecutionContext};
use roci::tools::types::AgentToolParameters;

use super::common::{truncate_utf8, SHELL_OUTPUT_MAX_BYTES, SHELL_TIMEOUT};

/// Create the `shell` tool — executes a shell command via `sh -c`.
///
/// Captures stdout and stderr, applies a 30-second timeout, and truncates
/// output beyond 32 KB to prevent context explosion.
pub fn shell_tool() -> Arc<dyn Tool> {
    let tool = AgentTool::new(
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
    );
    Arc::new(tool.with_approval(ToolApproval::requires_approval(
        ToolApprovalKind::CommandExecution,
    )))
}
