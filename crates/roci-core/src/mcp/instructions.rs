//! MCP instruction helpers and provenance types.

use std::collections::HashSet;

/// Supported MCP server kinds for identity metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MCPServerKind {
    /// A stdio-connected MCP server.
    Stdio,
    /// A streamable HTTP/SSE MCP server.
    Sse,
    /// Unknown or unspecified server kind.
    #[default]
    Unknown,
}

/// Metadata identifying an MCP server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MCPServerMetadata {
    pub id: String,
    pub label: Option<String>,
    pub kind: MCPServerKind,
}

impl MCPServerMetadata {
    /// Create metadata with a server id.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: None,
            kind: MCPServerKind::Unknown,
        }
    }

    /// Create metadata with a server id and label.
    pub fn with_label(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: Some(label.into()),
            kind: MCPServerKind::Unknown,
        }
    }

    /// Return the label used for prompt rendering.
    pub fn display_label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }
}

/// Instruction payload from an MCP server with provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MCPInstructionSource {
    pub server: MCPServerMetadata,
    pub instructions: String,
}

impl MCPInstructionSource {
    /// Return the label used for prompt rendering.
    pub fn display_label(&self) -> &str {
        self.server.display_label()
    }
}

/// Policy for merging MCP instructions with an existing system prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MCPInstructionMergePolicy {
    /// Append the MCP instruction block after the system prompt.
    #[default]
    AppendBlock,
}

/// Merge MCP instruction sources with an existing system prompt.
pub fn merge_mcp_instructions(
    system_prompt: Option<&str>,
    instructions: &[MCPInstructionSource],
    policy: MCPInstructionMergePolicy,
) -> Option<String> {
    let block = render_mcp_instruction_block(instructions);
    match (system_prompt, block, policy) {
        (None, None, _) => None,
        (Some(prompt), None, _) => Some(prompt.to_string()),
        (None, Some(block), _) => Some(block),
        (Some(prompt), Some(block), MCPInstructionMergePolicy::AppendBlock) => {
            Some(format!("{prompt}\n\n{block}"))
        }
    }
}

/// Render a deterministic MCP instruction block.
pub fn render_mcp_instruction_block(
    instructions: &[MCPInstructionSource],
) -> Option<String> {
    let normalized = normalize_sources(instructions);
    if normalized.is_empty() {
        return None;
    }

    let mut sections = Vec::with_capacity(normalized.len());
    for source in normalized {
        sections.push(format!(
            "[server:{}]\n{}",
            source.display_label(),
            source.instructions
        ));
    }

    Some(format!(
        "MCP server instructions:\n\n{}",
        sections.join("\n\n")
    ))
}

fn normalize_sources(instructions: &[MCPInstructionSource]) -> Vec<MCPInstructionSource> {
    let mut sources = instructions
        .iter()
        .filter(|source| !source.instructions.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>();
    sources.sort_by(|left, right| left.server.id.cmp(&right.server.id));

    let mut seen = HashSet::new();
    sources.retain(|source| {
        let key = (
            source.server.id.clone(),
            source.server.label.clone(),
            source.instructions.clone(),
        );
        seen.insert(key)
    });
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, label: Option<&str>, text: &str) -> MCPInstructionSource {
        let server = match label {
            Some(label) => MCPServerMetadata::with_label(id, label),
            None => MCPServerMetadata::new(id),
        };
        MCPInstructionSource {
            server,
            instructions: text.to_string(),
        }
    }

    #[test]
    fn render_block_orders_by_server_id() {
        let sources = vec![
            source("beta", None, "Use beta tools"),
            source("alpha", None, "Use alpha tools"),
        ];

        let block = render_mcp_instruction_block(&sources).expect("block should render");

        let alpha_pos = block.find("[server:alpha]").expect("alpha label missing");
        let beta_pos = block.find("[server:beta]").expect("beta label missing");
        assert!(alpha_pos < beta_pos);
    }

    #[test]
    fn render_block_uses_label_when_present() {
        let sources = vec![source("alpha", Some("Alpha MCP"), "Alpha instructions")];

        let block = render_mcp_instruction_block(&sources).expect("block should render");

        assert!(block.contains("[server:Alpha MCP]"));
    }

    #[test]
    fn merge_appends_instruction_block() {
        let sources = vec![source("alpha", None, "Alpha instructions")];
        let merged = merge_mcp_instructions(Some("System prompt"), &sources, MCPInstructionMergePolicy::AppendBlock)
            .expect("merged prompt should render");

        assert!(merged.starts_with("System prompt"));
        assert!(merged.contains("MCP server instructions:"));
    }

    #[test]
    fn merge_returns_none_when_empty() {
        let merged = merge_mcp_instructions(None, &[], MCPInstructionMergePolicy::AppendBlock);
        assert!(merged.is_none());
    }
}
