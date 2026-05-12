use std::sync::Arc;

use roci::error::RociError;
use roci::tools::arguments::ToolArguments;
use roci::tools::tool::{
    AgentTool, Tool, ToolExecutionContext, ToolSafetyKind, ToolSafetyPlan, ToolSafetySummary,
};
use roci::tools::types::AgentToolParameters;

use super::common::resolve_session_path;

/// Create the `write_file` tool — writes content to a file.
///
/// Creates parent directories when they do not exist. Returns the written
/// byte count and the resolved path.
pub fn write_file_tool() -> Arc<dyn Tool> {
    let tool = AgentTool::new(
        "write_file",
        "Write content to a file, creating parent directories if needed",
        AgentToolParameters::object()
            .string("path", "Path to the file to write", true)
            .string("content", "Content to write to the file", true)
            .build(),
        |args_val, ctx: ToolExecutionContext| async move {
            let path = args_val.get_str("path")?;
            let content = args_val.get_str("content")?;

            if let (Some(session_fs), Some(logical_path)) =
                (ctx.session_fs.as_ref(), resolve_session_path(&ctx, path)?)
            {
                let bytes = content.len();
                session_fs
                    .write(&logical_path, content.as_bytes())
                    .map_err(|e| RociError::ToolExecution {
                        tool_name: "write_file".into(),
                        message: format!("{logical_path}: {e}"),
                    })?;

                return Ok(serde_json::json!({
                    "success": true,
                    "path": logical_path.to_string(),
                    "bytes_written": bytes,
                }));
            }

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
    );
    Arc::new(tool.with_safety(write_file_safety_summary(), write_file_safety))
}

fn write_file_safety(args: &ToolArguments) -> ToolSafetyPlan {
    match args.get_str("path") {
        Ok(path) => ToolSafetyPlan::file_write(path),
        Err(_) => ToolSafetyPlan::approval_required(ToolSafetyKind::FileChange),
    }
}

fn write_file_safety_summary() -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: false,
        destructive_by_default: false,
        concurrency_safe_by_default: false,
        approval_kind: ToolSafetyKind::FileChange,
    }
}
