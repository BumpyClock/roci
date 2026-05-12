use std::sync::Arc;

use roci::error::RociError;
use roci::tools::arguments::ToolArguments;
use roci::tools::tool::{
    AgentTool, Tool, ToolExecutionContext, ToolSafetyKind, ToolSafetyPlan, ToolSafetySummary,
};
use roci::tools::types::AgentToolParameters;

use super::common::{resolve_session_path, truncate_utf8, READ_FILE_MAX_BYTES};

/// Create the `read_file` tool — reads a file as UTF-8 text.
///
/// Returns the content, the byte count, and a truncation flag. Content is
/// capped at 64 KB with a trailing note when truncated.
pub fn read_file_tool() -> Arc<dyn Tool> {
    let tool = AgentTool::new(
        "read_file",
        "Read a file's contents as UTF-8 text",
        AgentToolParameters::object()
            .string("path", "Path to the file to read", true)
            .build(),
        |args_val, ctx: ToolExecutionContext| async move {
            let path = args_val.get_str("path")?;

            let content = if let (Some(session_fs), Some(path)) =
                (ctx.session_fs.as_ref(), resolve_session_path(&ctx, path)?)
            {
                let bytes = session_fs
                    .read(&path)
                    .map_err(|e| RociError::ToolExecution {
                        tool_name: "read_file".into(),
                        message: format!("{path}: {e}"),
                    })?;
                String::from_utf8(bytes).map_err(|e| RociError::ToolExecution {
                    tool_name: "read_file".into(),
                    message: format!("{path}: {e}"),
                })?
            } else {
                tokio::fs::read_to_string(path)
                    .await
                    .map_err(|e| RociError::ToolExecution {
                        tool_name: "read_file".into(),
                        message: format!("{path}: {e}"),
                    })?
            };

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
    );
    Arc::new(tool.with_safety(read_file_safety_summary(), read_file_safety))
}

fn read_file_safety(args: &ToolArguments) -> ToolSafetyPlan {
    match args.get_str("path") {
        Ok(path) => ToolSafetyPlan::file_read(path),
        Err(_) => ToolSafetyPlan::safe_read_only(ToolSafetyKind::Read),
    }
}

fn read_file_safety_summary() -> ToolSafetySummary {
    ToolSafetySummary {
        read_only_by_default: true,
        destructive_by_default: false,
        concurrency_safe_by_default: true,
        approval_kind: ToolSafetyKind::Read,
    }
}
