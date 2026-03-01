use std::sync::Arc;

use roci::mcp::transport::{SSETransport, StdioTransport};
use roci::mcp::{
    MCPAggregateServer, MCPClient, MCPInstructionSource, MCPServerKind, MCPServerMetadata,
    MCPToolAggregator,
};
use roci::tools::dynamic::DynamicToolProvider;

pub(crate) struct McpRuntimeWiring {
    pub(crate) dynamic_tool_providers: Vec<Arc<dyn DynamicToolProvider>>,
    pub(crate) instructions: Vec<MCPInstructionSource>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct McpStdioServerSpec {
    pub(crate) id: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct McpSseServerSpec {
    pub(crate) id: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) auth_token: Option<String>,
    pub(crate) headers: Vec<(String, String)>,
}

pub(crate) async fn build_mcp_runtime_wiring(
    stdio_specs: &[String],
    sse_specs: &[String],
) -> Result<McpRuntimeWiring, Box<dyn std::error::Error>> {
    if stdio_specs.is_empty() && sse_specs.is_empty() {
        return Ok(McpRuntimeWiring {
            dynamic_tool_providers: Vec::new(),
            instructions: Vec::new(),
        });
    }

    let mut servers = Vec::new();

    for (index, raw_spec) in stdio_specs.iter().enumerate() {
        let spec = parse_mcp_stdio_spec(raw_spec)
            .map_err(|error| format!("Invalid --mcp-stdio spec '{raw_spec}': {error}"))?;
        let command = required_transport_field("--mcp-stdio", raw_spec, "command", spec.command)?;
        let id = optional_non_empty(spec.id).unwrap_or_else(|| format!("stdio-{}", index + 1));
        let metadata = MCPServerMetadata {
            id,
            label: spec.label,
            kind: MCPServerKind::Stdio,
        };
        let transport = StdioTransport::new(command, spec.args);
        let client = MCPClient::new(Box::new(transport));
        servers.push(MCPAggregateServer::with_metadata(metadata, client));
    }

    for (index, raw_spec) in sse_specs.iter().enumerate() {
        let spec = parse_mcp_sse_spec(raw_spec)
            .map_err(|error| format!("Invalid --mcp-sse spec '{raw_spec}': {error}"))?;
        let url = required_transport_field("--mcp-sse", raw_spec, "url", spec.url)?;
        let id = optional_non_empty(spec.id).unwrap_or_else(|| format!("sse-{}", index + 1));
        let metadata = MCPServerMetadata {
            id,
            label: spec.label,
            kind: MCPServerKind::Sse,
        };
        let mut transport = SSETransport::new(url);
        if let Some(token) = spec.auth_token {
            transport = transport.auth_token(token);
        }
        if !spec.headers.is_empty() {
            transport = transport.headers(spec.headers);
        }
        let client = MCPClient::new(Box::new(transport));
        servers.push(MCPAggregateServer::with_metadata(metadata, client));
    }

    let aggregator = Arc::new(MCPToolAggregator::new(servers)?);
    let instructions = aggregator.list_instruction_sources().await?;
    let dynamic_tool_provider: Arc<dyn DynamicToolProvider> = aggregator;

    Ok(McpRuntimeWiring {
        dynamic_tool_providers: vec![dynamic_tool_provider],
        instructions,
    })
}

fn required_transport_field(
    flag: &str,
    raw_spec: &str,
    field_name: &str,
    value: Option<String>,
) -> Result<String, Box<dyn std::error::Error>> {
    optional_non_empty(value).ok_or_else(|| {
        format!("{flag} spec '{raw_spec}' must include non-empty '{field_name}'").into()
    })
}

fn optional_non_empty(value: Option<String>) -> Option<String> {
    value.and_then(|candidate| {
        if candidate.trim().is_empty() {
            None
        } else {
            Some(candidate)
        }
    })
}

pub(crate) fn parse_mcp_stdio_spec(raw: &str) -> Result<McpStdioServerSpec, String> {
    let mut spec = McpStdioServerSpec::default();
    for (key, value) in parse_structured_spec_entries(raw)? {
        match key.as_str() {
            "id" => spec.id = Some(value),
            "label" => spec.label = Some(value),
            "command" | "cmd" => spec.command = Some(value),
            "arg" => spec.args.push(value),
            "args" => {
                spec.args.extend(
                    value
                        .split('|')
                        .filter(|item| !item.is_empty())
                        .map(str::to_string),
                );
            }
            _ => {
                return Err(format!(
                    "unknown key '{key}' (allowed: id,label,command,arg,args)"
                ))
            }
        }
    }
    Ok(spec)
}

pub(crate) fn parse_mcp_sse_spec(raw: &str) -> Result<McpSseServerSpec, String> {
    let mut spec = McpSseServerSpec::default();
    for (key, value) in parse_structured_spec_entries(raw)? {
        match key.as_str() {
            "id" => spec.id = Some(value),
            "label" => spec.label = Some(value),
            "url" => spec.url = Some(value),
            "auth_token" | "token" => spec.auth_token = Some(value),
            "header" => spec.headers.push(parse_sse_header(&value)?),
            _ => {
                return Err(format!(
                    "unknown key '{key}' (allowed: id,label,url,auth_token,header)"
                ))
            }
        }
    }
    Ok(spec)
}

fn parse_structured_spec_entries(raw: &str) -> Result<Vec<(String, String)>, String> {
    let mut entries = Vec::new();
    for segment in raw.split(',') {
        if segment.trim().is_empty() {
            continue;
        }
        let Some((raw_key, raw_value)) = segment.split_once('=') else {
            return Err(format!("entry '{segment}' must use key=value format"));
        };
        let key = raw_key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err(format!("entry '{segment}' has an empty key"));
        }
        entries.push((key, raw_value.to_string()));
    }
    Ok(entries)
}

fn parse_sse_header(raw: &str) -> Result<(String, String), String> {
    let Some((name, value)) = raw.split_once(':') else {
        return Err(format!("header '{raw}' must be Name:Value"));
    };
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("header '{raw}' must include a non-empty name"));
    }
    Ok((name.to_string(), value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{parse_mcp_sse_spec, parse_mcp_stdio_spec};

    #[test]
    fn parse_mcp_stdio_spec_allows_optional_fields_and_repeatable_args() {
        let parsed =
            parse_mcp_stdio_spec("label=Local Files,command=npx,arg=-y,arg=@mcp/server,args=.")
                .expect("stdio spec should parse");

        assert_eq!(parsed.id, None);
        assert_eq!(parsed.label.as_deref(), Some("Local Files"));
        assert_eq!(parsed.command.as_deref(), Some("npx"));
        assert_eq!(parsed.args, vec!["-y", "@mcp/server", "."]);
    }

    #[test]
    fn parse_mcp_sse_spec_parses_headers_and_preserves_label() {
        let parsed = parse_mcp_sse_spec(
            "id=docs,label=Docs Gateway,url=https://example.com/mcp,auth_token=abc,header=x-env:dev,header=x-team:agent",
        )
        .expect("sse spec should parse");

        assert_eq!(parsed.id.as_deref(), Some("docs"));
        assert_eq!(parsed.label.as_deref(), Some("Docs Gateway"));
        assert_eq!(parsed.url.as_deref(), Some("https://example.com/mcp"));
        assert_eq!(parsed.auth_token.as_deref(), Some("abc"));
        assert_eq!(
            parsed.headers,
            vec![
                ("x-env".to_string(), "dev".to_string()),
                ("x-team".to_string(), "agent".to_string())
            ]
        );
    }
}
