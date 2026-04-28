use std::sync::Arc;

use roci::error::RociError;
use roci::tools::tool::{AgentTool, Tool, ToolApproval, ToolExecutionContext};
use roci::tools::types::AgentToolParameters;

use super::common::{truncate_utf8, READ_FILE_MAX_BYTES};

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
    );
    Arc::new(tool.with_approval(ToolApproval::safe_read_only()))
}
