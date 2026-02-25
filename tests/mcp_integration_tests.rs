#![cfg(feature = "mcp")]

use std::{collections::HashSet, time::Duration};

use roci::mcp::aggregate::{MCPAggregateServer, MCPToolAggregator};
use roci::mcp::client::MCPClient;
use roci::mcp::transport::{MCPTransport, SSETransport, StdioTransport};
use roci::models::openai::OpenAiModel;
use roci::provider::{openai_responses::OpenAiResponsesProvider, ModelProvider, ProviderRequest};
use roci::tools::{arguments::ToolArguments, tool::ToolExecutionContext};
use roci::types::{GenerationSettings, ModelMessage, OpenAiResponsesOptions};
use serde_json::json;
use tokio::time::timeout;
use wiremock::Request;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn rpc_message() -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    })
}

fn mcp_tools_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string"
            }
        }
    })
}

fn mock_mcp_sse_handler(
    server_name: &'static str,
    tools: &'static [(&'static str, &'static str)],
) -> impl Fn(&Request) -> ResponseTemplate + Send + Sync {
    move |request: &Request| {
        let body: serde_json::Value = request.body_json().unwrap_or_else(|_| json!({}));
        let method = body.get("method").and_then(|value| value.as_str()).unwrap_or_default();
        let id = body.get("id").cloned().unwrap_or_else(|| json!(1));

        match method {
            "initialize" => ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": {
                        "name": server_name,
                        "version": "0.1.0"
                    }
                }
            })),
            "tools/list" => {
                let tool_definitions: Vec<_> = tools
                    .iter()
                    .map(|(tool_name, description)| {
                        json!({
                            "name": tool_name,
                            "description": description,
                            "inputSchema": mcp_tools_schema()
                        })
                    })
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "tools": tool_definitions,
                        "nextCursor": null
                    }
                }))
            }
            "tools/call" => {
                let called_tool = body
                    .get("params")
                    .and_then(|params| params.get("name"))
                    .and_then(|name| name.as_str())
                    .unwrap_or_default();
                let arguments = body
                    .get("params")
                    .and_then(|params| params.get("arguments"))
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                ResponseTemplate::new(200).set_body_json(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "result": {
                        "content": [{ "type": "text", "text": format!("{server_name}:{called_tool}") }],
                        "structuredContent": {
                            "server": server_name,
                            "tool": called_tool,
                            "arguments": arguments
                        },
                        "isError": false
                    }
                }))
            }
            "notifications/initialized" => ResponseTemplate::new(202),
            _ => ResponseTemplate::new(200).set_body_json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": null
            })),
        }
    }
}

fn request_methods(requests: &[Request]) -> HashSet<String> {
    requests
        .iter()
        .filter_map(|request| {
            request
                .body_json::<serde_json::Value>()
                .ok()
                .and_then(|body| {
                    body.get("method")
                        .and_then(|method| method.as_str())
                        .map(str::to_string)
                })
        })
        .collect()
}

fn request_headers_match(requests: &[Request], header: &str, expected: &str) -> bool {
    requests.iter().all(|request| {
        request
            .headers
            .get(header)
            .and_then(|value| value.to_str().ok())
            == Some(expected)
    })
}

#[tokio::test]
async fn mcp_stdio_transport_roundtrips_via_stdin() {
    let mut transport = StdioTransport::from_command("cat");
    let request = rpc_message();

    transport.send(request.clone()).await.expect("stdio send should work");

    let response = timeout(Duration::from_secs(1), transport.receive())
        .await
        .expect("receive should complete before timeout")
        .expect("stdio response should parse as json");

    assert_eq!(response, request);
    assert!(transport.close().await.is_ok());
}

#[tokio::test]
async fn mcp_sse_transport_sends_with_custom_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(mock_mcp_sse_handler("unit-alpha", &[]))
        .mount(&server)
        .await;

    let mut client = MCPClient::new(Box::new(
        SSETransport::new(format!("{}/mcp", server.uri())).header("x-rpci-scope", "qa"),
    ));

    timeout(Duration::from_secs(2), client.initialize())
        .await
        .expect("initialize should complete before timeout")
        .expect("MCP client should initialize");

    timeout(Duration::from_secs(2), client.list_tools())
        .await
        .expect("tools/list should complete before timeout")
        .expect("MCP client should return tools");

    let requests = timeout(Duration::from_secs(1), server.received_requests())
        .await
        .expect("server should capture requests before timeout")
        .expect("server should have captured requests");
    let methods = request_methods(&requests);
    assert!(methods.contains("initialize"));
    assert!(methods.contains("tools/list"));
    assert!(request_headers_match(&requests, "x-rpci-scope", "qa"));
}

#[tokio::test]
async fn mcp_multi_server_aggregation_has_isolated_clients() {
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("x-server", "a"))
        .respond_with(mock_mcp_sse_handler(
            "alpha",
            &[("search", "Alpha search tool")],
        ))
        .mount(&server_a)
        .await;

    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("x-server", "b"))
        .respond_with(mock_mcp_sse_handler(
            "beta",
            &[("search", "Beta search tool")],
        ))
        .mount(&server_b)
        .await;

    let (mut client_a, mut client_b) = (
        MCPClient::new(Box::new(
            SSETransport::new(format!("{}/mcp", server_a.uri())).header("x-server", "a"),
        )),
        MCPClient::new(Box::new(
            SSETransport::new(format!("{}/mcp", server_b.uri())).header("x-server", "b"),
        )),
    );

    let (result_a, result_b) = tokio::join!(
        async {
            timeout(Duration::from_secs(2), client_a.initialize())
                .await
                .expect("client A initialize should complete before timeout")
                .expect("client A should initialize");
            timeout(Duration::from_secs(2), client_a.list_tools())
                .await
                .expect("client A tools/list should complete before timeout")
                .expect("client A should list tools");
        },
        async {
            timeout(Duration::from_secs(2), client_b.initialize())
                .await
                .expect("client B initialize should complete before timeout")
                .expect("client B should initialize");
            timeout(Duration::from_secs(2), client_b.list_tools())
                .await
                .expect("client B tools/list should complete before timeout")
                .expect("client B should list tools");
        }
    );
    let _ = result_a;
    let _ = result_b;

    let requests_a = timeout(Duration::from_secs(1), server_a.received_requests())
        .await
        .expect("server A should capture requests before timeout")
        .expect("server A should have captured requests");
    let requests_b = timeout(Duration::from_secs(1), server_b.received_requests())
        .await
        .expect("server B should capture requests before timeout")
        .expect("server B should have captured requests");

    let methods_a = request_methods(&requests_a);
    let methods_b = request_methods(&requests_b);
    assert!(methods_a.contains("initialize"));
    assert!(methods_a.contains("tools/list"));
    assert!(methods_b.contains("initialize"));
    assert!(methods_b.contains("tools/list"));
    assert!(request_headers_match(&requests_a, "x-server", "a"));
    assert!(request_headers_match(&requests_b, "x-server", "b"));
}

#[tokio::test]
async fn mcp_client_with_sse_transport_discovers_and_executes_tools() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(mock_mcp_sse_handler(
            "unit-alpha",
            &[("weather", "Mock weather tool"), ("echo", "Mock echo tool")],
        ))
        .mount(&server)
        .await;

    let mut client = MCPClient::new(Box::new(
        SSETransport::new(format!("{}/mcp", server.uri())).header("x-test", "e2e"),
    ));

    client
        .initialize()
        .await
        .expect("MCP client should initialize");
    assert!(client.is_initialized());

    let tools = client
        .list_tools()
        .await
        .expect("SSE MCP client should list tools");
    assert_eq!(tools.len(), 2);
    assert!(tools.iter().any(|tool| tool.name == "weather"));
    assert!(tools.iter().any(|tool| tool.name == "echo"));

    let weather = client
        .call_tool("weather", json!({ "query": "today" }))
        .await
        .expect("SSE MCP client should execute tool");
    assert_eq!(
        weather.structured_content,
        Some(json!({
            "server": "unit-alpha",
            "tool": "weather",
            "arguments": { "query": "today" }
        }))
    );
    assert_eq!(weather.text_content, Some("unit-alpha:weather".into()));
}

#[tokio::test]
async fn mcp_tool_aggregator_routes_multiple_sse_servers() {
    let server_a = MockServer::start().await;
    let server_b = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(mock_mcp_sse_handler(
            "alpha",
            &[("search", "Alpha search tool"), ("ping", "Alpha ping tool")],
        ))
        .mount(&server_a)
        .await;

    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(mock_mcp_sse_handler(
            "beta",
            &[("search", "Beta search tool"), ("status", "Beta status tool")],
        ))
        .mount(&server_b)
        .await;

    let aggregator = MCPToolAggregator::new(vec![
        MCPAggregateServer::new(
            "alpha",
            MCPClient::new(Box::new(
                SSETransport::new(format!("{}/mcp", server_a.uri()))
                    .header("x-mcp-server", "alpha"),
            )),
        ),
        MCPAggregateServer::new(
            "beta",
            MCPClient::new(Box::new(
                SSETransport::new(format!("{}/mcp", server_b.uri()))
                    .header("x-mcp-server", "beta"),
            )),
        ),
    ])
    .expect("aggregator should accept multiple servers");

    let tools = aggregator
        .list_tools_with_origin()
        .await
        .expect("aggregator should merge tool listings");
    assert_eq!(tools.len(), 4);
    assert!(tools.iter().any(|tool| tool.exposed_name == "alpha__search"));
    assert!(tools.iter().any(|tool| tool.exposed_name == "beta__search"));

    let alpha_result = aggregator
        .execute_routed_tool(
            "alpha__search",
            &ToolArguments::new(json!({ "query": "news" })),
            &ToolExecutionContext::default(),
        )
        .await
        .expect("alpha route should execute");
    assert_eq!(alpha_result["server"], "alpha");
    assert_eq!(alpha_result["tool"], "search");

    let beta_result = aggregator
        .execute_routed_tool(
            "beta__search",
            &ToolArguments::new(json!({ "query": "status" })),
            &ToolExecutionContext::default(),
        )
        .await
        .expect("beta route should execute");
    assert_eq!(beta_result["server"], "beta");
    assert_eq!(beta_result["tool"], "search");
}

#[tokio::test]
async fn openai_responses_instructions_override_merges_into_request_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .and(body_string_contains("\"instructions\":\"OVERRIDE INSTRUCTIONS\""))
        .and(body_string_contains("\"role\":\"user\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "ok"
                }]
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let request = ProviderRequest {
        messages: vec![
            ModelMessage::system("System instruction should be ignored when override exists."),
            ModelMessage::user("What is MCP?"),
        ],
        settings: GenerationSettings {
            openai_responses: Some(OpenAiResponsesOptions {
                instructions: Some("OVERRIDE INSTRUCTIONS".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        },
        tools: None,
        response_format: None,
        session_id: None,
        transport: None,
    };

    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), Some(server.uri()), None);

    let response = provider
        .generate_text(&request)
        .await
        .expect("generate_text should return merged instructions result");
    assert_eq!(response.text, "ok");
}

#[tokio::test]
async fn openai_responses_instructions_falls_back_to_system_messages() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": [{
                "type": "message",
                "content": [{
                    "type": "output_text",
                    "text": "ok"
                }]
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let request = ProviderRequest {
        messages: vec![
            ModelMessage::system("Use this system message"),
            ModelMessage::user("What is MCP?"),
        ],
        settings: GenerationSettings {
            openai_responses: Some(OpenAiResponsesOptions::default()),
            ..Default::default()
        },
        tools: None,
        response_format: None,
        session_id: None,
        transport: None,
    };

    let provider =
        OpenAiResponsesProvider::new(OpenAiModel::Gpt5Nano, "test-key".to_string(), Some(server.uri()), None);

    let response = provider
        .generate_text(&request)
        .await
        .expect("generate_text should use merged system messages");
    let received_requests = server
        .received_requests()
        .await
        .expect("server should have captured requests");
    assert_eq!(received_requests.len(), 1);
    let request_body = received_requests[0]
        .body_json::<serde_json::Value>()
        .expect("request body should be valid JSON");
    let input = request_body
        .get("input")
        .and_then(|value| value.as_array())
        .expect("request body should include input items");
    let has_system = input.iter().any(|item| {
        item.get("role").and_then(|role| role.as_str()) == Some("developer")
            && item.get("content").and_then(|content| content.as_str())
                == Some("Use this system message")
    });
    assert!(has_system);
    assert_eq!(response.text, "ok");
}
