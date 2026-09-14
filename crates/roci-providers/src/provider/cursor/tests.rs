use super::*;
use roci_core::{
    provider::ToolDefinition,
    types::{GenerationSettings, ModelMessage},
};

fn request() -> ProviderRequest {
    ProviderRequest {
        messages: vec![ModelMessage::user("hello")],
        settings: GenerationSettings::default(),
        tools: None,
        response_format: None,
        api_key_override: None,
        headers: Default::default(),
        metadata: Default::default(),
        payload_callback: None,
        session_id: None,
        transport: None,
    }
}

#[test]
fn request_and_control_messages_match_descriptor() {
    let mut req = request();
    req.tools = Some(vec![ToolDefinition {
        name: "echo".into(),
        description: "Echo input".into(),
        parameters: json!({"type":"object","properties":{"text":{"type":"string"}}}),
    }]);
    let mut prepared = prepare("model", &req).unwrap();
    assert!(!wire::client(prepared.message.clone()).unwrap().is_empty());
    let key = prepared.blobs.keys().next().unwrap().clone();
    let mut controls = vec![
        json!({"kvServerMessage":{"id":1,"getBlobArgs":{"blobId":key}}}),
        json!({"kvServerMessage":{"id":2,"setBlobArgs":{"blobId":STANDARD.encode(b"id"),"blobData":STANDARD.encode(b"body")}}}),
        json!({"execServerMessage":{"id":3,"execId":"ctx","requestContextArgs":{}}}),
    ];
    for kind in [
        "readArgs",
        "writeArgs",
        "deleteArgs",
        "lsArgs",
        "shellArgs",
        "shellStreamArgs",
        "backgroundShellSpawnArgs",
        "grepArgs",
        "fetchArgs",
        "writeShellStdinArgs",
        "diagnosticsArgs",
    ] {
        controls.push(json!({"execServerMessage":{"id":4,"execId":"deny",kind:{}}}));
    }
    for control in controls {
        let bytes = wire::encode("AgentServerMessage", control).unwrap();
        let decoded = wire::decode("AgentServerMessage", &bytes).unwrap();
        let actions = process(decoded, &mut prepared).unwrap();
        assert!(!actions.is_empty());
        for action in actions {
            if let Action::Reply(reply) = action {
                wire::client(reply).unwrap();
            }
        }
    }
}

#[test]
fn sdk_tool_call_and_result_survive_cold_continuation() {
    let mut req = request();
    req.tools = Some(vec![ToolDefinition {
        name: "echo".into(),
        description: "Echo".into(),
        parameters: json!({"type":"object"}),
    }]);
    let mut prepared = prepare("model", &req).unwrap();
    let args = STANDARD.encode(wire::protobuf_value(&json!("héllo")).encode_to_vec());
    let actions = process(json!({"execServerMessage":{"id":1,"execId":"exec","mcpArgs":{"toolName":"echo","toolCallId":"call","args":{"text":args}}}}), &mut prepared).unwrap();
    let call = actions
        .into_iter()
        .find_map(|action| match action {
            Action::Tool(call) => Some(call),
            _ => None,
        })
        .unwrap();
    assert_eq!(call.arguments, json!({"text":"héllo"}));
    let mut assistant = ModelMessage::assistant("");
    assistant.content.push(ContentPart::ToolCall(call));
    req.messages.push(assistant);
    let mut tool = ModelMessage::user("");
    tool.role = Role::Tool;
    tool.content.push(ContentPart::ToolResult(
        roci_core::types::message::AgentToolResult {
            tool_call_id: "call".into(),
            result: json!({"text":"héllo"}),
            is_error: false,
        },
    ));
    req.messages.push(tool);
    let prepared = prepare("model", &req).unwrap();
    let text = prepared
        .message
        .pointer("/runRequest/action/userMessageAction/userMessage/text")
        .unwrap()
        .as_str()
        .unwrap();
    assert!(text.contains("tool_call"));
    assert!(text.contains("tool_result"));
    assert!(text.contains("héllo"));
    wire::client(prepared.message).unwrap();
}

#[test]
fn repeated_completed_tool_id_returns_saved_result_without_sdk_execution() {
    let mut req = request();
    req.tools = Some(vec![ToolDefinition {
        name: "echo".into(),
        description: "Echo".into(),
        parameters: json!({"type":"object"}),
    }]);
    let mut assistant = ModelMessage::assistant("");
    assistant.content.push(ContentPart::ToolCall(AgentToolCall {
        id: "done".into(),
        name: "echo".into(),
        arguments: json!({"text":"hello"}),
        called_as: None,
        recipient: None,
    }));
    req.messages.push(assistant);
    req.messages.push(ModelMessage::tool_result(
        "done",
        json!("cached result"),
        false,
    ));
    let mut prepared = prepare("model", &req).unwrap();
    let args = STANDARD.encode(wire::protobuf_value(&json!("hello")).encode_to_vec());
    let actions = process(json!({"execServerMessage":{"id":1,"execId":"new-exec","mcpArgs":{"toolName":"echo","toolCallId":"done","args":{"text":args}}}}), &mut prepared).unwrap();
    assert!(!actions.iter().any(|a| matches!(a, Action::Tool(_))));
    let reply = actions
        .into_iter()
        .find_map(|a| match a {
            Action::Reply(v) => Some(v),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        reply
            .pointer("/execClientMessage/mcpResult/success/content/0/text/text")
            .unwrap(),
        "cached result"
    );
    wire::client(reply).unwrap();
}

#[test]
fn checkpoint_reuse_requires_matching_history_account_session_and_completed_text() {
    let mut first = request();
    first.session_id = Some("session".into());
    let prepared = prepare("model", &first).unwrap();
    let saved = CursorSessionSnapshot {
        version: 1,
        conversation_id: "upstream".into(),
        checkpoint: json!({"summary":STANDARD.encode(b"state")}),
        blobs: HashMap::new(),
        input_messages: 1,
        input_digest: prepared.input_digest.clone(),
        assistant_text: "answer".into(),
        completed_text_turn: true,
    };
    let mut next = first.clone();
    next.messages.push(ModelMessage::assistant("answer"));
    next.messages.push(ModelMessage::user("next"));
    let mut second = prepare("model", &next).unwrap();
    assert!(restore_checkpoint(&mut second, &next, saved.clone()).unwrap());
    assert_eq!(second.message["runRequest"]["conversationId"], "upstream");
    assert_eq!(
        second
            .message
            .pointer("/runRequest/action/userMessageAction/userMessage/text")
            .unwrap(),
        "next"
    );
    next.messages[0] = ModelMessage::user("edited history");
    assert!(!restore_checkpoint(&mut second, &next, saved.clone()).unwrap());
    let mut pending = saved;
    pending.completed_text_turn = false;
    assert!(!restore_checkpoint(&mut second, &next, pending).unwrap());
    assert_ne!(
        session_key("a", "m", "e", "s"),
        session_key("b", "m", "e", "s")
    );
    assert_ne!(
        session_key("a", "m", "e", "s"),
        session_key("a", "m", "e", "other")
    );
    assert_ne!(
        session_key("a", "m", "e", "s"),
        session_key("a", "other", "e", "s")
    );
}

#[tokio::test]
async fn checkpoint_and_blobs_survive_provider_recreation_and_feed_native_followup() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(FileCursorSessionStore::new(root.path()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for turn in 0..2 {
            let (socket, _) = listener.accept().await.unwrap();
            let mut conn = h2::server::handshake(socket).await.unwrap();
            let (request, mut respond) = conn.accept().await.unwrap().unwrap();
            let mut upload = request.into_body();
            let mut output = respond
                .send_response(
                    http::Response::builder().status(200).body(()).unwrap(),
                    false,
                )
                .unwrap();
            let handler = tokio::spawn(async move {
                let initial = read_client_message(&mut upload).await;
                if turn == 1 {
                    assert_eq!(
                        initial
                            .pointer("/runRequest/conversationState/summary")
                            .unwrap(),
                        &json!(STANDARD.encode(b"saved-checkpoint"))
                    );
                    assert_eq!(
                        initial
                            .pointer("/runRequest/action/userMessageAction/userMessage/text")
                            .unwrap(),
                        "followup"
                    );
                }
                let key = initial
                    .pointer("/runRequest/conversationState/rootPromptMessagesJson/0")
                    .unwrap()
                    .clone();
                // The second request must serve the previous process's system blob.
                output.send_data(bytes::Bytes::from(wire::frame(wire::encode("AgentServerMessage",json!({"kvServerMessage":{"id":1,"getBlobArgs":{"blobId":key}}})).unwrap()).unwrap()),false).unwrap();
                loop {
                    if read_client_message(&mut upload)
                        .await
                        .get("kvClientMessage")
                        .is_some()
                    {
                        break;
                    }
                }
                for update in [
                    json!({"interactionUpdate":{"textDelta":{"text":if turn == 0 {"first-answer"} else {"second-answer"}}}}),
                    json!({"conversationCheckpointUpdate":{"rootPromptMessagesJson":[key],"summary":STANDARD.encode(b"saved-checkpoint")}}),
                    json!({"interactionUpdate":{"turnEnded":{}}}),
                ] {
                    output
                        .send_data(
                            bytes::Bytes::from(
                                wire::frame(wire::encode("AgentServerMessage", update).unwrap())
                                    .unwrap(),
                            ),
                            false,
                        )
                        .unwrap();
                }
                while let Some(data) = upload.data().await {
                    if data.is_err() {
                        break;
                    }
                }
            });
            tokio::select! { result = handler => result.unwrap(), _ = async { while conn.accept().await.is_some() {} } => {} }
        }
    });
    let mut req = request();
    req.session_id = Some("persistent".into());
    let first = CursorProvider::new("model-high".into(), "token".into(), Some(endpoint.clone()))
        .with_account_id("account".into())
        .with_session_store(store.clone());
    let answer = tokio::time::timeout(Duration::from_secs(5), first.generate_text(&req))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.text, "first-answer");
    drop(first);
    req.messages.push(ModelMessage::assistant(answer.text));
    req.messages.push(ModelMessage::user("followup"));
    let second = CursorProvider::new("model-high".into(), "rotated-token".into(), Some(endpoint))
        .with_account_id("account".into())
        .with_session_store(Arc::new(FileCursorSessionStore::new(root.path())));
    let answer = tokio::time::timeout(Duration::from_secs(5), second.generate_text(&req))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.text, "second-answer");
    server.await.unwrap();
}

#[test]
fn unsupported_settings_fail_before_network() {
    let mut req = request();
    req.settings.temperature = Some(0.2);
    assert!(matches!(
        prepare("model", &req),
        Err(RociError::UnsupportedOperation(_))
    ));
}

#[test]
fn namespace_and_override_isolate_leases_without_using_rotating_tokens() {
    let root = tempfile::tempdir().unwrap();
    let store = Arc::new(FileCursorSessionStore::new(root.path()));
    let mut req = request();
    req.session_id = Some("session".into());
    let first = CursorProvider::new("model-high".into(), "old-token".into(), None)
        .with_account_namespace("work".into())
        .with_account_id("upstream-user".into())
        .with_session_store(store.clone());
    let _first_lease = first.session_lease(&req, "model").unwrap();
    let same = CursorProvider::new("model-high".into(), "new-token".into(), None)
        .with_account_namespace("work".into())
        .with_account_id("upstream-user".into())
        .with_session_store(store.clone());
    assert!(same.session_lease(&req, "model").is_err());
    let other = CursorProvider::new("model-high".into(), "new-token".into(), None)
        .with_account_namespace("personal".into())
        .with_account_id("upstream-user".into())
        .with_session_store(store.clone());
    assert!(other.session_lease(&req, "model").is_ok());
    let opaque = CursorProvider::new("model-high".into(), "opaque-token".into(), None)
        .with_session_store(store);
    assert!(opaque.session_lease(&req, "model").is_err());
}

#[tokio::test]
async fn native_h2_stream_replies_to_blob_and_context_before_text() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut connection = h2::server::handshake(socket).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.uri().path(), "/agent.v1.AgentService/Run");
        assert_eq!(request.headers()["authorization"], "Bearer test-token");
        let mut upload = request.into_body();
        let mut output = respond
            .send_response(
                http::Response::builder()
                    .status(200)
                    .header("content-type", "application/connect+proto")
                    .body(())
                    .unwrap(),
                false,
            )
            .unwrap();
        let handler = tokio::spawn(async move {
            let initial = read_client_message(&mut upload).await;
            let key = initial
                .pointer("/runRequest/conversationState/rootPromptMessagesJson/0")
                .unwrap()
                .clone();
            output
                .send_data(
                    bytes::Bytes::from(
                        wire::frame(
                            wire::encode(
                                "AgentServerMessage",
                                json!({"kvServerMessage":{"id":1,"getBlobArgs":{"blobId":key}}}),
                            )
                            .unwrap(),
                        )
                        .unwrap(),
                    ),
                    false,
                )
                .unwrap();
            loop {
                let reply = read_client_message(&mut upload).await;
                if reply.get("kvClientMessage").is_some() {
                    let data = wire::bytes(&reply["kvClientMessage"]["getBlobResult"]["blobData"])
                        .unwrap();
                    assert!(String::from_utf8(data).unwrap().contains("system"));
                    break;
                }
            }
            output.send_data(bytes::Bytes::from(wire::frame(wire::encode("AgentServerMessage", json!({"execServerMessage":{"id":2,"execId":"ctx","requestContextArgs":{}}})).unwrap()).unwrap()), false).unwrap();
            loop {
                let reply = read_client_message(&mut upload).await;
                if reply.get("execClientMessage").is_some() {
                    assert!(reply
                        .pointer("/execClientMessage/requestContextResult/success/requestContext")
                        .is_some());
                    break;
                }
            }
            output
                .send_data(
                    bytes::Bytes::from(
                        wire::frame(
                            wire::encode(
                                "AgentServerMessage",
                                json!({"interactionUpdate":{"textDelta":{"text":"native-ok"}}}),
                            )
                            .unwrap(),
                        )
                        .unwrap(),
                    ),
                    false,
                )
                .unwrap();
            output
                .send_data(
                    bytes::Bytes::from(
                        wire::frame(
                            wire::encode(
                                "AgentServerMessage",
                                json!({"interactionUpdate":{"turnEnded":{}}}),
                            )
                            .unwrap(),
                        )
                        .unwrap(),
                    ),
                    true,
                )
                .unwrap();
            // Keep the h2 connection driver alive until the client has consumed
            // the response and closed its upload stream.
            while let Some(data) = upload.data().await {
                if data.is_err() {
                    break;
                }
            }
        });
        tokio::select! {
            result = handler => result.unwrap(),
            _ = async { while connection.accept().await.is_some() {} } => {}
        }
    });
    let provider = CursorProvider::new("model-high".into(), "test-token".into(), Some(endpoint));
    let response = tokio::time::timeout(Duration::from_secs(5), provider.generate_text(&request()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.text, "native-ok");
    assert_eq!(response.finish_reason, Some(FinishReason::Stop));
    server.await.unwrap();
}

async fn read_client_message(stream: &mut h2::RecvStream) -> Value {
    loop {
        let data = stream.data().await.unwrap().unwrap();
        stream.flow_control().release_capacity(data.len()).unwrap();
        // Test client writes each small control as one DATA frame.
        assert!(data.len() >= 5);
        let length = u32::from_be_bytes([data[1], data[2], data[3], data[4]]) as usize;
        if length == 0 {
            continue;
        }
        return wire::decode("AgentClientMessage", &data[5..5 + length]).unwrap();
    }
}

#[tokio::test]
async fn native_model_catalog_uses_one_unary_media_type_and_empty_protobuf() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await.unwrap();
        let mut connection = h2::server::handshake(socket).await.unwrap();
        let (request, mut respond) = connection.accept().await.unwrap().unwrap();
        assert_eq!(request.method(), http::Method::POST);
        assert_eq!(
            request.uri().path(),
            "/agent.v1.AgentService/GetUsableModels"
        );
        assert_eq!(request.headers().get_all("content-type").iter().count(), 1);
        assert_eq!(request.headers()["content-type"], "application/proto");
        assert!(!request.headers().contains_key("connect-protocol-version"));
        assert_eq!(request.headers()["authorization"], "Bearer catalog-token");
        assert_eq!(request.headers()["x-cursor-client-type"], "cli");
        let handler = tokio::spawn(async move {
            let mut upload = request.into_body();
            while let Some(chunk) = upload.data().await {
                assert!(
                    chunk.unwrap().is_empty(),
                    "unary empty request must not have a Connect frame"
                );
            }
            let body = wire::encode(
                "GetUsableModelsResponse",
                json!({
                    "models": [{"modelId":"claude-sonnet-4.6"}, {"modelId":"gpt-5"}]
                }),
            )
            .unwrap();
            let mut output = respond
                .send_response(
                    http::Response::builder()
                        .status(200)
                        .header("content-type", "application/proto")
                        .body(())
                        .unwrap(),
                    false,
                )
                .unwrap();
            output.send_data(bytes::Bytes::from(body), true).unwrap();
        });
        // Drive HTTP/2 until the client consumes the response and closes its client.
        while connection.accept().await.is_some() {}
        handler.await.unwrap();
    });
    let provider = CursorProvider::new(String::new(), "catalog-token".into(), Some(endpoint));
    let models = tokio::time::timeout(Duration::from_secs(5), provider.list_model_ids())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(models, ["claude-sonnet-4.6", "gpt-5"]);
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn family_resolution_uses_override_catalog_and_resolved_variant_session_key() {
    let root = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let override_token = format!(
        "header.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"actual-user"}"#)
    );
    let expected_bearer = format!("Bearer {override_token}");
    let server = tokio::spawn(async move {
        for catalog in [true, false] {
            let (socket, _) = listener.accept().await.unwrap();
            let mut connection = h2::server::handshake(socket).await.unwrap();
            let (request, mut respond) = connection.accept().await.unwrap().unwrap();
            assert_eq!(request.headers()["authorization"], expected_bearer);
            assert_eq!(
                request.uri().path(),
                if catalog {
                    "/agent.v1.AgentService/GetUsableModels"
                } else {
                    "/agent.v1.AgentService/Run"
                }
            );
            let handler = tokio::spawn(async move {
                let mut upload = request.into_body();
                if catalog {
                    while let Some(chunk) = upload.data().await {
                        assert!(chunk.unwrap().is_empty());
                    }
                } else {
                    let initial = read_client_message(&mut upload).await;
                    assert_eq!(
                        initial["runRequest"]["modelDetails"]["modelId"],
                        "claude-fable-5-thinking-high-fast"
                    );
                }
                let mut output = respond
                    .send_response(
                        http::Response::builder()
                            .status(200)
                            .header(
                                "content-type",
                                if catalog {
                                    "application/proto"
                                } else {
                                    "application/connect+proto"
                                },
                            )
                            .body(())
                            .unwrap(),
                        false,
                    )
                    .unwrap();
                if catalog {
                    let body = wire::encode("GetUsableModelsResponse", json!({"models":[
                        {"modelId":"claude-fable-5-thinking-medium"}, {"modelId":"claude-fable-5-thinking-high-fast"}
                    ]})).unwrap();
                    output.send_data(bytes::Bytes::from(body), true).unwrap();
                } else {
                    for (update, end) in [
                        (
                            json!({"interactionUpdate":{"textDelta":{"text":"resolved"}}}),
                            false,
                        ),
                        (json!({"interactionUpdate":{"turnEnded":{}}}), true),
                    ] {
                        output
                            .send_data(
                                bytes::Bytes::from(
                                    wire::frame(
                                        wire::encode("AgentServerMessage", update).unwrap(),
                                    )
                                    .unwrap(),
                                ),
                                end,
                            )
                            .unwrap();
                    }
                    while let Some(chunk) = upload.data().await {
                        if chunk.is_err() {
                            break;
                        }
                    }
                }
            });
            while connection.accept().await.is_some() {}
            handler.await.unwrap();
        }
    });
    let provider = CursorProvider::new(
        "claude-fable-5".into(),
        "wrong-seed-token".into(),
        Some(endpoint.clone()),
    )
    .with_account_id("wrong-seed-account".into())
    .with_session_store(Arc::new(FileCursorSessionStore::new(root.path())));
    let mut req = request();
    req.api_key_override = Some(override_token);
    req.session_id = Some("session".into());
    req.settings.reasoning_effort = Some(roci_core::types::ReasoningEffort::High);
    req.settings.speed = Some(roci_core::types::GenerationSpeed::Fast);
    let answer = tokio::time::timeout(Duration::from_secs(5), provider.generate_text(&req))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(answer.text, "resolved");
    let account = digest(&("default", true, "override:actual-user")).unwrap();
    let resolved_key = session_key(
        &account,
        "claude-fable-5-thinking-high-fast",
        &endpoint,
        "session",
    );
    assert!(root.path().join(format!("{resolved_key}.lock")).exists());
    let unresolved_key = session_key(&account, "claude-fable-5", &endpoint, "session");
    assert!(!root.path().join(format!("{unresolved_key}.lock")).exists());
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();
}
