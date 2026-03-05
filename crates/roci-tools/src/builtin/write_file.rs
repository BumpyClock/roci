use std::sync::Arc;

use roci::error::RociError;
use roci::tools::tool::{AgentTool, Tool, ToolExecutionContext};
use roci::tools::types::AgentToolParameters;

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
