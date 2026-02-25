# MCP parity + test learnings

read_when:
- You are adding or validating MCP transport coverage (stdio + SSE + multi-server).
- You are validating OpenAI Responses instruction merge behavior for system vs override instructions.

## MCP transport test observations

- `StdioTransport::from_command("cat")` can be used as a deterministic echo loop for transport round-trips in tests.
  - Sending a `tools/list` JSON-RPC payload and receiving it back verifies connect/send/receive lifecycle without adding a fixture server.
- `SSETransport` supports immutable transport configuration plus fluent builders (`header`, `headers`, `auth_token`, timeout/retry setters).
  - Custom headers configured via `.header(...)` are forwarded into RMCP transport config and can be asserted in `wiremock`.
- A multi-server scenario can be tested by creating two independent `SSETransport` instances pointed at different MCP URLs and running sends in `tokio::join!`.
  - This validates that separate endpoints and headers are respected per transport/client pair.

## Instruction merge behavior in OpenAI Responses

- `openai_responses.instructions` (if non-empty) takes precedence in `build_codex_request_body` and is placed into top-level `instructions`.
- Without an explicit override, system messages are concatenated and used as fallback instructions.
- If no usable system message text exists, the request falls back to the default Codex instruction string.
- Tool calls still work with the same merge path because instruction content is applied at provider request construction.

## Wiring guidance (library setup)

```rust,no_run
use roci::mcp::aggregate::{MCPAggregateServer, MCPToolAggregator};
use roci::mcp::client::MCPClient;
use roci::mcp::transport::{SSETransport, StdioTransport};
use roci::tools::{arguments::ToolArguments, tool::ToolExecutionContext};
use serde_json::json;

let alpha = MCPClient::new(Box::new(
    SSETransport::new("http://localhost:8080/mcp").header("x-env", "alpha"),
));
let beta = MCPClient::new(Box::new(
    SSETransport::new("http://localhost:8081/mcp").auth_token("token"),
));
let local = MCPClient::new(Box::new(StdioTransport::from_command("path/to/local/mcp-binary")));

let aggregator = MCPToolAggregator::new(vec![
    MCPAggregateServer::new("alpha", alpha),
    MCPAggregateServer::new("beta", beta),
    MCPAggregateServer::new("local", local),
])
.expect("servers should be unique");

let _tools = aggregator.list_tools_with_origin().await.expect("list tools");
let _search = aggregator
    .execute_routed_tool(
        "alpha__search",
        &ToolArguments::new(json!({ "query": "foo" })),
        &ToolExecutionContext::default(),
    )
    .await
    .expect("execute routed tool");
```

- Set `GenerationSettings::openai_responses.instructions` to force a top-level instruction.
- If unset, system messages are concatenated and used as fallback.
- If there is no system content, provider defaults apply.
