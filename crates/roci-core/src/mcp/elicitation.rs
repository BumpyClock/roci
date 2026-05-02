//! MCP client-side elicitation handling.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use rmcp::model::{
    ClientInfo, CreateElicitationRequestParams, CreateElicitationResult, ElicitationAction,
    ElicitationCapability, ElicitationSchema, EnumSchema, ErrorData as McpError,
    FormElicitationCapability, Meta, MultiSelectEnumSchema, PrimitiveSchema, ProtocolVersion,
    SingleSelectEnumSchema,
};
use rmcp::service::{RequestContext, RoleClient};
use rmcp::ClientHandler;
use serde_json::Value;
use uuid::Uuid;

use crate::human_interaction::{
    HumanInteractionCoordinator, HumanInteractionPayload, HumanInteractionRequest,
    HumanInteractionResponse, HumanInteractionResponsePayload, HumanInteractionSource,
    UiElicitationField, UiElicitationRequest, UiElicitationResponse, UiElicitationSchema,
};

const ELICITATION_CREATE_OPERATION: &str = "elicitation/create";

#[derive(Debug, Clone)]
pub struct MCPClientHandler {
    client_info: ClientInfo,
    ui_elicitation: Option<MCPUiElicitationHandler>,
}

#[derive(Debug, Clone)]
struct MCPUiElicitationHandler {
    server_id: String,
    coordinator: Arc<HumanInteractionCoordinator>,
}

impl MCPClientHandler {
    pub fn new(protocol_version: ProtocolVersion) -> Self {
        Self {
            client_info: ClientInfo {
                protocol_version,
                ..Default::default()
            },
            ui_elicitation: None,
        }
    }

    pub fn with_ui_elicitation(
        mut self,
        server_id: String,
        coordinator: Arc<HumanInteractionCoordinator>,
    ) -> Self {
        self.client_info.capabilities.elicitation = Some(ElicitationCapability {
            form: Some(FormElicitationCapability {
                schema_validation: Some(true),
            }),
            url: None,
        });
        self.ui_elicitation = Some(MCPUiElicitationHandler {
            server_id,
            coordinator,
        });
        self
    }

    pub fn client_info(&self) -> &ClientInfo {
        &self.client_info
    }

    async fn handle_create_elicitation(
        &self,
        params: CreateElicitationRequestParams,
    ) -> Result<CreateElicitationResult, McpError> {
        let Some(handler) = &self.ui_elicitation else {
            return Ok(CreateElicitationResult {
                action: ElicitationAction::Decline,
                content: None,
            });
        };

        let (meta, request) = map_mcp_elicitation_request(&handler.server_id, params)?;
        reject_sensitive_elicitation(meta.as_ref(), &request)?;

        let pending = handler
            .coordinator
            .create_request(request.clone())
            .await
            .map_err(|error| McpError::internal_error(error.to_string(), None))?;

        let response = pending.wait(request.timeout_ms).await;
        map_ui_elicitation_response(response, &request)
    }
}

impl ClientHandler for MCPClientHandler {
    async fn create_elicitation(
        &self,
        request: CreateElicitationRequestParams,
        _context: RequestContext<RoleClient>,
    ) -> Result<CreateElicitationResult, McpError> {
        self.handle_create_elicitation(request).await
    }

    fn get_info(&self) -> ClientInfo {
        self.client_info.clone()
    }
}

fn map_mcp_elicitation_request(
    server_id: &str,
    params: CreateElicitationRequestParams,
) -> Result<(Option<Meta>, HumanInteractionRequest), McpError> {
    let (meta, message, requested_schema) = match params {
        CreateElicitationRequestParams::FormElicitationParams {
            meta,
            message,
            requested_schema,
        } => (meta, message, requested_schema),
        CreateElicitationRequestParams::UrlElicitationParams { .. } => {
            return Err(McpError::invalid_params(
                "MCP URL elicitation is not supported",
                None,
            ));
        }
    };

    let requested_schema = map_elicitation_schema(requested_schema)?;
    let request = UiElicitationRequest::form(message, requested_schema)
        .map_err(|error| McpError::invalid_params(error.to_string(), None))?;

    Ok((
        meta,
        HumanInteractionRequest {
            request_id: Uuid::new_v4(),
            source: HumanInteractionSource::Mcp {
                server_id: server_id.to_string(),
                operation: Some(ELICITATION_CREATE_OPERATION.to_string()),
            },
            payload: HumanInteractionPayload::UiElicitation(request),
            timeout_ms: None,
            created_at: Utc::now(),
        },
    ))
}

fn map_elicitation_schema(schema: ElicitationSchema) -> Result<UiElicitationSchema, McpError> {
    let properties = schema
        .properties
        .into_iter()
        .map(|(name, schema)| {
            let field = map_primitive_schema(&name, schema)?;
            Ok((name, field))
        })
        .collect::<Result<HashMap<_, _>, McpError>>()?;

    let required = schema.required.unwrap_or_default();
    let schema = UiElicitationSchema::object(properties, required);
    schema
        .validate()
        .map_err(|error| McpError::invalid_params(error.to_string(), None))?;

    Ok(schema)
}

fn map_primitive_schema(
    field_name: &str,
    schema: PrimitiveSchema,
) -> Result<UiElicitationField, McpError> {
    match schema {
        PrimitiveSchema::String(schema) => Ok(UiElicitationField::String {
            title: schema.title.map(|value| value.to_string()),
            description: schema.description.map(|value| value.to_string()),
            default: None,
            enum_values: None,
        }),
        PrimitiveSchema::Number(schema) => Ok(UiElicitationField::Number {
            title: schema.title.map(|value| value.to_string()),
            description: schema.description.map(|value| value.to_string()),
            default: None,
        }),
        PrimitiveSchema::Integer(schema) => Ok(UiElicitationField::Integer {
            title: schema.title.map(|value| value.to_string()),
            description: schema.description.map(|value| value.to_string()),
            default: None,
        }),
        PrimitiveSchema::Boolean(schema) => Ok(UiElicitationField::Boolean {
            title: schema.title.map(|value| value.to_string()),
            description: schema.description.map(|value| value.to_string()),
            default: schema.default,
        }),
        PrimitiveSchema::Enum(schema) => map_enum_schema(field_name, schema),
    }
}

fn map_enum_schema(field_name: &str, schema: EnumSchema) -> Result<UiElicitationField, McpError> {
    match schema {
        EnumSchema::Single(schema) => {
            let (title, description, enum_values, default) = match schema {
                SingleSelectEnumSchema::Untitled(schema) => (
                    schema.title.map(|value| value.to_string()),
                    schema.description.map(|value| value.to_string()),
                    schema.enum_,
                    schema.default,
                ),
                SingleSelectEnumSchema::Titled(schema) => (
                    schema.title.map(|value| value.to_string()),
                    schema.description.map(|value| value.to_string()),
                    schema.one_of.into_iter().map(|item| item.const_).collect(),
                    schema.default,
                ),
            };
            Ok(UiElicitationField::String {
                title,
                description,
                default,
                enum_values: Some(enum_values),
            })
        }
        EnumSchema::Legacy(schema) => Ok(UiElicitationField::String {
            title: schema.title.map(|value| value.to_string()),
            description: schema.description.map(|value| value.to_string()),
            default: None,
            enum_values: Some(schema.enum_),
        }),
        EnumSchema::Multi(MultiSelectEnumSchema::Untitled(_))
        | EnumSchema::Multi(MultiSelectEnumSchema::Titled(_)) => Err(McpError::invalid_params(
            format!("MCP elicitation field '{field_name}' uses unsupported multi-select enum"),
            None,
        )),
    }
}

fn reject_sensitive_elicitation(
    meta: Option<&Meta>,
    request: &HumanInteractionRequest,
) -> Result<(), McpError> {
    if meta.is_some_and(meta_marks_sensitive) {
        return Err(McpError::invalid_params(
            "MCP sensitive elicitation is not supported",
            None,
        ));
    }

    let HumanInteractionPayload::UiElicitation(payload) = &request.payload else {
        return Ok(());
    };

    if contains_sensitive_text(&payload.message) {
        return Err(McpError::invalid_params(
            "MCP sensitive elicitation is not supported",
            None,
        ));
    }

    for (name, field) in &payload.requested_schema.properties {
        if contains_sensitive_text(name) {
            return Err(McpError::invalid_params(
                "MCP sensitive elicitation is not supported",
                None,
            ));
        }
        if field_text(field)
            .into_iter()
            .any(|text| contains_sensitive_text(&text))
        {
            return Err(McpError::invalid_params(
                "MCP sensitive elicitation is not supported",
                None,
            ));
        }
    }

    Ok(())
}

fn meta_marks_sensitive(meta: &Meta) -> bool {
    meta.iter().any(|(key, value)| {
        let key = key.to_ascii_lowercase();
        (key == "sensitive" || key == "secret" || key == "requires_sensitive")
            && value.as_bool().unwrap_or(false)
    })
}

fn field_text(field: &UiElicitationField) -> Vec<String> {
    match field {
        UiElicitationField::String {
            title, description, ..
        }
        | UiElicitationField::Boolean {
            title, description, ..
        }
        | UiElicitationField::Number {
            title, description, ..
        }
        | UiElicitationField::Integer {
            title, description, ..
        } => [title.clone(), description.clone()]
            .into_iter()
            .flatten()
            .collect(),
    }
}

fn contains_sensitive_text(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    [
        "password",
        "secret",
        "api key",
        "api_key",
        "token",
        "credential",
        "private key",
        "private_key",
    ]
    .iter()
    .any(|needle| value.contains(needle))
}

fn map_ui_elicitation_response(
    response: Result<HumanInteractionResponse, crate::human_interaction::HumanInteractionError>,
    request: &HumanInteractionRequest,
) -> Result<CreateElicitationResult, McpError> {
    match response {
        Ok(response) => match response.payload {
            HumanInteractionResponsePayload::UiElicitation(UiElicitationResponse::Accept {
                content,
            }) => {
                validate_content(&request.payload, &content)?;
                Ok(CreateElicitationResult {
                    action: ElicitationAction::Accept,
                    content: Some(Value::Object(content)),
                })
            }
            HumanInteractionResponsePayload::UiElicitation(UiElicitationResponse::Decline)
            | HumanInteractionResponsePayload::Declined => Ok(CreateElicitationResult {
                action: ElicitationAction::Decline,
                content: None,
            }),
            HumanInteractionResponsePayload::UiElicitation(UiElicitationResponse::Cancel)
            | HumanInteractionResponsePayload::Canceled => Ok(CreateElicitationResult {
                action: ElicitationAction::Cancel,
                content: None,
            }),
            _ => Err(McpError::invalid_params(
                "human interaction response was not UI elicitation",
                None,
            )),
        },
        Err(crate::human_interaction::HumanInteractionError::Canceled { .. })
        | Err(crate::human_interaction::HumanInteractionError::Timeout { .. }) => {
            Ok(CreateElicitationResult {
                action: ElicitationAction::Cancel,
                content: None,
            })
        }
        Err(crate::human_interaction::HumanInteractionError::Unavailable { .. }) => {
            Ok(CreateElicitationResult {
                action: ElicitationAction::Decline,
                content: None,
            })
        }
        Err(error) => Err(McpError::internal_error(error.to_string(), None)),
    }
}

fn validate_content(
    payload: &HumanInteractionPayload,
    content: &serde_json::Map<String, Value>,
) -> Result<(), McpError> {
    let HumanInteractionPayload::UiElicitation(request) = payload else {
        return Err(McpError::invalid_params(
            "human interaction request was not UI elicitation",
            None,
        ));
    };

    for required in &request.requested_schema.required {
        if !content.contains_key(required) {
            return Err(McpError::invalid_params(
                format!("MCP elicitation response missing required field '{required}'"),
                None,
            ));
        }
    }

    for (field_name, value) in content {
        let Some(field) = request.requested_schema.properties.get(field_name) else {
            return Err(McpError::invalid_params(
                format!("MCP elicitation response contains unknown field '{field_name}'"),
                None,
            ));
        };
        validate_field_content(field_name, field, value)?;
    }

    Ok(())
}

fn validate_field_content(
    field_name: &str,
    field: &UiElicitationField,
    value: &Value,
) -> Result<(), McpError> {
    match field {
        UiElicitationField::String { enum_values, .. } => {
            let Some(value) = value.as_str() else {
                return Err(invalid_field_type(field_name, "string"));
            };
            if let Some(enum_values) = enum_values {
                if !enum_values.iter().any(|enum_value| enum_value == value) {
                    return Err(McpError::invalid_params(
                        format!(
                            "MCP elicitation response field '{field_name}' must match schema enum"
                        ),
                        None,
                    ));
                }
            }
        }
        UiElicitationField::Boolean { .. } if !value.is_boolean() => {
            return Err(invalid_field_type(field_name, "boolean"));
        }
        UiElicitationField::Number { .. } if !value.is_number() => {
            return Err(invalid_field_type(field_name, "number"));
        }
        UiElicitationField::Integer { .. } if value.as_i64().is_none() => {
            return Err(invalid_field_type(field_name, "integer"));
        }
        _ => {}
    }

    Ok(())
}

fn invalid_field_type(field_name: &str, expected: &str) -> McpError {
    McpError::invalid_params(
        format!("MCP elicitation response field '{field_name}' must be {expected}"),
        None,
    )
}

#[cfg(test)]
mod tests;
