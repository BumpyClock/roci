use std::sync::Arc;

use roci::mcp::transport::{StdioTransport, StreamableHttpTransport, WebSocketTransport};
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
pub(crate) struct McpRemoteServerSpec {
    pub(crate) id: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) url: Option<String>,
    pub(crate) auth_token: Option<String>,
    pub(crate) headers: Vec<(String, String)>,
}

pub(crate) async fn build_mcp_runtime_wiring(
    stdio_specs: &[String],
    streamable_http_specs: &[String],
    websocket_specs: &[String],
) -> Result<McpRuntimeWiring, Box<dyn std::error::Error>> {
    if stdio_specs.is_empty() && streamable_http_specs.is_empty() && websocket_specs.is_empty() {
        return Ok(McpRuntimeWiring {
            dynamic_tool_providers: Vec::new(),
            instructions: Vec::new(),
        });
    }

    let mut servers = Vec::new();

    for (index, raw_spec) in stdio_specs.iter().enumerate() {
        let redacted_spec = redact_mcp_spec(raw_spec);
        let spec = parse_mcp_stdio_spec(raw_spec)
            .map_err(|error| format!("Invalid --mcp-stdio spec '{redacted_spec}': {error}"))?;
        let command =
            required_transport_field("--mcp-stdio", &redacted_spec, "command", spec.command)?;
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

    for (index, raw_spec) in streamable_http_specs.iter().enumerate() {
        let redacted_spec = redact_mcp_spec(raw_spec);
        let spec = parse_mcp_remote_spec(raw_spec).map_err(|error| {
            format!("Invalid --mcp-streamable-http spec '{redacted_spec}': {error}")
        })?;
        let url =
            required_transport_field("--mcp-streamable-http", &redacted_spec, "url", spec.url)?;
        let id =
            optional_non_empty(spec.id).unwrap_or_else(|| format!("streamable-http-{}", index + 1));
        let metadata = MCPServerMetadata {
            id,
            label: spec.label,
            kind: MCPServerKind::StreamableHttp,
        };
        let mut transport = StreamableHttpTransport::new(url);
        if let Some(token) = spec.auth_token {
            transport = transport.auth_token(token);
        }
        if !spec.headers.is_empty() {
            transport = transport.headers(spec.headers);
        }
        let client = MCPClient::new(Box::new(transport));
        servers.push(MCPAggregateServer::with_metadata(metadata, client));
    }

    for (index, raw_spec) in websocket_specs.iter().enumerate() {
        let redacted_spec = redact_mcp_spec(raw_spec);
        let spec = parse_mcp_remote_spec(raw_spec)
            .map_err(|error| format!("Invalid --mcp-websocket spec '{redacted_spec}': {error}"))?;
        let url = required_transport_field("--mcp-websocket", &redacted_spec, "url", spec.url)?;
        let id = optional_non_empty(spec.id).unwrap_or_else(|| format!("websocket-{}", index + 1));
        let metadata = MCPServerMetadata {
            id,
            label: spec.label,
            kind: MCPServerKind::WebSocket,
        };
        let mut transport = WebSocketTransport::new(url);
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
    value.filter(|candidate| !candidate.trim().is_empty())
}

fn redact_mcp_spec(raw: &str) -> String {
    let mut segments = Vec::new();
    let mut redact_next_arg_value = false;
    for segment in raw.split(',') {
        let Some((raw_key, raw_value)) = segment.split_once('=') else {
            segments.push(redacted_malformed_spec_segment());
            continue;
        };
        let key = normalize_mcp_key(raw_key);
        if is_sensitive_mcp_key(&key) {
            segments.push(format!("{raw_key}=<redacted>"));
        } else if key == "header" {
            segments.push(format!("{raw_key}={}", redact_header_value(raw_value)));
        } else if key == "arg" {
            let (redacted_value, redact_next) =
                redact_stdio_arg_value(raw_value, redact_next_arg_value);
            redact_next_arg_value = redact_next;
            segments.push(format!("{raw_key}={redacted_value}"));
        } else if key == "args" {
            let (redacted_value, redact_next) =
                redact_stdio_args_value(raw_value, redact_next_arg_value);
            redact_next_arg_value = redact_next;
            segments.push(format!("{raw_key}={redacted_value}"));
        } else {
            segments.push(format!("{raw_key}={}", redact_mcp_value(&key, raw_value)));
        }
    }
    segments.join(",")
}

fn redact_mcp_value(key: &str, raw_value: &str) -> String {
    if key == "url" {
        redact_url_value(raw_value)
    } else {
        redact_secret_substrings(raw_value)
    }
}

fn redact_stdio_args_value(raw_value: &str, redact_first_value: bool) -> (String, bool) {
    let mut redact_current_value = redact_first_value;
    let mut redact_next_value = false;
    let mut values = Vec::new();
    for value in raw_value.split('|') {
        let (redacted_value, redact_next) = redact_stdio_arg_value(value, redact_current_value);
        values.push(redacted_value);
        redact_current_value = redact_next;
        redact_next_value = redact_next;
    }
    (values.join("|"), redact_next_value)
}

fn redact_stdio_arg_value(raw_value: &str, redact_value: bool) -> (String, bool) {
    if redact_value {
        return ("<redacted>".to_string(), false);
    }

    let redacted_value = redact_secret_substrings(raw_value);
    let redact_next = is_auth_like_stdio_flag(raw_value);
    (redacted_value, redact_next)
}

fn is_auth_like_stdio_flag(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase().replace('_', "-");
    matches!(
        normalized.as_str(),
        "--auth-token"
            | "--token"
            | "--api-key"
            | "--key"
            | "--authorization"
            | "--bearer-token"
            | "-t"
            | "-k"
    )
}

fn redact_url_value(raw_url: &str) -> String {
    let (without_fragment, fragment) = match raw_url.split_once('#') {
        Some((url, fragment)) => (url, Some(fragment)),
        None => (raw_url, None),
    };
    let redacted = match without_fragment.split_once('?') {
        Some((base, query)) => {
            let query = query
                .split('&')
                .map(redact_url_query_param)
                .collect::<Vec<_>>()
                .join("&");
            format!("{}?{}", redact_secret_substrings(base), query)
        }
        None => redact_secret_substrings(without_fragment),
    };
    match fragment {
        Some(fragment) => format!("{redacted}#{}", redact_secret_substrings(fragment)),
        None => redacted,
    }
}

fn redact_url_query_param(param: &str) -> String {
    let Some((name, value)) = param.split_once('=') else {
        let normalized_name = normalize_mcp_key(param);
        return if is_sensitive_url_query_key(&normalized_name) {
            format!("{param}=<redacted>")
        } else {
            redact_secret_substrings(param)
        };
    };
    let normalized_name = normalize_mcp_key(name);
    if is_sensitive_url_query_key(&normalized_name) {
        format!("{name}=<redacted>")
    } else {
        format!("{name}={}", redact_secret_substrings(value))
    }
}

fn is_sensitive_url_query_key(key: &str) -> bool {
    is_sensitive_mcp_key(key)
}

fn redact_header_value(raw_header: &str) -> String {
    let Some((name, value)) = raw_header.split_once(':') else {
        return redacted_malformed_header();
    };
    if is_sensitive_mcp_key(&normalize_mcp_key(name)) {
        format!("{}:<redacted>", name.trim())
    } else {
        format!("{}:{}", name.trim(), redact_secret_substrings(value))
    }
}

fn redact_secret_substrings(value: &str) -> String {
    let mut redacted = value.to_string();
    for pattern in [
        "bearer ",
        "--bearer-token=",
        "--authorization=",
        "--auth-token=",
        "--auth_token=",
        "--access-token=",
        "--api_key=",
        "--access_key=",
        "--access-key=",
        "--client-secret=",
        "--client_secret=",
        "--x-api-key=",
        "--api-key=",
        "--token=",
        "--key=",
        "access_token=",
        "api-key=",
        "api_key=",
        "auth_token=",
        "access-key=",
        "access_key=",
        "client-secret=",
        "client_secret=",
        "x-api-key=",
        "token=",
        "auth=",
    ] {
        redacted = redact_after_pattern(&redacted, pattern, is_short_secret_delimiter);
    }
    for pattern in ["proxy-authorization:", "authorization:"] {
        redacted = redact_after_pattern(&redacted, pattern, is_header_secret_delimiter);
    }
    redacted
}

fn redact_after_pattern(value: &str, pattern: &str, is_delimiter: fn(char) -> bool) -> String {
    let mut output = String::with_capacity(value.len());
    let mut remaining = value;
    loop {
        let Some(index) = find_case_insensitive(remaining, pattern) else {
            output.push_str(remaining);
            break;
        };
        let redaction_start = index + pattern.len();
        output.push_str(&remaining[..redaction_start]);
        output.push_str("<redacted>");
        let tail = &remaining[redaction_start..];
        let redaction_len = tail.find(is_delimiter).unwrap_or(tail.len());
        remaining = &tail[redaction_len..];
    }
    output
}

fn is_short_secret_delimiter(ch: char) -> bool {
    matches!(ch, ',' | '&' | '|' | ';' | ' ' | '\t' | '\n')
}

fn is_header_secret_delimiter(ch: char) -> bool {
    matches!(ch, ',' | '&' | '|' | ';' | '\n')
}

fn find_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    haystack.to_ascii_lowercase().find(needle)
}

fn is_sensitive_mcp_key(key: &str) -> bool {
    let key = normalize_mcp_key(key);
    is_authorization_key(&key)
        || matches!(
            key.as_str(),
            "key" | "api-key" | "access-key" | "client-secret" | "x-api-key"
        )
        || key.contains("auth")
        || key.contains("token")
}

fn is_authorization_key(key: &str) -> bool {
    key == "authorization" || key == "proxy-authorization" || key.ends_with("authorization")
}

fn normalize_mcp_key(raw: &str) -> String {
    raw.trim().to_ascii_lowercase().replace('_', "-")
}

fn redacted_malformed_spec_segment() -> String {
    "<redacted malformed segment>".to_string()
}

fn redacted_malformed_header() -> String {
    "<redacted malformed header>".to_string()
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

pub(crate) fn parse_mcp_remote_spec(raw: &str) -> Result<McpRemoteServerSpec, String> {
    let mut spec = McpRemoteServerSpec::default();
    for (key, value) in parse_structured_spec_entries(raw)? {
        match key.as_str() {
            "id" => spec.id = Some(value),
            "label" => spec.label = Some(value),
            "url" => spec.url = Some(value),
            "auth_token" | "token" => spec.auth_token = Some(value),
            "header" => spec.headers.push(parse_remote_header(&value)?),
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
            return Err(format!(
                "entry '{}' must use key=value format",
                redacted_malformed_spec_segment()
            ));
        };
        let key = raw_key.trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err(format!(
                "entry '{}' has an empty key",
                redacted_malformed_spec_segment()
            ));
        }
        entries.push((key, raw_value.to_string()));
    }
    Ok(entries)
}

fn parse_remote_header(raw: &str) -> Result<(String, String), String> {
    let Some((name, value)) = raw.split_once(':') else {
        return Err(format!(
            "header '{}' must be Name:Value",
            redacted_malformed_header()
        ));
    };
    let name = name.trim();
    if name.is_empty() {
        return Err(format!(
            "header '{}' must include a non-empty name",
            redacted_malformed_header()
        ));
    }
    Ok((name.to_string(), value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::{build_mcp_runtime_wiring, parse_mcp_remote_spec, parse_mcp_stdio_spec};

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
    fn parse_mcp_remote_spec_parses_headers_and_preserves_label() {
        let parsed = parse_mcp_remote_spec(
            "id=docs,label=Docs Gateway,url=https://example.com/mcp,auth_token=abc,header=x-env:dev,header=x-team:agent",
        )
        .expect("remote spec should parse");

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

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_remote_secrets_in_parse_errors() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &["url=https://example.com/mcp,auth_token=secret-token,header=Authorization:Bearer secret,unknown=value".to_string()],
            &[],
        )
        .await
        {
            Ok(_) => panic!("invalid key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("auth_token=<redacted>"));
        assert!(message.contains("Authorization:<redacted>"));
        assert!(!message.contains("secret-token"));
        assert!(!message.contains("Bearer secret"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_remote_secrets_in_missing_field_errors() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &[
                "auth_token=secret-token,token=second-secret,header=Authorization:Bearer secret"
                    .to_string(),
            ],
            &[],
        )
        .await
        {
            Ok(_) => panic!("missing url should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("auth_token=<redacted>"));
        assert!(message.contains("token=<redacted>"));
        assert!(message.contains("Authorization:<redacted>"));
        assert!(!message.contains("secret-token"));
        assert!(!message.contains("second-secret"));
        assert!(!message.contains("Bearer secret"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_malformed_auth_token_segment() {
        let err = match build_mcp_runtime_wiring(&[], &["auth_token secret-token".to_string()], &[])
            .await
        {
            Ok(_) => panic!("malformed auth token segment should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("<redacted malformed segment>"));
        assert!(!message.contains("secret-token"));
        assert!(!message.contains("auth_token secret-token"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_malformed_authorization_header_segment() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &["header Authorization:Bearer secret".to_string()],
            &[],
        )
        .await
        {
            Ok(_) => panic!("malformed authorization header segment should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("<redacted malformed segment>"));
        assert!(!message.contains("Bearer secret"));
        assert!(!message.contains("header Authorization"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_top_level_authorization_in_streamable_http_errors() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &["url=https://example.com/mcp,Authorization=Bearer secret".to_string()],
            &[],
        )
        .await
        {
            Ok(_) => panic!("unknown authorization key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("Authorization=<redacted>"));
        assert!(!message.contains("Bearer secret"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_top_level_proxy_authorization_in_websocket_errors() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &[],
            &["url=ws://example.com/mcp,Proxy-Authorization=Bearer secret".to_string()],
        )
        .await
        {
            Ok(_) => panic!("unknown proxy authorization key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("Proxy-Authorization=<redacted>"));
        assert!(!message.contains("Bearer secret"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_top_level_api_key_in_streamable_http_errors() {
        let err =
            match build_mcp_runtime_wiring(&[], &["api_key=secret-token".to_string()], &[]).await {
                Ok(_) => panic!("unknown key should fail"),
                Err(error) => error,
            };
        let message = err.to_string();

        assert!(message.contains("api_key=<redacted>"));
        assert!(!message.contains("secret-token"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_api_key_header_names_in_streamable_http_errors() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &["url=https://example.com/mcp,header=api-key:secret-token,unknown=value".to_string()],
            &[],
        )
        .await
        {
            Ok(_) => panic!("unknown key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("header=api-key:<redacted>"));
        assert!(!message.contains("secret-token"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_x_api_key_header_names_in_streamable_http_errors() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &[
                "url=https://example.com/mcp,header=x-api-key:secret-token,unknown=value"
                    .to_string(),
            ],
            &[],
        )
        .await
        {
            Ok(_) => panic!("unknown key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("header=x-api-key:<redacted>"));
        assert!(!message.contains("secret-token"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_auth_token_header_names_in_streamable_http_errors() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &[
                "url=https://example.com/mcp,header=X-Auth-Token:secret-token,unknown=value"
                    .to_string(),
            ],
            &[],
        )
        .await
        {
            Ok(_) => panic!("unknown key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("X-Auth-Token:<redacted>"));
        assert!(!message.contains("secret-token"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_token_header_names_in_websocket_errors() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &[],
            &["url=ws://example.com/mcp,header=x-token:secret,unknown=value".to_string()],
        )
        .await
        {
            Ok(_) => panic!("unknown key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("x-token:<redacted>"));
        assert!(!message.contains("secret"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_url_query_token_values_in_errors() {
        let err = match build_mcp_runtime_wiring(
            &[],
            &["url=https://example.com/mcp?token=secret-token&mode=dev,unknown=value".to_string()],
            &[],
        )
        .await
        {
            Ok(_) => panic!("unknown key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("url=https://example.com/mcp?token=<redacted>&mode=dev"));
        assert!(!message.contains("secret-token"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_stdio_auth_like_arg_values_in_errors() {
        let err = match build_mcp_runtime_wiring(
            &["command=npx,arg=--auth_token=secret-token,unknown=value".to_string()],
            &[],
            &[],
        )
        .await
        {
            Ok(_) => panic!("unknown key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("arg=--auth_token=<redacted>"));
        assert!(!message.contains("secret-token"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_split_stdio_auth_arg_values_in_errors() {
        let err = match build_mcp_runtime_wiring(
            &["command=npx,arg=--auth-token,arg=secret-token,unknown=value".to_string()],
            &[],
            &[],
        )
        .await
        {
            Ok(_) => panic!("unknown key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("arg=--auth-token,arg=<redacted>"));
        assert!(!message.contains("secret-token"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_split_stdio_args_auth_values_in_errors() {
        let err = match build_mcp_runtime_wiring(
            &["command=npx,args=--api-key|secret-token|--mode|dev,unknown=value".to_string()],
            &[],
            &[],
        )
        .await
        {
            Ok(_) => panic!("unknown key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("args=--api-key|<redacted>|--mode|dev"));
        assert!(!message.contains("secret-token"));
    }

    #[tokio::test]
    async fn build_mcp_runtime_wiring_redacts_inline_stdio_hyphen_auth_args_in_errors() {
        let err = match build_mcp_runtime_wiring(
            &["command=npx,arg=--api-key=secret-token,unknown=value".to_string()],
            &[],
            &[],
        )
        .await
        {
            Ok(_) => panic!("unknown key should fail"),
            Err(error) => error,
        };
        let message = err.to_string();

        assert!(message.contains("arg=--api-key=<redacted>"));
        assert!(!message.contains("secret-token"));
    }
}
