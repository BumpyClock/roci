# Roci Foundation Lanes Implementation Plan

> **For agentic workers:** Prefer `subagent-driven-development` for execution when available. Task implementers own task work and review fixes; integration owner owns final integration. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build four ready P0 lanes in parallel: MCP identity/namespacing, MCP client/server transport foundation, security primitives, and model candidate retry/health.

**Architecture:** Keep Roci core provider-agnostic and host-friendly: identity/routing stays structured, MCP protocol IO stays below transport/server boundaries, security primitives are reusable SDK contracts, and model fallback is explicit candidate orchestration instead of hidden provider magic. `roci-cli` remains example app and live verification harness.

**Tech Stack:** Rust, Tokio, async-trait, serde/serde_json, rmcp, tokio-tungstenite, reqwest, regex, sha2, cargo test/clippy/fmt, tmux live CLI verification.

---

## Scope

Tasks covered:
- `tsq-p4cpczyg.6` MCP identity/namespacing
- `tsq-p4cpczyg.1` MCP transport lifecycle split
- `tsq-p4cpczyg.2.1` MCP server core foundation
- `tsq-1av9jz0z.2.1` command classifier
- `tsq-1av9jz0z.3.1` sensitive data redactor
- `tsq-1av9jz0z.4.1` filesystem permission policy
- `tsq-g6ba4ega.1` model candidates
- `tsq-g6ba4ega.2` retry and candidate advancement
- `tsq-g6ba4ega.3` model health observations

Out of scope:
- Full interactive `/model` command UX beyond CLI switches needed for live tests.
- Periodic model heartbeat/ping. Health only observes real request outcomes.
- Persistent retry moving between candidates. Persistent retry retries current candidate only.

## Current Code Map

- Modify `crates/roci-core/src/mcp/aggregate.rs`: MCP exposed names, collision policy, routing tests.
- Modify `crates/roci-core/Cargo.toml`: add `sha2 = "0.10"` for deterministic collision suffixes.
- Modify `crates/roci-core/src/mcp/transport.rs`: export canonical Streamable HTTP and WebSocket transport types.
- Modify `crates/roci-core/src/mcp/transport/sse.rs`: replace public SSE naming with Streamable HTTP naming.
- Modify `crates/roci-core/src/mcp/instructions.rs`: rename `MCPServerKind::Sse` to `StreamableHttp`.
- Create `crates/roci-core/src/mcp/transport/websocket.rs`: WebSocket transport adapter behind `mcp` feature.
- Create `crates/roci-core/src/mcp/server.rs`: transport-agnostic MCP server core wrapping Roci tools.
- Modify `crates/roci-core/src/mcp/mod.rs`: export new MCP server and transport APIs.
- Create `crates/roci-core/src/security/mod.rs`: security module exports.
- Create `crates/roci-core/src/security/command.rs`: command normalization/classification.
- Create `crates/roci-core/src/security/redaction.rs`: JSON/text redaction APIs.
- Create `crates/roci-core/src/security/filesystem.rs`: path resolution and permission checks.
- Modify `crates/roci-core/src/lib.rs`: export `security`.
- Modify `crates/roci-core/src/tools/tool.rs`: connect `SandboxProvider` helper impl to command classifier where useful.
- Modify `crates/roci-core/src/models/mod.rs`: export model candidate APIs.
- Create `crates/roci-core/src/models/candidates.rs`: ordered candidate collection and parse helpers.
- Create `crates/roci-core/src/models/health.rs`: session-local health tracker and observations.
- Modify `crates/roci-core/src/models/selector.rs`: parse candidate lists without changing single-model parse.
- Modify `crates/roci-core/src/agent_loop/runner.rs`: `RunRequest` stores candidates, keeps single-model constructor compatibility.
- Modify `crates/roci-core/src/agent/runtime/config.rs`: `AgentConfig` replaces runtime `model` with canonical `candidates`.
- Modify `crates/roci-core/src/agent/runtime.rs`: runtime current-model APIs become candidate-aware.
- Modify `crates/roci-core/src/agent/runtime/run_loop.rs`: pass candidate list into `RunRequest`.
- Modify `crates/roci-core/src/agent/runtime/mutations.rs`: update runtime model mutation helpers to rebuild `candidates`.
- Modify `crates/roci-core/src/agent/runtime/lifecycle.rs`: ensure lifecycle payloads use active candidate identity.
- Modify `crates/roci-core/src/agent/core.rs`: migrate agent constructors to `candidates = [model]`.
- Modify `crates/roci-core/src/agent/subagents/launcher.rs`: pass resolved profile models to child `AgentConfig.candidates`.
- Modify `crates/roci-core/src/agent/subagents/profiles.rs`: keep launch-time resolution boundary; do not rebuild primary/fallback runtime pairs.
- Modify `crates/roci-core/src/agent_loop/runner/engine/mod.rs`: create providers per candidate and emit retry events.
- Modify `crates/roci-core/src/agent_loop/runner/engine/llm_phase.rs`: retry current candidate only; candidate advance happens in runner orchestration.
- Modify `crates/roci-core/src/agent_loop/events.rs`: add `RetryEvent` payload.
- Modify `crates/roci-cli/src/cli/mod.rs`: add testable candidate/retry switches, rename `--mcp-sse` to `--mcp-streamable-http`, and add `--mcp-websocket`.
- Modify `crates/roci-cli/src/chat.rs`: wire candidate list, retry mode, health debug output, and streamable HTTP MCP args.
- Modify `crates/roci-cli/src/chat/mcp.rs`: rename SSE parser/spec/kind to streamable HTTP.
- Modify `crates/roci-cli/src/chat/runtime_events.rs`: render retry events.
- Modify `docs/testing.md`: add live verification commands for model fallback and MCP/security smoke.
- Modify `.env.example`: add optional framed OpenAI-compatible endpoint hint if current convention keeps endpoint hints there.

## Worker Split

- Worker A owns MCP identity and aggregation files only.
- Worker B owns MCP transport/server files only.
- Worker C owns security module files only.
- Worker D owns model candidate/retry/health core files only.
- Worker E owns `roci-cli`, docs, and live verification after A-D land.
- Integration owner resolves compile breaks across shared exports and runs final gates.

Workers are not alone in repo. Check `git status` before edits. Do not revert other workers.

---

## Task 1: MCP Identity And Collision Policy

**Files:**
- Modify `crates/roci-core/Cargo.toml`
- Modify `crates/roci-core/src/mcp/aggregate.rs`
- Test in `crates/roci-core/src/mcp/aggregate.rs`

- [ ] **Step 1: Write failing aggregation tests**

Add/replace tests:

```rust
#[tokio::test]
async fn list_tools_exposes_mcp_prefixed_names() {
    let (client, _calls, _list_calls) = MockClientOps::new(
        vec![Ok(vec![test_tool("search")])],
        HashMap::from([(String::from("search"), json!({"ok": true}))]),
    );
    let aggregator = MCPToolAggregator::new(vec![MCPAggregateServer::from_client_ops(
        "filesystem",
        Box::new(client),
    )])
    .expect("aggregator should construct");

    let tools = aggregator.list_tools_with_origin().await.expect("list tools");

    assert_eq!(tools[0].exposed_name, "mcp__filesystem__search");
    assert_eq!(tools[0].server_id, "filesystem");
    assert_eq!(tools[0].upstream_tool_name, "search");
}

#[tokio::test]
async fn default_collision_policy_denies_exposed_name_collision() {
    let (first, _first_calls, _first_list_calls) =
        MockClientOps::new(vec![Ok(vec![test_tool("beta__search")])], HashMap::new());
    let (second, _second_calls, _second_list_calls) =
        MockClientOps::new(vec![Ok(vec![test_tool("search")])], HashMap::new());
    let aggregator = MCPToolAggregator::new(vec![
        MCPAggregateServer::from_client_ops("alpha", Box::new(first)),
        MCPAggregateServer::from_client_ops("alpha__beta", Box::new(second)),
    ])
    .expect("aggregator should construct");

    let err = aggregator
        .list_tools_with_origin()
        .await
        .expect_err("collision should fail");

    assert!(err.to_string().contains("Duplicate aggregated MCP tool name"));
}

#[tokio::test]
async fn suffix_collision_policy_appends_stable_hash_suffix() {
    let (first, _first_calls, _first_list_calls) =
        MockClientOps::new(vec![Ok(vec![test_tool("beta__search")])], HashMap::new());
    let (second, _second_calls, _second_list_calls) =
        MockClientOps::new(vec![Ok(vec![test_tool("search")])], HashMap::new());
    let aggregator = MCPToolAggregator::with_config(
        vec![
            MCPAggregateServer::from_client_ops("alpha", Box::new(first)),
            MCPAggregateServer::from_client_ops("alpha__beta", Box::new(second)),
        ],
        MCPAggregationConfig {
            collision_policy: MCPCollisionPolicy::SuffixOnCollision { hash_len: 12 },
            init_policy: MCPAggregateInitPolicy::StrictFailFast,
        },
    )
    .expect("aggregator should construct");

    let tools = aggregator.list_tools_with_origin().await.expect("list tools");
    let names = tools.into_iter().map(|tool| tool.exposed_name).collect::<Vec<_>>();

    assert_eq!(names.len(), 2);
    assert!(names.iter().any(|name| name == "mcp__alpha__beta__search"));
    assert!(names.iter().any(|name| name.starts_with("mcp__alpha__beta__search__h")));
}
```

Run: `cargo test -p roci-core mcp::aggregate --features mcp`

- [ ] **Step 2: Add core hash dependency**

In `crates/roci-core/Cargo.toml` dependencies:

```toml
sha2 = "0.10"
```

- [ ] **Step 3: Implement identity serializer and collision policy**

Add imports and types:

```rust
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MCPToolIdentity {
    pub server_id: String,
    pub tool_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MCPResourceIdentity {
    pub server_id: String,
    pub uri: String,
}

impl MCPToolIdentity {
    pub fn new(server_id: impl Into<String>, tool_name: impl Into<String>) -> Self {
        Self {
            server_id: server_id.into(),
            tool_name: tool_name.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MCPCollisionPolicy {
    #[default]
    DenyOnCollision,
    SuffixOnCollision { hash_len: usize },
}

pub fn serialize_mcp_tool_name(server_id: &str, tool_name: &str) -> String {
    format!("mcp__{server_id}__{tool_name}")
}
```

Replace `MCPToolRoute` fields with structured identity:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MCPToolRoute {
    pub identity: MCPToolIdentity,
    pub server_label: Option<String>,
}
```

Use helper:

```rust
fn exposed_name(
    policy: MCPCollisionPolicy,
    used: &HashMap<String, MCPToolRoute>,
    identity: &MCPToolIdentity,
) -> Result<String, RociError> {
    let base = serialize_mcp_tool_name(&identity.server_id, &identity.tool_name);
    if !used.contains_key(&base) {
        return Ok(base);
    }
    match policy {
        MCPCollisionPolicy::DenyOnCollision => Err(RociError::InvalidState(format!(
            "Duplicate aggregated MCP tool name '{base}'"
        ))),
        MCPCollisionPolicy::SuffixOnCollision { hash_len } => {
            let mut hasher = Sha256::new();
            hasher.update(identity.server_id.as_bytes());
            hasher.update([0]);
            hasher.update(identity.tool_name.as_bytes());
            let hex = format!("{:x}", hasher.finalize());
            let len = hash_len.clamp(1, hex.len());
            let mut suffixed = format!("{base}__h{}", &hex[..len]);
            while used.contains_key(&suffixed) {
                suffixed.push('0');
            }
            Ok(suffixed)
        }
    }
}
```

- [ ] **Step 4: Verify routing still uses upstream name**

Run:

```bash
cargo test -p roci-core mcp::aggregate::tests::execute_tool_routes_to_correct_server_and_upstream_name --features mcp
cargo test -p roci-core mcp::aggregate --features mcp
```

Expected: all MCP aggregate tests pass. Existing route tests updated to call `mcp__alpha__search`.

- [ ] **Step 5: Preserve structured MCP resource provenance**

Add resource identity/provenance in the MCP aggregation layer:

```rust
#[derive(Debug, Clone)]
pub struct MCPAggregatedResource {
    pub identity: MCPResourceIdentity,
    pub server_label: Option<String>,
    pub uri: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub mime_type: Option<String>,
}
```

Contract:
- resource routing key is structured `{ server_id, uri }`
- never encode server id into URI for routing
- display labels may include server label/id, but route lookup uses `MCPResourceIdentity`

Tests:
- two servers can expose same `uri` without route collision because `server_id` differs
- read/resource call routes by `{ server_id, uri }`, not parsed display text
- server label changes do not change resource identity

---

## Task 2: MCP Transport Surface And Server Core

**Files:**
- Modify `crates/roci-core/src/mcp/transport.rs`
- Move/modify `crates/roci-core/src/mcp/transport/sse.rs` to `crates/roci-core/src/mcp/transport/streamable_http.rs`
- Create `crates/roci-core/src/mcp/transport/websocket.rs`
- Create `crates/roci-core/src/mcp/server.rs`
- Modify `crates/roci-core/src/mcp/mod.rs`
- Modify `crates/roci-core/src/mcp/instructions.rs`
- Modify `crates/roci-cli/src/cli/mod.rs`
- Modify `crates/roci-cli/src/chat.rs`
- Modify `crates/roci-cli/src/chat/mcp.rs`
- Test in new/modified module tests

- [ ] **Step 1: Replace public SSE naming with Streamable HTTP**

Rename the public type, tests, and CLI imports. Do not export `SSETransport` from `transport.rs`.

```rust
#[derive(Debug, Clone)]
pub struct StreamableHttpTransportConfig {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub auth_token: Option<String>,
    pub request_timeout_ms: Option<u64>,
    pub connect_timeout_ms: Option<u64>,
    pub retry_max_attempts: Option<usize>,
    pub retry_base_delay_ms: u64,
}

pub struct StreamableHttpTransport {
    url: String,
    auth_token: Option<String>,
    custom_headers: HashMap<String, String>,
    request_timeout_ms: Option<u64>,
    connect_timeout_ms: Option<u64>,
    retry_max_attempts: Option<usize>,
    retry_base_delay_ms: u64,
    inner: Option<Box<dyn DynRoleClientTransport>>,
    closed: bool,
}
```

Keep any old `sse` module private only long enough to move code. Public API after task: `StreamableHttpTransport` and `WebSocketTransport`.

Also rename metadata/CLI surface:

```rust
pub enum MCPServerKind {
    Stdio,
    StreamableHttp,
    Unknown,
}
```

CLI changes:
- `mcp_sse` field -> `mcp_streamable_http`
- `--mcp-sse` -> `--mcp-streamable-http`
- add `mcp_websocket` field and `--mcp-websocket`
- `parse_mcp_sse_spec` -> `parse_mcp_streamable_http_spec`
- `McpSseServerSpec` -> `McpStreamableHttpServerSpec`
- add `parse_mcp_websocket_spec` and `McpWebSocketServerSpec`
- `MCPServerKind::Sse` -> `MCPServerKind::StreamableHttp`
- update parse tests to assert old `--mcp-sse` is absent from public examples and new HTTP/WS args parse repeated specs

- [ ] **Step 2: Implement WebSocket transport against current trait**

`MCPTransport` currently requires:

```rust
#[async_trait]
pub trait MCPTransport: Send {
    async fn connect(
        &mut self,
        client_handler: MCPClientHandler,
    ) -> Result<MCPRunningService, ClientInitializeError>;
    async fn send(&mut self, message: serde_json::Value) -> Result<(), RociError>;
    async fn receive(&mut self) -> Result<serde_json::Value, RociError>;
    async fn close(&mut self) -> Result<(), RociError>;
}
```

Create `websocket.rs` with the same constructor/config shape:

```rust
#[derive(Debug, Clone)]
pub struct WebSocketTransportConfig {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub auth_token: Option<String>,
    pub connect_timeout_ms: Option<u64>,
    pub request_timeout_ms: Option<u64>,
}

pub struct WebSocketTransport {
    config: WebSocketTransportConfig,
    inner: Option<Box<dyn DynRoleClientTransport>>,
    closed: bool,
}
```

Implementation contract:
- `connect(&mut self, client_handler)` initializes an MCP session through `client_handler.into_dyn().serve(transport).await`.
- `send` serializes `ClientJsonRpcMessage` and writes one WebSocket text message.
- `receive` reads one WebSocket text message and deserializes `ServerJsonRpcMessage`.
- `close` sends close frame once and is idempotent.
- timeout -> `RociError::Timeout(ms)`.
- invalid URL/header/auth -> `RociError::Configuration`.
- closed peer/malformed message -> deterministic `RociError::Stream("MCP transport closed by peer")` or `RociError::Provider { provider: "mcp".into(), message }`.
- no `UnsupportedOperation` placeholder is allowed if `tsq-p4cpczyg.1` is closed.

If `rmcp` exposes a WebSocket client transport, wrap it with `ErasedRoleClientTransport`. If not, implement a small `RmcpTransport<RoleClient>` adapter over `tokio_tungstenite` and then erase it through the existing `common.rs` path.

- [ ] **Step 3: Export only canonical transport APIs**

In `transport.rs`:

```rust
mod common;
mod stdio;
mod streamable_http;
mod websocket;

pub use stdio::StdioTransport;
pub use streamable_http::{StreamableHttpTransport, StreamableHttpTransportConfig};
pub use websocket::{WebSocketTransport, WebSocketTransportConfig};
```

- [ ] **Step 4: Add transport integration tests**

Add tests covering:
- Streamable HTTP initialize/list/call happy path.
- Streamable HTTP JSON and `text/event-stream` response handling.
- Streamable HTTP session id and close/delete semantics where rmcp exposes them.
- Streamable HTTP timeout and unsupported content-type mapping.
- WebSocket initialize/list/call happy path.
- WebSocket timeout, malformed-peer, closed-peer, auth/header cases.
- Stdio regression test still passes.

Run: `cargo test -p roci-core mcp::transport --features mcp`

- [ ] **Step 5: Add transport-agnostic MCP server core**

Create `crates/roci-core/src/mcp/server.rs` with structured identity and dynamic providers:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum McpServerToolIdentity {
    Native { name: String },
    Mcp { server_id: String, tool_name: String },
}

pub struct McpServerCore {
    tools: Vec<Arc<dyn Tool>>,
    dynamic_tool_providers: Vec<Arc<dyn DynamicToolProvider>>,
}

impl McpServerCore {
    pub fn new(tools: Vec<Arc<dyn Tool>>) -> Self;
    pub fn with_dynamic_tool_providers(
        self,
        providers: Vec<Arc<dyn DynamicToolProvider>>,
    ) -> Self;
    pub async fn list_tools(&self) -> Result<Vec<MCPToolSchema>, RociError>;
    pub async fn call_tool(
        &self,
        identity: McpServerToolIdentity,
        arguments: serde_json::Value,
    ) -> Result<MCPToolCallResult, RociError>;
}
```

Server core must not parse `mcp__<server_id>__<tool_name>` to route. Native tools route by `McpServerToolIdentity::Native { name }`; future aggregated MCP tools route by `McpServerToolIdentity::Mcp { server_id, tool_name }`.

- [ ] **Step 6: Add server core mapping tests**

Tests:
- Roci `Tool` metadata -> MCP schema.
- `DynamicToolProvider` metadata -> MCP schema.
- `tools/list` deterministic ordering and schema stability.
- successful native `tools/call`.
- unknown tool maps to MCP error-result.
- validation/runtime/cancel errors map to deterministic MCP call results.
- structured identity test proves no exposed-name reparsing.

Run: `cargo test -p roci-core mcp::server --features mcp`

---

## Task 3: Security Command Classifier

**Files:**
- Create `crates/roci-core/src/security/mod.rs`
- Create `crates/roci-core/src/security/command.rs`
- Modify `crates/roci-core/src/lib.rs`

- [ ] **Step 1: Add failing classifier tests**

Create tests in `command.rs`:

```rust
#[test]
fn classifies_wrapper_and_env_command() {
    let report = classify_shell_command("FOO=bar env sudo rm -rf /tmp/demo");
    assert_eq!(report.primary_executable.as_deref(), Some("rm"));
    assert!(report.categories.contains(&CommandCategory::DestructiveDelete));
    assert!(report.reasons.iter().any(|reason| reason == "wrapper detected: sudo"));
}

#[test]
fn preserves_unknown_for_unparsed_shell_features() {
    let report = classify_shell_command("echo ok | sh");
    assert!(report.categories.contains(&CommandCategory::Unknown));
    assert!(report.categories.contains(&CommandCategory::CodeExecution));
}

#[test]
fn unions_categories_across_connectors() {
    let report = classify_shell_command("cat Cargo.toml && curl https://example.com");
    assert!(report.categories.contains(&CommandCategory::ReadOnly));
    assert!(report.categories.contains(&CommandCategory::NetworkLikely));
}
```

- [ ] **Step 2: Implement conservative normalization**

Create command API:

```rust
pub trait CommandClassifier: Send + Sync {
    fn classify(&self, input: CommandInput) -> CommandInsight;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInput {
    pub raw_command: String,
    pub cwd: Option<std::path::PathBuf>,
    pub tool_name: Option<String>,
    pub shell_kind: Option<ShellKind>,
    pub platform: Option<CommandPlatform>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellKind {
    Sh,
    Bash,
    Zsh,
    Fish,
    PowerShell,
    Cmd,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandPlatform {
    Unix,
    Windows,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CommandCategory {
    ReadOnly,
    WritesFilesystem,
    DestructiveDelete,
    PrivilegeEscalation,
    PermissionChange,
    ProcessControl,
    NetworkLikely,
    CodeExecution,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandConfidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandInsight {
    pub normalized_command: String,
    pub primary_executable: Option<String>,
    pub categories: Vec<CommandCategory>,
    pub reasons: Vec<String>,
    pub confidence: CommandConfidence,
}

pub struct HeuristicCommandClassifier;

pub fn classify_shell_command(raw_command: &str) -> CommandInsight {
    HeuristicCommandClassifier.classify(CommandInput {
        raw_command: raw_command.to_string(),
        cwd: None,
        tool_name: None,
        shell_kind: None,
        platform: None,
    })
}

impl CommandClassifier for HeuristicCommandClassifier {
    fn classify(&self, input: CommandInput) -> CommandInsight {
        // Split only enough for v1 safety: env assignments, known wrappers,
        // connectors (; && || |) mark Unknown while preserving detected commands.
    }
}
```

Implement exact tables:

```rust
const WRAPPERS: &[&str] = &["sudo", "doas", "command", "builtin", "time", "env", "xargs"];
const DESTRUCTIVE: &[&str] = &["rm", "rmdir", "unlink", "shred", "dd", "mkfs"];
const WRITE_FS: &[&str] = &["mv", "cp", "touch", "mkdir", "tee"];
const PERMISSION: &[&str] = &["chmod", "chown", "chgrp", "setfacl"];
const PROCESS: &[&str] = &["kill", "pkill", "killall", "launchctl", "systemctl"];
const NETWORK: &[&str] = &["curl", "wget", "ssh", "scp", "rsync", "nc"];
const CODE_EXEC: &[&str] = &["sh", "bash", "zsh", "fish", "python", "ruby", "node", "perl"];
```

Git rule: `git status/log/diff/show/branch` -> `ReadOnly`; `git commit/push/rebase/reset/checkout/clean/apply` -> `WritesFilesystem`.

`reasons` must include stable strings such as `"matched destructive delete executable: rm"` and `"connector detected: |"`. Unknown commands and connector-heavy shell input include `Unknown` and `confidence = Low`.

- [ ] **Step 3: Export security module**

In `security/mod.rs`:

```rust
pub mod command;
pub mod filesystem;
pub mod redaction;
```

In `lib.rs`:

```rust
pub mod security;
```

- [ ] **Step 4: Verify classifier**

Run:

```bash
cargo test -p roci-core security::command
```

---

## Task 4: Security Redactor

**Files:**
- Create `crates/roci-core/src/security/redaction.rs`

- [ ] **Step 1: Add failing redaction tests**

Tests:

```rust
#[test]
fn redacts_json_string_values_with_json_pointer_offsets() {
    let value = serde_json::json!({"api": {"key": "sk-abc123"}, "keep": "visible"});
    let report = SecretRedactor::new_default().redact_json(&value);

    assert_eq!(report.redacted["api"]["key"], "[REDACTED_API_KEY]");
    assert_eq!(report.redacted["keep"], "visible");
    assert_eq!(
        report.matches[0].location,
        SecretLocation::JsonPointer("/api/key".to_string())
    );
}

#[test]
fn redacts_text_with_utf8_byte_offsets() {
    let report = SecretRedactor::new_default().redact_text("token=sk-abc123 café");

    assert_eq!(report.redacted, "token=[REDACTED_API_KEY] café");
    assert_eq!(
        report.matches[0].location,
        SecretLocation::TextRange { start: 6, end: 15 }
    );
}

#[test]
fn preserves_json_keys() {
    let value = serde_json::json!({"sk-abc123": "value"});
    let report = SecretRedactor::new_default().redact_json(&value);

    assert_eq!(report.redacted["sk-abc123"], "value");
}
```

- [ ] **Step 2: Implement redaction API**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretKind {
    PrivateKey,
    AuthHeader,
    BearerToken,
    ApiKey,
    EnvSecret,
    GenericSecret,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretLocation {
    TextRange { start: usize, end: usize },
    JsonPointer(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    pub kind: SecretKind,
    pub location: SecretLocation,
    pub replacement: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRedaction<T> {
    pub redacted: T,
    pub matches: Vec<SecretMatch>,
}

pub struct SecretRedactor {
    patterns: Vec<(SecretKind, regex::Regex)>,
}

impl SecretRedactor {
    pub fn new_default() -> Self;
    pub fn scan_text(&self, input: &str) -> Vec<SecretMatch>;
    pub fn redact_text(&self, input: &str) -> SecretRedaction<String>;
    pub fn redact_json(&self, input: &serde_json::Value) -> SecretRedaction<serde_json::Value>;
}
```

Default replacement tokens:
- `PrivateKey` -> `[REDACTED_PRIVATE_KEY]`
- `AuthHeader` -> `[REDACTED_AUTH_HEADER]`
- `BearerToken` -> `[REDACTED_TOKEN]`
- `ApiKey` -> `[REDACTED_API_KEY]`
- `EnvSecret` -> `[REDACTED_SECRET]`
- `GenericSecret` -> `[REDACTED_SECRET]`

Default patterns cover common API keys, bearer tokens, auth headers, private key blocks, and env-style secret assignments.
Overlap rule: sort by start ascending, end descending, then kind priority `PrivateKey`, `AuthHeader`, `BearerToken`, `ApiKey`, `EnvSecret`, `GenericSecret`; keep first non-overlapping match.
Text implementation rule: build output from retained byte ranges plus match-specific replacement token. JSON implementation rule: recurse through object values and array values, redact string values only, preserve keys unchanged, and fill `SecretLocation::JsonPointer` using RFC 6901 escaping (`~` -> `~0`, `/` -> `~1`).

- [ ] **Step 3: Verify redactor**

Run:

```bash
cargo test -p roci-core security::redaction
```

---

## Task 5: Filesystem Permission Policy

**Files:**
- Create `crates/roci-core/src/security/filesystem.rs`

- [ ] **Step 1: Add failing filesystem tests**

Tests:

```rust
#[test]
fn lexical_mode_blocks_parent_escape() {
    let policy = FilesystemPolicy {
        readable_roots: vec![PathBoundary::root(PathBuf::from("/workspace"))],
        writable_roots: Vec::new(),
        denied: Vec::new(),
        resolution_mode: PathResolutionMode::Lexical,
        symlink_policy: SymlinkPolicy::DenySymlinks,
    };

    assert!(policy.evaluate(PathAccessRequest {
        operation: PathOperation::Read,
        path: PathBuf::from("/workspace/src/lib.rs"),
        cwd: None,
    }).allowed);
    assert!(!policy.evaluate(PathAccessRequest {
        operation: PathOperation::Read,
        path: PathBuf::from("/workspace/../etc/passwd"),
        cwd: None,
    }).allowed);
}

#[test]
fn best_effort_allows_missing_child_inside_existing_root() {
    let temp = tempfile::tempdir().expect("tempdir");
    let policy = FilesystemPolicy {
        readable_roots: vec![PathBoundary::root(temp.path().to_path_buf())],
        writable_roots: Vec::new(),
        denied: Vec::new(),
        resolution_mode: PathResolutionMode::CanonicalizeBestEffort,
        symlink_policy: SymlinkPolicy::DenySymlinks,
    };

    assert!(policy.evaluate(PathAccessRequest {
        operation: PathOperation::Read,
        path: temp.path().join("missing/file.txt"),
        cwd: None,
    }).allowed);
}
```

- [ ] **Step 2: Implement policy types**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathResolutionMode {
    Lexical,
    CanonicalizeExisting,
    CanonicalizeBestEffort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymlinkPolicy {
    DenySymlinks,
    FollowIfTargetAllowed,
    AllowLexical,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathBoundary {
    pub root: std::path::PathBuf,
    pub glob: Option<String>,
}

impl PathBoundary {
    pub fn root(root: std::path::PathBuf) -> Self {
        Self { root, glob: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathOperation {
    Read,
    Write,
    Create,
    Delete,
    List,
    Search,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathAccessRequest {
    pub operation: PathOperation,
    pub path: std::path::PathBuf,
    pub cwd: Option<std::path::PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesystemDecision {
    pub allowed: bool,
    pub normalized_path: Option<std::path::PathBuf>,
    pub reason: String,
    pub matched_boundary: Option<PathBoundary>,
}

pub struct FilesystemPolicy {
    pub readable_roots: Vec<PathBoundary>,
    pub writable_roots: Vec<PathBoundary>,
    pub denied: Vec<PathBoundary>,
    pub resolution_mode: PathResolutionMode,
    pub symlink_policy: SymlinkPolicy,
}

impl FilesystemPolicy {
    pub fn permissive() -> Self;
    pub fn evaluate(&self, request: PathAccessRequest) -> FilesystemDecision;
}
```

Precedence: invalid/unsupported normalization denies when restrictions exist; denied paths/globs; operation-specific allow roots; permissive default when no restrictions are configured. Denied rules always win. For best-effort, canonicalize deepest existing parent, then append lexical missing suffix.

- [ ] **Step 3: Verify filesystem policy**

Run:

```bash
cargo test -p roci-core security::filesystem
```

---

## Task 6: Model Candidate API

**Files:**
- Create `crates/roci-core/src/models/candidates.rs`
- Modify `crates/roci-core/src/models/mod.rs`
- Modify `crates/roci-core/src/models/selector.rs`
- Modify `crates/roci-core/src/agent_loop/runner.rs`
- Modify `crates/roci-core/src/agent/runtime/config.rs`
- Modify `crates/roci-core/src/agent/runtime.rs`
- Modify `crates/roci-core/src/agent/runtime/run_loop.rs`
- Modify `crates/roci-core/src/agent/runtime/mutations.rs`
- Modify `crates/roci-core/src/agent/runtime/lifecycle.rs`
- Modify `crates/roci-core/src/agent/core.rs`
- Modify `crates/roci-core/src/agent/subagents/launcher.rs`
- Modify `crates/roci-core/src/agent/subagents/profiles.rs`

- [ ] **Step 1: Add candidate tests**

In `candidates.rs`:

```rust
#[test]
fn candidates_dedupe_by_provider_and_model() {
    let list = ModelCandidates::new(vec![
        ModelSelector::parse("openai:gpt-4o").unwrap(),
        ModelSelector::parse("openai:gpt-4o").unwrap(),
        ModelSelector::parse("google:gemini-2.5-pro").unwrap(),
    ])
    .unwrap();

    assert_eq!(list.as_slice().len(), 2);
}

#[test]
fn empty_candidates_is_configuration_error() {
    let err = ModelCandidates::new(Vec::new()).expect_err("empty candidates fail");
    assert!(err.to_string().contains("at least one model candidate"));
}
```

- [ ] **Step 2: Implement model candidate collection**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCandidates {
    candidates: Vec<LanguageModel>,
}

impl ModelCandidates {
    pub fn new(candidates: Vec<LanguageModel>) -> Result<Self, RociError> {
        let mut seen = std::collections::HashSet::new();
        let mut deduped = Vec::new();
        for model in candidates {
            let key = (model.provider_name().to_string(), model.model_id().to_string());
            if seen.insert(key) {
                deduped.push(model);
            }
        }
        if deduped.is_empty() {
            return Err(RociError::Configuration(
                "model candidates must contain at least one model candidate".into(),
            ));
        }
        Ok(Self { candidates: deduped })
    }

    pub fn from_model(model: LanguageModel) -> Self {
        Self { candidates: vec![model] }
    }

    pub fn primary(&self) -> &LanguageModel {
        &self.candidates[0]
    }

    pub fn as_slice(&self) -> &[LanguageModel] {
        &self.candidates
    }

    pub fn into_vec(self) -> Vec<LanguageModel> {
        self.candidates
    }
}
```

Export:

```rust
pub mod candidates;
pub use candidates::ModelCandidates;
```

- [ ] **Step 3: Replace runtime model fields with canonical candidates**

In `AgentConfig` and `RunRequest`, replace runtime `model: LanguageModel` with:

```rust
pub candidates: Vec<LanguageModel>,
```

Do not keep `model + fallback_chain` or parallel `model_candidates` runtime fields. Migration constructors from old single-model call sites must convert directly to `candidates = vec![model]`, then normalize with `ModelCandidates::new`.

```rust
pub fn with_candidates(mut self, candidates: Vec<LanguageModel>) -> Result<Self, RociError> {
    self.candidates = ModelCandidates::new(candidates)?.into_vec();
    Ok(self)
}

pub fn active_model(&self) -> &LanguageModel {
    &self.candidates[0]
}
```

Default:

```rust
let model = LanguageModel::Known {
    provider_key: "openai".to_string(),
    model_id: "gpt-4o".to_string(),
};
Self {
    candidates: vec![model],
    system_prompt: None,
    tools: Vec::new(),
    tool_visibility_policy: ToolVisibilityPolicy::default(),
    dynamic_tool_providers: Vec::new(),
    settings: GenerationSettings::default(),
    transform_context: None,
    convert_to_llm: None,
    before_agent_start: None,
    event_sink: None,
    approval_policy: ApprovalPolicy::Ask,
    approval_handler: None,
    session_id: None,
    session: None,
    sandbox_provider: None,
    steering_mode: QueueDrainMode::All,
    follow_up_mode: QueueDrainMode::All,
    transport: None,
    max_retry_delay_ms: None,
    retry_backoff: RetryBackoffPolicy::default(),
    api_key_override: None,
    provider_headers: reqwest::header::HeaderMap::new(),
    provider_metadata: HashMap::new(),
    provider_payload_callback: None,
    get_api_key: None,
    compaction: CompactionSettings::default(),
    session_before_compact: None,
    session_before_tree: None,
    pre_tool_use: None,
    post_tool_use: None,
    user_input_timeout_ms: None,
    context_budget: None,
    chat: ChatRuntimeConfig::default(),
    #[cfg(feature = "agent")]
    human_interaction_coordinator: None,
}
```

- [ ] **Step 4: Migrate runtime, lifecycle, and subagent call sites**

Required migration rules:
- `AgentRuntime::current_model()` returns `candidates[0]` until retry work adds active candidate state.
- any `set_model(model)` helper becomes `set_candidates(vec![model])` or a deprecated wrapper outside runtime state.
- `before_agent_start`, `transform_context`, `convert_to_llm`, lifecycle/debug payloads use `request.active_model()` or active candidate identity, not removed `request.model`.
- `RunRequest::new(model, messages)` may remain as a migration constructor but stores `candidates = vec![model]` internally.
- child runtime config from resolved `profile.models` assigns `child_config.candidates = ordered_viable_candidates`.
- child retry/health defaults inherit from supervisor base config separately from candidate selection.

- [ ] **Step 5: Verify candidate API**

Run:

```bash
cargo test -p roci-core models::candidates
cargo test -p roci-core agent_loop::runner::tests::request_pipeline
cargo test -p roci-core agent::runtime_tests::subagent_profiles
cargo test -p roci-core agent::subagents::supervisor::tests
```

---

## Task 7: Retry Event And Candidate Advancement

**Files:**
- Modify `crates/roci-core/src/agent_loop/events.rs`
- Modify `crates/roci-core/src/agent_loop/runner.rs`
- Modify `crates/roci-core/src/agent_loop/runner/engine/mod.rs`
- Modify `crates/roci-core/src/agent_loop/runner/engine/llm_phase.rs`
- Modify `crates/roci-core/src/agent/core.rs`
- Modify `crates/roci-cli/src/chat/runtime_events.rs`

- [ ] **Step 1: Add retry event type**

In events:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RetryMode {
    Bounded { max_attempts: u32 },
    Persistent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RetryEventKind {
    RetryScheduled,
    RetryResuming,
    RetryCanceled,
    CandidateAdvancing,
    RetryExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FailureCategory {
    RateLimit,
    Network,
    Server,
    Timeout,
    Overflow,
    Auth,
    Configuration,
    InvalidRequest,
    Tool,
    Canceled,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RetryNextAction {
    Sleep,
    ResumeSameCandidate,
    AdvanceCandidate,
    ReturnFailure,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RetryEvent {
    pub kind: RetryEventKind,
    pub run_id: RunId,
    pub provider: String,
    pub model_id: String,
    pub candidate_index: usize,
    pub attempt: u32,
    pub retry_mode: RetryMode,
    pub failure_category: FailureCategory,
    pub sleep_ms: Option<u64>,
    pub elapsed_retry_ms: u64,
    pub candidates_remaining: usize,
    pub partial_output_seen: bool,
    pub next_action: RetryNextAction,
}

// Add this variant to RunEventPayload:
Retry { event: RetryEvent },
```

Downstream handling:
- `crates/roci-core/src/agent/core.rs::run_event_to_stream_item` treats `RunEventPayload::Retry { .. }` as `None`, like plan/diff/tool-result helper events.
- `crates/roci-cli/src/chat/runtime_events.rs` renders retry events with `kind`, `candidate_index`, `attempt`, `next_action`, `failure_category`, and `sleep_ms`.
- CLI live proof must show `RetryScheduled`, `RetryResuming`, and `CandidateAdvancing` or `RetryExhausted` lines.

- [ ] **Step 2: Add effective retry mode**

Add to `AgentConfig` and `RunRequest`:

```rust
pub retry_mode: RetryMode,
```

Default:

```rust
RetryMode::Bounded { max_attempts: 3 }
```

Validation: `Bounded { max_attempts }` rejects `0` as `RociError::Configuration`; `1` disables same-candidate retry.

- [ ] **Step 3: Move provider creation into candidate loop**

In `engine/mod.rs`, replace single provider creation before loop with active candidate state:

```rust
let candidates = request.candidates.clone();
let mut active_candidate_index = 0usize;
let retry_mode = request.retry_mode;
```

Before each LLM phase, create provider from `candidates[active_candidate_index]`. Candidate advancement is allowed only when all conditions hold:
- failure category is `RateLimit`, `Network`, `Server`, or `Timeout`
- same-candidate bounded retry exhausted
- `active_candidate_index + 1 < candidates.len()`
- no partial streamed assistant output and no tool delta/call emitted
- not canceled
- not overflow before overflow compaction path has run

- [ ] **Step 4: Keep same-candidate retry inside `llm_phase`**

Change `LlmPhaseOutcome::Failed` to carry retry metadata:

```rust
Failed {
    reason: String,
    assistant_message: Option<ModelMessage>,
    retry_exhausted: bool,
    failure_category: FailureCategory,
    partial_output_seen: bool,
}
```

Behavior:
- `RetryMode::Bounded { max_attempts }` counts total provider attempts per candidate including initial attempt.
- `RetryMode::Persistent` ignores bounded count, never advances candidate, and retries current candidate until cancellation or nonretryable failure.
- emit `RetryScheduled` before bounded/persistent retry sleep.
- emit `RetryResuming` after sleep completes.
- emit `RetryCanceled` when cancellation interrupts sleep.
- emit `CandidateAdvancing` when moving to `candidates[i + 1]`.
- emit `RetryExhausted` when no candidate remains or advancement disallowed.
- no heartbeat/cadence events.

- [ ] **Step 5: Add runner tests**

Tests:

```rust
#[tokio::test]
async fn interruptible_advances_after_retry_exhaustion() {
    // Configure candidates [openai:fails, openai:succeeds].
    // Set retry_mode = RetryMode::Bounded { max_attempts: 1 }.
    // Assert provider factory call log == ["openai:fails", "openai:succeeds"].
    // Assert RunResult status is completed and text came from succeeds.
}

#[tokio::test]
async fn persistent_does_not_advance_candidate() {
    // Configure candidates [openai:fails, openai:succeeds].
    // Set retry_mode = RetryMode::Persistent.
    // Cancel during retry sleep.
    // Assert provider factory call log contains only "openai:fails".
    // Assert RunResult status is canceled and no CandidateAdvancing event emitted.
}

#[tokio::test]
async fn single_candidate_returns_error_after_exhaustion() {
    // Configure candidates [openai:fails].
    // Set retry_mode = RetryMode::Bounded { max_attempts: 1 }.
    // Assert provider factory call log == ["openai:fails"].
    // Assert RunResult status is failed and reason contains first candidate error.
}
```

Use custom provider factory keyed by `(provider, model_id)` with first candidate returning retryable `RociError::RateLimited { retry_after_ms: Some(1) }`, second returning text.

Additional tests:
- `nonretryable_auth_does_not_advance_candidate`
- `partial_output_seen_does_not_advance_candidate`
- `tool_delta_seen_does_not_advance_candidate`
- `overflow_compaction_runs_before_candidate_advance`
- `retry_events_include_schedule_resume_cancel_advance_exhausted`

- [ ] **Step 6: Verify retry behavior**

Run:

```bash
cargo test -p roci-core agent_loop::runner::tests -- --nocapture
cargo test -p roci-core agent_loop::runner::tests::request_pipeline
```

---

## Task 8: Model Health Observations

**Files:**
- Create `crates/roci-core/src/models/health.rs`
- Modify `crates/roci-core/src/models/mod.rs`
- Modify `crates/roci-core/src/agent_loop/runner.rs`
- Modify `crates/roci-core/src/agent_loop/runner/engine/mod.rs`

- [ ] **Step 1: Add health tests**

```rust
#[test]
fn health_last_observation_wins() {
    let health = ModelHealthTracker::new_session(Arc::new(SharedModelHealthRegistry::default()));
    let key = ModelHealthKey::from_model(&ModelSelector::parse("openai:gpt-4o").unwrap());

    health.observe(HealthSignal::Success { key: key.clone(), observed_at_ms: 10 });
    assert_eq!(health.snapshot(&key).status, ModelHealthStatus::Healthy);

    health.observe(HealthSignal::RetryExhausted {
        candidate_index: 0,
        key: key.clone(),
        category: FailureCategory::RateLimit,
        observed_at_ms: 20,
    });
    assert_eq!(health.snapshot(&key).status, ModelHealthStatus::Unhealthy);
}
```

- [ ] **Step 2: Implement session-local health tracker**

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelHealthKey {
    pub provider: String,
    pub model_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthSignal {
    Success {
        key: ModelHealthKey,
        observed_at_ms: u64,
    },
    TransientFailure {
        key: ModelHealthKey,
        category: FailureCategory,
        observed_at_ms: u64,
    },
    NonRetryableFailure {
        key: ModelHealthKey,
        category: FailureCategory,
        observed_at_ms: u64,
    },
    RetryExhausted {
        candidate_index: usize,
        key: ModelHealthKey,
        category: FailureCategory,
        observed_at_ms: u64,
    },
    CandidateAdvanced {
        from_index: usize,
        to_index: usize,
        from: ModelHealthKey,
        to: ModelHealthKey,
        reason: FailureCategory,
        observed_at_ms: u64,
    },
    Canceled {
        key: ModelHealthKey,
        observed_at_ms: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelHealthStatus {
    Unknown,
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelHealthSnapshot {
    pub key: ModelHealthKey,
    pub status: ModelHealthStatus,
    pub consecutive_transient_failures: u32,
    pub last_failure_category: Option<FailureCategory>,
    pub last_failure_at_ms: Option<u64>,
    pub last_success_at_ms: Option<u64>,
}

#[derive(Default)]
pub struct SharedModelHealthRegistry {
    snapshots: std::sync::Mutex<std::collections::HashMap<ModelHealthKey, ModelHealthSnapshot>>,
}

impl SharedModelHealthRegistry {
    pub fn observe(&self, snapshot: ModelHealthSnapshot);
    pub fn snapshot(&self, key: &ModelHealthKey) -> ModelHealthSnapshot;
}

pub struct ModelHealthTracker {
    local: std::sync::Mutex<std::collections::HashMap<ModelHealthKey, ModelHealthSnapshot>>,
    shared: Arc<SharedModelHealthRegistry>,
}

impl ModelHealthKey {
    pub fn from_model(model: &LanguageModel) -> Self {
        Self {
            provider: model.provider_name().to_string(),
            model_id: model.model_id().to_string(),
        }
    }
}

impl ModelHealthTracker {
    pub fn new_session(shared: Arc<SharedModelHealthRegistry>) -> Self;
    pub fn observe(&self, signal: HealthSignal);
    pub fn snapshot(&self, key: &ModelHealthKey) -> ModelHealthSnapshot;
}
```

Implementation details:
- `ModelHealthTracker::new_session(shared)` creates fresh run/session-local state.
- `snapshot` overlays local snapshot over shared global snapshot for same key.
- shared merge uses last-observation-wins by observation timestamp.
- no run stores `Arc<ModelHealthTracker>` in reusable config because that would leak local state across runs.

Mapping:
- no observations -> `Unknown`
- `Success` -> `Healthy`, resets transient count
- 1-2 consecutive `TransientFailure` -> `Degraded`
- >=3 consecutive `TransientFailure` or transient `RetryExhausted` -> `Unhealthy`
- `NonRetryableFailure` and `Canceled` are recorded but do not degrade provider/model health
- `CandidateAdvanced` is recorded as signal history; registry does not trigger advancement or reorder candidates

No background probes.

- [ ] **Step 3: Wire tracker into request/runtime**

Add shared registry to runtime config:

```rust
pub model_health_registry: Arc<SharedModelHealthRegistry>,
```

At run start:

```rust
let model_health = ModelHealthTracker::new_session(config.model_health_registry.clone());
```

Each run has fresh session-local tracker state overlaying shared in-process global snapshots. Shared snapshot merge is last-observation-wins by `observed_at_ms`. No disk persistence, background worker, daemon, probe, or heartbeat.

- [ ] **Step 4: Verify health events through runner tests**

Run:

```bash
cargo test -p roci-core models::health
cargo test -p roci-core agent_loop::runner::tests::interruptible_advances_after_retry_exhaustion -- --nocapture
```

Assert failed first candidate becomes `Unhealthy` after retry exhaustion, successful second candidate becomes `Healthy`, auth/config nonretryable failure does not degrade health, and `CandidateAdvanced` preserves source/target candidate indices.

---

## Task 9: CLI Wiring And Live Verification

**Files:**
- Modify `crates/roci-cli/src/cli/mod.rs`
- Modify `crates/roci-cli/src/chat.rs`
- Modify `crates/roci-cli/src/chat/mcp.rs`
- Modify `crates/roci-cli/src/chat/runtime_events.rs`
- Modify `docs/testing.md`
- Optional modify `.env.example`

- [ ] **Step 1: Add CLI parse tests**

CLI additions:

```rust
#[arg(long = "candidate-model", value_name = "PROVIDER:MODEL")]
pub candidate_models: Vec<String>,

#[arg(long = "retry-mode", value_enum, default_value_t = ChatRetryModeArg::Bounded)]
pub retry_mode: ChatRetryModeArg,

#[arg(long = "max-retry-attempts", default_value_t = 3)]
pub max_retry_attempts: u32,

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, Eq)]
pub enum ChatRetryModeArg {
    Bounded,
    Persistent,
}
```

Tests:

```rust
#[test]
fn parses_candidate_models() {
    let cli = Cli::try_parse_from([
        "roci",
        "chat",
        "--model",
        "openai:gpt-4o",
        "--candidate-model",
        "google:gemini-2.5-pro",
    ])
    .expect("parse");
    match cli.command {
        Commands::Chat(args) => {
            assert_eq!(args.model, "openai:gpt-4o");
            assert_eq!(args.candidate_models, vec!["google:gemini-2.5-pro"]);
            assert_eq!(args.retry_mode, ChatRetryModeArg::Bounded);
            assert_eq!(args.max_retry_attempts, 3);
        }
        other => panic!("expected chat command, got {other:?}"),
    }
}
```

- [ ] **Step 2: Wire candidates into chat config**

In `chat.rs`:

```rust
let mut candidates = vec![model.clone()];
for selector in candidate_models {
    candidates.push(ModelSelector::parse(&selector)?);
}
let candidates = ModelCandidates::new(candidates)?.into_vec();
let retry_mode = match retry_mode {
    ChatRetryModeArg::Bounded => RetryMode::Bounded {
        max_attempts: max_retry_attempts,
    },
    ChatRetryModeArg::Persistent => RetryMode::Persistent,
};
let agent_config = AgentConfig {
    candidates,
    retry_mode,
    // existing non-model fields unchanged
};
```

Because `handle_chat` destructures `ChatArgs`, add `candidate_models`, `retry_mode`, and `max_retry_attempts` to the destructuring block before using them.

- [ ] **Step 3: Add live verification docs**

Add to `docs/testing.md`:

```bash
# Terminal 1: test provider
ssh framed 'true'

# Terminal 2: roci CLI model candidate smoke
tmux new -s roci-candidate-live
OPENAI_API_KEY=sk-local-dummy \
OPENAI_BASE_URL=http://framed:4001/v1 \
cargo run -p roci-cli -- chat \
  --model openai:bad-model-for-fallback \
  --candidate-model openai:gpt-4o-mini \
  --retry-mode bounded \
  --max-retry-attempts 1
```

Expected: first candidate fails, retry event observed, second candidate returns model response.

Add MCP live gate:

```bash
# Terminal 1: MCP fixture server with Streamable HTTP + WebSocket endpoints
tmux new -s roci-mcp-live
cargo run -p roci-cli -- mcp-fixture-server --streamable-http 127.0.0.1:18881 --websocket 127.0.0.1:18882

# Terminal 2: Streamable HTTP initialize/list/call through current roci-cli
cargo run -p roci-cli -- chat \
  --mcp-streamable-http 'id=fixture-http,label=Fixture HTTP,url=http://127.0.0.1:18881/mcp' \
  --model openai:gpt-4o-mini \
  'Use the fixture echo tool with {"message":"http-ok"}'

# Terminal 3: WebSocket initialize/list/call through current roci-cli
cargo run -p roci-cli -- chat \
  --mcp-websocket 'id=fixture-ws,label=Fixture WS,url=ws://127.0.0.1:18882/mcp' \
  --model openai:gpt-4o-mini \
  'Use the fixture echo tool with {"message":"ws-ok"}'
```

If no fixture-server subcommand exists, implementation must add a hidden test-only fixture command or equivalent integration test harness before closing MCP transport tasks. Evidence must show initialize, `tools/list`, and `tools/call` for both Streamable HTTP and WebSocket.

- [ ] **Step 4: Run automated gates**

Run:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --features mcp -- -D warnings
cargo test --workspace --features mcp
```

- [ ] **Step 5: Run live tmux verification**

Start tmux:

```bash
tmux new -s roci-foundation-live
```

Tell user:

```bash
tmux attach -t roci-foundation-live
```

Run CLI binary from current changes, not installed stale binary:

```bash
OPENAI_API_KEY=sk-local-dummy \
OPENAI_BASE_URL=http://framed:4001/v1 \
cargo run -p roci-cli -- chat \
  --model openai:bad-model-for-fallback \
  --candidate-model openai:gpt-4o-mini \
  --retry-mode bounded \
  --max-retry-attempts 1
```

Live proof must show:
- `roci-cli` binary from workspace ran.
- First configured candidate attempted and failed.
- Retry/fallback path attempted next candidate.
- Model response returned after switch.
- No session/app data written into project CWD unless explicitly configured.

---

## Integration Checklist

- [ ] `for id in tsq-p4cpczyg.6 tsq-p4cpczyg.1 tsq-p4cpczyg.2.1 tsq-1av9jz0z.2.1 tsq-1av9jz0z.3.1 tsq-1av9jz0z.4.1 tsq-g6ba4ega.1 tsq-g6ba4ega.2 tsq-g6ba4ega.3; do tsq spec --check "$id"; done`
- [ ] `cargo fmt --all`
- [ ] `cargo clippy --workspace --all-targets --features mcp -- -D warnings`
- [ ] `cargo test --workspace --features mcp`
- [ ] tmux live fallback verification with `cargo run -p roci-cli`
- [ ] Update task notes with exact commands and live evidence.

## Risks And Mitigations

- MCP WebSocket support may not exist in current `rmcp` version. Mitigation: implement a small `RmcpTransport<RoleClient>` adapter over `tokio_tungstenite`; do not close `tsq-p4cpczyg.1` until initialize/list/call WebSocket tests pass.
- Candidate advancement touches runner control flow. Mitigation: test with provider factory that records `(provider, model_id)` attempts and emits retry events.
- `AgentConfig`/`RunRequest` migration touches many callers. Mitigation: remove runtime `model` field in one coordinated refactor and use `candidates[0]`/active candidate helpers where old code read model identity.
- Redaction offsets can break with Unicode. Mitigation: byte-offset tests include `café`.
- Filesystem symlink policy is platform-sensitive. Mitigation: tempdir tests avoid hardcoded symlink support except behind Unix cfg.
