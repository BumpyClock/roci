use super::*;
use rmcp::model::{BooleanSchema, IntegerSchema, StringSchema};
use serde_json::{json, Map};
use tokio::time::{sleep, timeout, Duration};

fn form_params(schema: ElicitationSchema) -> CreateElicitationRequestParams {
    CreateElicitationRequestParams::FormElicitationParams {
        meta: None,
        message: "Need deployment details".to_string(),
        requested_schema: schema,
    }
}

fn simple_schema() -> ElicitationSchema {
    ElicitationSchema::builder()
        .required_string("environment")
        .optional_bool("dry_run", false)
        .build()
        .expect("schema should build")
}

async fn next_pending_request(
    coordinator: &HumanInteractionCoordinator,
) -> HumanInteractionRequest {
    timeout(Duration::from_secs(1), async {
        loop {
            if let Some(request) = coordinator.pending_requests().await.into_iter().next() {
                return request;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("pending request should be created")
}

#[tokio::test]
async fn handler_advertises_form_only_elicitation_when_coordinator_exists() {
    let coordinator = Arc::new(HumanInteractionCoordinator::new());
    let handler = MCPClientHandler::new(ProtocolVersion::LATEST)
        .with_ui_elicitation("alpha".to_string(), coordinator);

    let capability = handler
        .client_info()
        .capabilities
        .elicitation
        .as_ref()
        .expect("elicitation capability should be advertised");

    assert!(capability.form.is_some());
    assert!(capability.url.is_none());
}

#[tokio::test]
async fn handler_does_not_advertise_elicitation_without_coordinator() {
    let handler = MCPClientHandler::new(ProtocolVersion::LATEST);

    assert!(handler.client_info().capabilities.elicitation.is_none());
    let result = handler
        .handle_create_elicitation(form_params(simple_schema()))
        .await
        .expect("no handler should decline deterministically");

    assert_eq!(result.action, ElicitationAction::Decline);
    assert!(result.content.is_none());
}

#[tokio::test]
async fn create_elicitation_maps_request_and_accept_response() {
    let coordinator = Arc::new(HumanInteractionCoordinator::new());
    let handler = MCPClientHandler::new(ProtocolVersion::LATEST)
        .with_ui_elicitation("alpha".to_string(), coordinator.clone());
    let task = tokio::spawn(async move {
        handler
            .handle_create_elicitation(form_params(simple_schema()))
            .await
    });

    let request = next_pending_request(&coordinator).await;
    assert_eq!(
        request.source,
        HumanInteractionSource::Mcp {
            server_id: "alpha".to_string(),
            operation: Some(ELICITATION_CREATE_OPERATION.to_string()),
        }
    );
    let HumanInteractionPayload::UiElicitation(payload) = &request.payload else {
        panic!("request should be UI elicitation");
    };
    assert_eq!(payload.message, "Need deployment details");
    assert!(payload
        .requested_schema
        .properties
        .contains_key("environment"));

    let mut content = Map::new();
    content.insert("environment".to_string(), json!("staging"));
    content.insert("dry_run".to_string(), json!(true));
    coordinator
        .submit_response(HumanInteractionResponse {
            request_id: request.request_id,
            payload: HumanInteractionResponsePayload::UiElicitation(
                UiElicitationResponse::Accept { content },
            ),
            resolved_at: Utc::now(),
        })
        .await
        .expect("response should submit");

    let result = task.await.expect("task should join").expect("MCP result");
    assert_eq!(result.action, ElicitationAction::Accept);
    assert_eq!(
        result.content,
        Some(json!({"environment": "staging", "dry_run": true}))
    );
}

#[tokio::test]
async fn create_elicitation_maps_decline_response() {
    let coordinator = Arc::new(HumanInteractionCoordinator::new());
    let handler = MCPClientHandler::new(ProtocolVersion::LATEST)
        .with_ui_elicitation("alpha".to_string(), coordinator.clone());
    let task = tokio::spawn(async move {
        handler
            .handle_create_elicitation(form_params(simple_schema()))
            .await
    });

    let request = next_pending_request(&coordinator).await;
    coordinator
        .submit_response(HumanInteractionResponse {
            request_id: request.request_id,
            payload: HumanInteractionResponsePayload::UiElicitation(UiElicitationResponse::Decline),
            resolved_at: Utc::now(),
        })
        .await
        .expect("response should submit");

    let result = task.await.expect("task should join").expect("MCP result");
    assert_eq!(result.action, ElicitationAction::Decline);
    assert!(result.content.is_none());
}

#[tokio::test]
async fn create_elicitation_maps_cancel_response() {
    let coordinator = Arc::new(HumanInteractionCoordinator::new());
    let handler = MCPClientHandler::new(ProtocolVersion::LATEST)
        .with_ui_elicitation("alpha".to_string(), coordinator.clone());
    let task = tokio::spawn(async move {
        handler
            .handle_create_elicitation(form_params(simple_schema()))
            .await
    });

    let request = next_pending_request(&coordinator).await;
    coordinator
        .submit_response(HumanInteractionResponse {
            request_id: request.request_id,
            payload: HumanInteractionResponsePayload::UiElicitation(UiElicitationResponse::Cancel),
            resolved_at: Utc::now(),
        })
        .await
        .expect("response should submit");

    let result = task.await.expect("task should join").expect("MCP result");
    assert_eq!(result.action, ElicitationAction::Cancel);
    assert!(result.content.is_none());
}

#[tokio::test]
async fn invalid_schema_is_rejected() {
    let mut schema = ElicitationSchema::new(
        [(
            "count".to_string(),
            PrimitiveSchema::Integer(IntegerSchema::new()),
        )]
        .into_iter()
        .collect(),
    );
    schema.required = Some(vec!["missing".to_string()]);
    let handler = MCPClientHandler::new(ProtocolVersion::LATEST).with_ui_elicitation(
        "alpha".to_string(),
        Arc::new(HumanInteractionCoordinator::new()),
    );

    let err = handler
        .handle_create_elicitation(form_params(schema))
        .await
        .expect_err("invalid schema should be rejected");

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn url_elicitation_is_rejected() {
    let handler = MCPClientHandler::new(ProtocolVersion::LATEST).with_ui_elicitation(
        "alpha".to_string(),
        Arc::new(HumanInteractionCoordinator::new()),
    );

    let err = handler
        .handle_create_elicitation(CreateElicitationRequestParams::UrlElicitationParams {
            meta: None,
            message: "Open this URL".to_string(),
            url: "https://example.com/form".to_string(),
            elicitation_id: "elicit_123".to_string(),
        })
        .await
        .expect_err("URL mode should be rejected");

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn sensitive_elicitation_is_rejected() {
    let schema = ElicitationSchema::builder()
        .required_property("password", PrimitiveSchema::String(StringSchema::new()))
        .build()
        .expect("schema should build");
    let handler = MCPClientHandler::new(ProtocolVersion::LATEST).with_ui_elicitation(
        "alpha".to_string(),
        Arc::new(HumanInteractionCoordinator::new()),
    );

    let err = handler
        .handle_create_elicitation(form_params(schema))
        .await
        .expect_err("sensitive schema should be rejected");

    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
}

#[tokio::test]
async fn accepted_content_must_match_schema() {
    let coordinator = Arc::new(HumanInteractionCoordinator::new());
    let schema = ElicitationSchema::builder()
        .required_property("dry_run", PrimitiveSchema::Boolean(BooleanSchema::new()))
        .build()
        .expect("schema should build");
    let handler = MCPClientHandler::new(ProtocolVersion::LATEST)
        .with_ui_elicitation("alpha".to_string(), coordinator.clone());
    let task =
        tokio::spawn(async move { handler.handle_create_elicitation(form_params(schema)).await });

    let request = next_pending_request(&coordinator).await;
    let mut content = Map::new();
    content.insert("dry_run".to_string(), json!("yes"));
    coordinator
        .submit_response(HumanInteractionResponse {
            request_id: request.request_id,
            payload: HumanInteractionResponsePayload::UiElicitation(
                UiElicitationResponse::Accept { content },
            ),
            resolved_at: Utc::now(),
        })
        .await
        .expect("response should submit");

    let err = task
        .await
        .expect("task should join")
        .expect_err("invalid content should be rejected");
    assert_eq!(err.code, rmcp::model::ErrorCode::INVALID_PARAMS);
}
