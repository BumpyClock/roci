use std::sync::Arc;

use roci::error::RociError;
use roci::security::command::classify_shell_command;
use roci::tools::arguments::ToolArguments;
use roci::tools::tool::{
    AgentTool, Tool, ToolExecutionContext, ToolSafetyKind, ToolSafetyPlan, ToolSafetySummary,
};
use roci::tools::types::AgentToolParameters;

use super::common::{
    truncate_utf8, validate_session_shell_command, SHELL_OUTPUT_MAX_BYTES, SHELL_TIMEOUT,
};

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
        |args_val, ctx: ToolExecutionContext| async move {
            let command = args_val.get_str("command")?;

            let mut process = tokio::process::Command::new("sh");
            process.arg("-c").arg(command);

            if let (Some(session_fs), Some(session_cwd)) =
                (ctx.session_fs.as_ref(), ctx.session_cwd.as_ref())
            {
                if let Some(provider) = ctx.sandbox_provider.as_ref() {
                    provider
                        .validate_shell_command(command, session_cwd)
                        .await?;
                }

                validate_session_shell_command(command).map_err(|reason| {
                    RociError::ToolExecution {
                        tool_name: "shell".into(),
                        message: format!("session shell command denied: {reason}"),
                    }
                })?;

                let cwd = session_fs.files_root().join(session_cwd.to_path_buf());
                tokio::fs::create_dir_all(&cwd)
                    .await
                    .map_err(|e| RociError::ToolExecution {
                        tool_name: "shell".into(),
                        message: format!("{}: {e}", cwd.display()),
                    })?;
                process.current_dir(cwd);
            }

            let result = tokio::time::timeout(SHELL_TIMEOUT, process.output()).await;

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
    Arc::new(tool.with_safety(shell_safety_summary(), shell_safety))
}

fn shell_safety(args: &ToolArguments) -> ToolSafetyPlan {
    match args.get_str("command") {
        Ok(command) => ToolSafetyPlan::from_command_insight(classify_shell_command(command)),
        Err(err) => {
            let mut plan = ToolSafetyPlan::approval_required(ToolSafetyKind::CommandExecution);
            plan.approval.reason = Some(format!("invalid shell arguments: {err}"));
            plan
        }
    }
}

fn shell_safety_summary() -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: false,
        destructive_by_default: false,
        concurrency_safe_by_default: false,
        approval_kind: ToolSafetyKind::CommandExecution,
    }
}
