use std::sync::Arc;

use roci::error::RociError;
use roci::tools::tool::{AgentTool, Tool, ToolApproval, ToolExecutionContext};
use roci::tools::types::AgentToolParameters;

/// Create the `list_directory` tool — lists directory entries.
///
/// Returns a sorted JSON array of entries, each with `name`, `type`
/// (`"file"` | `"dir"` | `"other"`), and `size` in bytes.
pub fn list_directory_tool() -> Arc<dyn Tool> {
    let tool = AgentTool::new(
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
    );
    Arc::new(tool.with_approval(ToolApproval::safe_read_only()))
}
