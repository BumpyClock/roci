//! Native Cursor AgentService transport (HTTP/2, Connect and protobuf).
//!
//! Compatible completed checkpoints survive provider/process recreation. SDK
//! tool execution remains host-owned; in-flight tool execution IDs belong to
//! the old stream and use complete-history continuation when that stream closes.

pub mod models;
pub mod state;
mod wire;

use std::{collections::HashMap, sync::Arc, time::Duration};

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::{stream::BoxStream, StreamExt};
use prost::Message;
use roci_core::{
    error::RociError,
    models::capabilities::ModelCapabilities,
    provider::{ModelProvider, ProviderRequest, ProviderResponse},
    types::{
        message::{AgentToolCall, AgentToolResult, ContentPart, Role, ThinkingContent},
        FinishReason, StreamEventType, TextStreamDelta, Usage,
    },
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use state::{
    CursorSessionLease, CursorSessionSnapshot, CursorSessionStore, FileCursorSessionStore,
};
use wire::{error, Frame};

const BASE_URL: &str = "https://api2.cursor.sh";
const CLIENT_VERSION: &str = "cli-2026.02.13-41ac335";
const MAX_BLOBS_BYTES: usize = 32 * 1024 * 1024;
const MAX_BLOBS: usize = 4096;

pub struct CursorProvider {
    model: String,
    access_token: String,
    base_url: String,
    capabilities: ModelCapabilities,
    account_id: Option<String>,
    account_namespace: String,
    session_store: Option<Arc<dyn CursorSessionStore>>,
}

impl CursorProvider {
    pub fn new(model: String, access_token: String, base_url: Option<String>) -> Self {
        Self {
            model,
            access_token,
            base_url: base_url.unwrap_or_else(|| BASE_URL.into()),
            capabilities: ModelCapabilities {
                supports_vision: false,
                supports_tools: true,
                supports_streaming: true,
                supports_json_mode: false,
                supports_json_schema: false,
                supports_reasoning: true,
                supported_speeds: vec![
                    roci_core::types::GenerationSpeed::Standard,
                    roci_core::types::GenerationSpeed::Fast,
                ],
                supports_system_messages: true,
                ..ModelCapabilities::default()
            },
            account_id: None,
            account_namespace: "default".into(),
            session_store: None,
        }
    }

    pub fn with_account_id(mut self, account_id: String) -> Self {
        self.account_id = Some(account_id);
        self
    }

    pub fn with_account_namespace(mut self, namespace: String) -> Self {
        self.account_namespace = namespace;
        self
    }

    pub fn with_session_store(mut self, store: Arc<dyn CursorSessionStore>) -> Self {
        self.session_store = Some(store);
        self
    }

    fn session_lease(
        &self,
        request: &ProviderRequest,
        model: &str,
    ) -> Result<Option<Box<dyn CursorSessionLease>>, RociError> {
        let Some(session_id) = request.session_id.as_deref() else {
            return Ok(None);
        };
        if request.headers.contains_key(reqwest::header::AUTHORIZATION) {
            return Err(error("use api_key_override instead of Authorization headers with durable Cursor sessions"));
        }
        let token = self.token(request)?;
        let jwt_account = token
            .split('.')
            .nth(1)
            .and_then(|part| {
                base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(part)
                    .ok()
            })
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|claims| claims["sub"].as_str().map(str::to_owned));
        // Overrides can select a different account; prefer their token's subject.
        let account = if request.api_key_override.is_none() {
            self.account_id.clone().or(jwt_account)
        } else {
            jwt_account.map(|account| format!("override:{account}"))
        }
        .ok_or_else(|| error("Cursor durable sessions require an account identity"))?;
        let scoped_account = digest(&(
            &self.account_namespace,
            request.api_key_override.is_some(),
            account,
        ))?;
        let key = session_key(&scoped_account, model, &self.base_url, session_id);
        let store: Arc<dyn CursorSessionStore> = match &self.session_store {
            Some(store) => store.clone(),
            None => Arc::new(FileCursorSessionStore::new_default()?),
        };
        store.acquire(&key).map(Some)
    }

    fn token<'a>(&'a self, request: &'a ProviderRequest) -> Result<&'a str, RociError> {
        request
            .api_key_override
            .as_deref()
            .or(Some(self.access_token.as_str()))
            .filter(|s| !s.is_empty())
            .ok_or_else(|| RociError::MissingCredential {
                provider: "cursor".into(),
            })
    }

    /// Fetch Cursor's native protobuf model catalog. No static result is passed
    /// off as a successful authenticated request when the endpoint fails.
    pub async fn list_model_ids(&self) -> Result<Vec<String>, RociError> {
        self.list_model_ids_with_token(&self.access_token).await
    }

    async fn list_model_ids_with_token(&self, token: &str) -> Result<Vec<String>, RociError> {
        let client = http_client()?;
        let response = cursor_identity_headers(
            client.post(format!(
                "{}/agent.v1.AgentService/GetUsableModels",
                self.base_url.trim_end_matches('/')
            )),
            token,
        )
        .header("content-type", "application/proto")
        .body(Vec::new())
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|_| error("Cursor model request failed"))?;
        check_status(&response)?;
        let body = bounded_body(response, wire::MAX_FRAME).await?;
        let payload = if body.len() >= 5
            && body[0] == 0
            && u32::from_be_bytes([body[1], body[2], body[3], body[4]]) as usize == body.len() - 5
        {
            &body[5..]
        } else {
            body.as_slice()
        };
        let value = wire::decode("GetUsableModelsResponse", payload)?;
        let mut models = Vec::new();
        collect_model_ids(&value, &mut models);
        models.sort();
        models.dedup();
        if models.is_empty() {
            return Err(error("Cursor returned no usable models"));
        }
        Ok(models)
    }
}

fn collect_model_ids(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            if let Some(id) = fields
                .get("modelId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                output.push(id.into());
            }
            for child in fields.values() {
                collect_model_ids(child, output);
            }
        }
        Value::Array(values) => {
            for child in values {
                collect_model_ids(child, output);
            }
        }
        _ => {}
    }
}

fn http_client() -> Result<reqwest::Client, RociError> {
    reqwest::Client::builder()
        .http2_prior_knowledge()
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|_| error("could not create Cursor HTTP/2 client"))
}

fn cursor_headers(builder: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    cursor_identity_headers(builder, token)
        .header("content-type", "application/connect+proto")
        .header("connect-protocol-version", "1")
}

fn cursor_identity_headers(
    builder: reqwest::RequestBuilder,
    token: &str,
) -> reqwest::RequestBuilder {
    builder
        .bearer_auth(token)
        .header("te", "trailers")
        .header("x-ghost-mode", "true")
        .header("x-cursor-client-version", CLIENT_VERSION)
        .header("x-cursor-client-type", "cli")
        .header("x-request-id", uuid::Uuid::new_v4().to_string())
}

fn check_status(response: &reqwest::Response) -> Result<(), RociError> {
    match response.status().as_u16() {
        200..=299 => Ok(()),
        401 | 403 => Err(wire::api_error(
            response.status().as_u16(),
            "Cursor authentication rejected",
        )),
        429 => Err(RociError::RateLimited {
            retry_after_ms: None,
        }),
        status => Err(wire::api_error(status, "Cursor request failed")),
    }
}

async fn bounded_body(mut response: reqwest::Response, limit: usize) -> Result<Vec<u8>, RociError> {
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| error("Cursor response interrupted"))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(error("Cursor response exceeds size limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

struct Prepared {
    message: Value,
    tools: Value,
    tool_names: Vec<String>,
    blobs: HashMap<String, Vec<u8>>,
    checkpoint: Option<Value>,
    input_messages: usize,
    input_digest: String,
    assistant_text: String,
    completed_tools: HashMap<String, (AgentToolCall, AgentToolResult)>,
}

fn digest(value: &impl serde::Serialize) -> Result<String, RociError> {
    let bytes =
        serde_json::to_vec(value).map_err(|_| error("cannot fingerprint Cursor conversation"))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn session_key(account: &str, model: &str, endpoint: &str, session: &str) -> String {
    let mut hash = Sha256::new();
    for part in [account, model, endpoint, session] {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn history_digest(messages: &[roci_core::types::ModelMessage]) -> Result<String, RociError> {
    // Wall-clock timestamps and display metadata do not change model input.
    let content: Vec<_> = messages
        .iter()
        .map(|m| (&m.role, &m.content, &m.name))
        .collect();
    digest(&content)
}

fn restore_checkpoint(
    prepared: &mut Prepared,
    request: &ProviderRequest,
    saved: CursorSessionSnapshot,
) -> Result<bool, RociError> {
    let n = saved.input_messages;
    if !saved.completed_text_turn
        || request.messages.len() != n.saturating_add(2)
        || n >= request.messages.len()
        || history_digest(&request.messages[..n])? != saved.input_digest
    {
        return Ok(false);
    }
    let assistant = &request.messages[n];
    let latest = &request.messages[n + 1];
    if assistant.role != Role::Assistant
        || assistant.text() != saved.assistant_text
        || !assistant.tool_calls().is_empty()
        || latest.role != Role::User
        || !latest
            .content
            .iter()
            .all(|p| matches!(p, ContentPart::Text { .. }))
    {
        return Ok(false);
    }
    // Revalidate persisted protobuf and blob bounds before putting them on wire.
    wire::encode("ConversationStateStructure", saved.checkpoint.clone())?;
    let mut blobs = HashMap::new();
    let mut total = 0usize;
    if saved.blobs.len() > MAX_BLOBS {
        return Err(error("Cursor saved blob count exceeds limit"));
    }
    for (id, data) in saved.blobs {
        validate_blob_key(&id)?;
        let bytes = STANDARD
            .decode(data)
            .map_err(|_| error("invalid saved Cursor blob"))?;
        total = total.saturating_add(bytes.len());
        if total > MAX_BLOBS_BYTES {
            return Err(error("Cursor saved blob storage exceeds limit"));
        }
        blobs.insert(id, bytes);
    }
    prepared.blobs = blobs;
    prepared.message["runRequest"]["conversationState"] = saved.checkpoint;
    prepared.message["runRequest"]["conversationId"] = json!(saved.conversation_id);
    prepared.message["runRequest"]["action"]["userMessageAction"]["userMessage"]["text"] =
        json!(latest.text());
    Ok(true)
}

fn save_checkpoint(
    prepared: &Prepared,
    lease: &mut Option<Box<dyn CursorSessionLease>>,
    complete: bool,
) -> Result<(), RociError> {
    let (Some(checkpoint), Some(lease)) = (&prepared.checkpoint, lease) else {
        return Ok(());
    };
    lease.save(&CursorSessionSnapshot {
        version: 1,
        conversation_id: prepared.message["runRequest"]["conversationId"]
            .as_str()
            .unwrap_or_default()
            .into(),
        checkpoint: checkpoint.clone(),
        blobs: prepared
            .blobs
            .iter()
            .map(|(k, v)| (k.clone(), STANDARD.encode(v)))
            .collect(),
        input_messages: prepared.input_messages,
        input_digest: prepared.input_digest.clone(),
        assistant_text: prepared.assistant_text.clone(),
        completed_text_turn: complete,
    })
}

fn prepare(model: &str, request: &ProviderRequest) -> Result<Prepared, RociError> {
    if request.response_format.is_some() {
        return Err(RociError::UnsupportedOperation(
            "Cursor structured responses are not supported".into(),
        ));
    }
    // Cursor AgentService has no general sampling-settings fields. Fail
    // explicitly instead of advertising that temperature/token caps were used.
    models::validate_settings(&request.settings)?;
    let mut systems = Vec::new();
    let mut history = Vec::new();
    let mut calls = HashMap::new();
    let mut completed_tools = HashMap::new();
    for message in &request.messages {
        if message.role == Role::System
            && !message
                .content
                .iter()
                .all(|p| matches!(p, ContentPart::Text { .. }))
        {
            return Err(RociError::UnsupportedOperation(
                "Cursor system messages must contain text only".into(),
            ));
        }
        let mut content = Vec::new();
        for part in &message.content {
            match part {
                ContentPart::Text { text } => content.push(json!({"text":text})),
                ContentPart::ToolCall(call) => { calls.insert(call.id.clone(), call.clone()); content.push(json!({"tool_call":{"id":call.id,"name":call.name,"arguments":call.arguments}})); },
                ContentPart::ToolResult(result) => { if let Some(call) = calls.get(&result.tool_call_id) { completed_tools.insert(result.tool_call_id.clone(), (call.clone(), result.clone())); } content.push(json!({"tool_result":{"id":result.tool_call_id,"result":result.result,"is_error":result.is_error}})); },
                ContentPart::Thinking(thinking) => content.push(json!({"reasoning":thinking.thinking})),
                ContentPart::Image(_) | ContentPart::RedactedThinking(_) => return Err(RociError::UnsupportedOperation("Cursor currently supports text and SDK tool messages; images and opaque thinking blocks are unsupported".into())),
            }
        }
        if message.role == Role::System {
            systems.push(message.text());
        } else {
            history.push(json!({"role":message.role,"content":content}));
        }
    }
    if history.is_empty() {
        return Err(RociError::InvalidArgument(
            "Cursor requires a conversation message".into(),
        ));
    }
    let system_blob = serde_json::to_vec(&json!({"role":"system", "content":systems.join("\n\n")}))
        .map_err(|_| error("invalid Cursor system message"))?;
    if system_blob.len() > wire::MAX_FRAME / 2 {
        return Err(error("Cursor system prompt exceeds blob frame limit"));
    }
    let blob_id = STANDARD.encode(Sha256::digest(&system_blob));
    let mut blobs = HashMap::new();
    blobs.insert(blob_id.clone(), system_blob);
    let tools: Vec<Value> = request.tools.as_deref().unwrap_or_default().iter().map(|tool| {
        json!({"name":tool.name,"toolName":tool.name,"providerIdentifier":"roci",
            "description":tool.description,"inputSchema":STANDARD.encode(wire::protobuf_value(&tool.parameters).encode_to_vec())})
    }).collect();
    let tool_names = request
        .tools
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|t| t.name.clone())
        .collect();
    let user_text = if request.messages.len() == 1
        && request.messages[0]
            .content
            .iter()
            .all(|part| matches!(part, ContentPart::Text { .. }))
    {
        request.messages[0].text()
    } else {
        serde_json::to_string(&history).map_err(|_| error("invalid Cursor conversation"))?
    };
    let message = json!({"runRequest":{
        "conversationState":{"rootPromptMessagesJson":[blob_id]},
        "action":{"userMessageAction":{"userMessage":{"text":user_text,"messageId":uuid::Uuid::new_v4().to_string()}}},
        "modelDetails":{"modelId":model,"displayModelId":model,"displayName":model},
        "conversationId":uuid::Uuid::new_v4().to_string(),
        "mcpTools":{"mcpTools":tools}
    }});
    Ok(Prepared {
        message,
        tools: Value::Array(tools),
        tool_names,
        blobs,
        checkpoint: None,
        input_messages: request.messages.len(),
        input_digest: history_digest(&request.messages)?,
        assistant_text: String::new(),
        completed_tools,
    })
}

fn delta(event_type: StreamEventType) -> TextStreamDelta {
    TextStreamDelta {
        text: String::new(),
        event_type,
        tool_call: None,
        finish_reason: None,
        usage: None,
        reasoning: None,
        reasoning_signature: None,
        reasoning_type: None,
    }
}

enum Action {
    Reply(Value),
    Delta(TextStreamDelta),
    Tool(AgentToolCall),
    Usage(u32),
    Done,
    Checkpoint,
}

fn validate_blob_key(key: &str) -> Result<(), RociError> {
    if key.is_empty() || key.len() > 128 || STANDARD.decode(key).is_err() {
        return Err(error("invalid Cursor blob identifier"));
    }
    Ok(())
}

fn process(message: Value, prepared: &mut Prepared) -> Result<Vec<Action>, RociError> {
    let mut actions = Vec::new();
    if let Some(kv) = message.get("kvServerMessage") {
        let id = kv.get("id").cloned().unwrap_or(json!(0));
        if let Some(get) = kv.get("getBlobArgs") {
            let key = get["blobId"]
                .as_str()
                .ok_or_else(|| error("Cursor blob request missing ID"))?;
            validate_blob_key(key)?;
            let data = prepared
                .blobs
                .get(key)
                .ok_or_else(|| error("Cursor requested an unknown conversation blob"))?;
            actions.push(Action::Reply(json!({"kvClientMessage":{"id":id,"getBlobResult":{"blobData":STANDARD.encode(data)}}})));
        } else if let Some(set) = kv.get("setBlobArgs") {
            let key = set["blobId"]
                .as_str()
                .ok_or_else(|| error("Cursor blob update missing ID"))?
                .to_owned();
            validate_blob_key(&key)?;
            let data = wire::bytes(&set["blobData"])?;
            let prior = prepared.blobs.get(&key).map_or(0, Vec::len);
            let total: usize = prepared.blobs.values().map(Vec::len).sum();
            if total.saturating_sub(prior).saturating_add(data.len()) > MAX_BLOBS_BYTES
                || (!prepared.blobs.contains_key(&key) && prepared.blobs.len() >= MAX_BLOBS)
            {
                return Err(error("Cursor conversation blob limit exceeded"));
            }
            prepared.blobs.insert(key, data);
            actions.push(Action::Reply(
                json!({"kvClientMessage":{"id":id,"setBlobResult":{}}}),
            ));
        } else {
            return Err(error("unsupported Cursor blob operation"));
        }
    }
    if let Some(exec) = message.get("execServerMessage") {
        let mut reply = json!({"id":exec.get("id").cloned().unwrap_or(json!(0)),"execId":exec.get("execId").cloned().unwrap_or(json!(""))});
        if exec.get("requestContextArgs").is_some() {
            reply["requestContextResult"] =
                json!({"success":{"requestContext":{"tools":prepared.tools}}});
        } else if let Some(mcp) = exec.get("mcpArgs") {
            let name = mcp
                .get("toolName")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    mcp.get("name")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                })
                .ok_or_else(|| error("Cursor tool call missing name"))?;
            if !prepared.tool_names.iter().any(|n| n == name) {
                return Err(error("Cursor requested an unregistered SDK tool"));
            }
            let id = mcp
                .get("toolCallId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let arguments = wire::tool_arguments(&mcp["args"])?;
            if let Some((call, result)) = prepared.completed_tools.get(&id) {
                if call.name != name || call.arguments != arguments {
                    return Err(error(
                        "Cursor reused a completed tool ID with different arguments",
                    ));
                }
                let text = result
                    .result
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| result.result.to_string());
                reply["mcpResult"] = json!({"success":{"content":[{"text":{"text":text}}],"isError":result.is_error}});
            } else {
                actions.push(Action::Tool(AgentToolCall {
                    id,
                    name: name.into(),
                    arguments,
                    called_as: None,
                    recipient: None,
                }));
            }
        } else {
            let reason = "Execute actions through the provided SDK tools; direct host operations are unavailable";
            let mut found = false;
            for (args, result) in [
                ("readArgs", "readResult"),
                ("writeArgs", "writeResult"),
                ("deleteArgs", "deleteResult"),
                ("lsArgs", "lsResult"),
            ] {
                if let Some(value) = exec.get(args) {
                    reply[result] = json!({"rejected":{"path":value.get("path").cloned().unwrap_or(json!("")),"reason":reason}});
                    found = true;
                }
            }
            for (args, result) in [
                ("shellArgs", "shellResult"),
                ("shellStreamArgs", "shellStream"),
                ("backgroundShellSpawnArgs", "backgroundShellSpawnResult"),
            ] {
                if let Some(value) = exec.get(args) {
                    reply[result] = json!({"rejected":{"command":value.get("command").cloned().unwrap_or(json!("")),"workingDirectory":value.get("workingDirectory").cloned().unwrap_or(json!("")),"reason":reason}});
                    found = true;
                }
            }
            for (args, result) in [
                ("grepArgs", "grepResult"),
                ("fetchArgs", "fetchResult"),
                ("writeShellStdinArgs", "writeShellStdinResult"),
                ("diagnosticsArgs", "diagnosticsResult"),
            ] {
                if exec.get(args).is_some() {
                    reply[result] = json!({"error":{"error":reason}});
                    found = true;
                }
            }
            if !found {
                return Err(RociError::UnsupportedOperation(
                    "unsupported Cursor execution request".into(),
                ));
            }
        }
        if !actions.iter().any(|a| matches!(a, Action::Tool(_))) {
            actions.push(Action::Reply(json!({"execClientMessage":reply})));
        }
    }
    if message.get("interactionQuery").is_some() {
        return Err(RociError::UnsupportedOperation(
            "Cursor requested an unsupported interactive decision".into(),
        ));
    }
    if let Some(checkpoint) = message.get("conversationCheckpointUpdate") {
        prepared.checkpoint = Some(checkpoint.clone());
        actions.push(Action::Checkpoint);
    }
    if let Some(update) = message.get("interactionUpdate") {
        if let Some(text) = update.pointer("/textDelta/text").and_then(Value::as_str) {
            let mut output = delta(StreamEventType::TextDelta);
            output.text = text.into();
            actions.push(Action::Delta(output));
        }
        if let Some(text) = update
            .pointer("/thinkingDelta/text")
            .and_then(Value::as_str)
        {
            let mut output = delta(StreamEventType::Reasoning);
            output.reasoning = Some(text.into());
            output.reasoning_type = Some("thinking".into());
            actions.push(Action::Delta(output));
        }
        if let Some(count) = update.pointer("/tokenDelta/tokens").and_then(Value::as_u64) {
            actions.push(Action::Usage(u32::try_from(count).unwrap_or(u32::MAX)));
        }
        if update.get("turnEnded").is_some() {
            actions.push(Action::Done);
        }
    }
    // A frame can contain both a control request and an interaction update.
    // Preserve all nonterminal content before ending at a tool/turn boundary.
    if actions.iter().any(|a| matches!(a, Action::Tool(_))) {
        actions.retain(|a| !matches!(a, Action::Done));
    }
    actions.sort_by_key(|a| matches!(a, Action::Tool(_) | Action::Done));
    Ok(actions)
}

#[async_trait]
impl ModelProvider for CursorProvider {
    fn provider_name(&self) -> &str {
        "cursor"
    }
    fn model_id(&self) -> &str {
        &self.model
    }
    fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    async fn generate_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<ProviderResponse, RociError> {
        let mut stream = self.stream_text(request).await?;
        let mut response = ProviderResponse {
            text: String::new(),
            usage: Usage::default(),
            tool_calls: Vec::new(),
            finish_reason: None,
            thinking: Vec::new(),
        };
        let mut reasoning = String::new();
        while let Some(item) = stream.next().await {
            let item = item?;
            response.text.push_str(&item.text);
            if let Some(text) = item.reasoning {
                reasoning.push_str(&text);
            }
            if let Some(tool) = item.tool_call {
                response.tool_calls.push(tool);
            }
            if let Some(usage) = item.usage {
                response.usage = usage;
            }
            if item.finish_reason.is_some() {
                response.finish_reason = item.finish_reason;
            }
        }
        if !reasoning.is_empty() {
            response
                .thinking
                .push(ContentPart::Thinking(ThinkingContent {
                    thinking: reasoning,
                    signature: String::new(),
                }));
        }
        Ok(response)
    }

    async fn stream_text(
        &self,
        request: &ProviderRequest,
    ) -> Result<BoxStream<'static, Result<TextStreamDelta, RociError>>, RociError> {
        models::validate_settings(&request.settings)?;
        let model = if models::is_explicit_variant(&self.model) {
            models::resolve(&self.model, &request.settings, &[])?
        } else {
            if request.headers.contains_key(reqwest::header::AUTHORIZATION) {
                return Err(error("use api_key_override instead of Authorization headers for Cursor family selection"));
            }
            let ids = self.list_model_ids_with_token(self.token(request)?).await?;
            models::resolve(&self.model, &request.settings, &ids)?
        };
        let mut prepared = prepare(&model, request)?;
        let mut lease = self.session_lease(request, &model)?;
        if let Some(saved) = lease
            .as_ref()
            .map(|lease| lease.load())
            .transpose()?
            .flatten()
        {
            restore_checkpoint(&mut prepared, request, saved)?;
        }
        if let Some(callback) = &request.payload_callback {
            callback(prepared.message.clone());
        }
        let initial = wire::client(prepared.message.clone())?;
        let token = self.token(request)?;
        let (sender, receiver) = mpsc::channel::<Vec<u8>>(8);
        sender
            .try_send(initial)
            .map_err(|_| error("could not queue Cursor request"))?;
        let body = futures::stream::unfold(receiver, |mut receiver| async {
            receiver
                .recv()
                .await
                .map(|bytes| (Ok::<_, std::io::Error>(bytes), receiver))
        });
        let client = http_client()?;
        let response = tokio::time::timeout(
            Duration::from_secs(30),
            cursor_headers(
                client.post(format!(
                    "{}/agent.v1.AgentService/Run",
                    self.base_url.trim_end_matches('/')
                )),
                token,
            )
            .headers(request.headers.clone())
            .body(reqwest::Body::wrap_stream(body))
            .send(),
        )
        .await
        .map_err(|_| RociError::Timeout(30_000))?
        .map_err(|_| error("Cursor HTTP/2 request failed"))?;
        check_status(&response)?;
        let idle = Duration::from_millis(
            request
                .settings
                .stream_idle_timeout_ms
                .unwrap_or(90_000)
                .max(1),
        );
        let output = async_stream::try_stream! {
            // The upload sender is owned by the returned stream. Dropping the
            // stream cancels the response and closes upload without a detached task.
            let sender = sender;
            let mut input = response.bytes_stream();
            let mut heartbeat = tokio::time::interval(Duration::from_secs(5));
            heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut deadline = tokio::time::Instant::now() + idle;
            let mut buffer = Vec::new();
            let mut usage = Usage::default();
            let mut finished = false;
            let mut started = false;
            loop {
                while let Some(frame) = wire::take_frame(&mut buffer)? {
                    let actions = match frame { Frame::End => vec![Action::Done], Frame::Message(message) => process(message, &mut prepared)? };
                    for action in actions {
                        match action {
                            Action::Reply(value) => {
                                let bytes = wire::client(value)?;
                                tokio::time::timeout(Duration::from_secs(10), sender.send(bytes)).await
                                    .map_err(|_| error("Cursor control reply timed out"))?
                                    .map_err(|_| error("Cursor upload closed during control reply"))?;
                            }
                            Action::Delta(item) => {
                                if prepared.assistant_text.len().saturating_add(item.text.len()) > MAX_BLOBS_BYTES {
                                    Err(error("Cursor response exceeds durable text limit"))?;
                                }
                                prepared.assistant_text.push_str(&item.text);
                                if !started { started = true; yield delta(StreamEventType::Start); }
                                yield item;
                            }
                            Action::Checkpoint => save_checkpoint(&prepared, &mut lease, false)?,
                            Action::Usage(count) => { usage.output_tokens = usage.output_tokens.saturating_add(count); usage.total_tokens = usage.output_tokens; }
                            Action::Tool(call) => {
                                save_checkpoint(&prepared, &mut lease, false)?;
                                if !started { started = true; yield delta(StreamEventType::Start); }
                                let mut item = delta(StreamEventType::ToolCallDelta); item.tool_call = Some(call); yield item;
                                let mut done = delta(StreamEventType::Done); done.finish_reason = Some(FinishReason::ToolCalls); done.usage = Some(usage.clone()); yield done;
                                finished = true;
                            }
                            Action::Done => {
                                save_checkpoint(&prepared, &mut lease, true)?;
                                let mut done = delta(StreamEventType::Done); done.finish_reason = Some(FinishReason::Stop); done.usage = Some(usage.clone()); yield done;
                                finished = true;
                            }
                        }
                        if finished { break; }
                    }
                    if finished { break; }
                }
                if finished { break; }
                let event = tokio::select! {
                    chunk = input.next() => Ok(Some(chunk)),
                    _ = heartbeat.tick() => Ok(None),
                    _ = tokio::time::sleep_until(deadline) => Err(RociError::Timeout(idle.as_millis().min(u64::MAX as u128) as u64)),
                };
                match event? {
                    Some(chunk) => {
                        let chunk = chunk.ok_or_else(|| error("Cursor stream ended before turn completion"))?
                            .map_err(|_| error("Cursor response stream interrupted"))?;
                        if buffer.len().saturating_add(chunk.len()) > wire::MAX_FRAME + 5 {
                            Err(error("Cursor response buffer limit exceeded"))?;
                        }
                        buffer.extend_from_slice(&chunk);
                        deadline = tokio::time::Instant::now() + idle;
                    }
                    None => {
                        let bytes = wire::client(json!({"clientHeartbeat":{}}))?;
                        sender.try_send(bytes).map_err(|_| error("Cursor heartbeat upload stalled"))?;
                    }
                }
            }
        };
        Ok(Box::pin(output))
    }
}

#[cfg(test)]
mod tests;
